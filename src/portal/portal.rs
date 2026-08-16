mod wayland;

use confique::Config;
use log::{debug, info};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, mpsc},
};
use tokio::{
    process::Command,
    sync::Mutex,
    time::{Duration, sleep},
};
use zbus::{
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedValue},
};

const SERVICE_NAME: &str = "org.freedesktop.impl.portal.desktop.bgapp";
const SERVICE_PATH: &str = "/org/freedesktop/portal/desktop";

#[derive(Config, Clone, Debug)]
struct AppConfig {
    pub update_cmdline: Option<String>,
    #[config(default = 1000)]
    pub update_interval_ms: u32,
}

// Portal backend implementation we provide for xdg-desktop-portal.
struct PortalInterface {
    app_id_set: Arc<Mutex<HashSet<String>>>,
}

impl PortalInterface {
    fn new(app_id_set: Arc<Mutex<HashSet<String>>>) -> Self {
        Self { app_id_set }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Background", spawn = false)]
impl PortalInterface {
    #[zbus(out_args("apps"))]
    async fn get_app_state(&self) -> zbus::fdo::Result<HashMap<String, OwnedValue>> {
        debug!("GetAppState called");
        let mut app_states = HashMap::new();
        for app_id in &*self.app_id_set.lock().await {
            app_states.insert(app_id.to_string(), OwnedValue::from(1u32));
        }
        Ok(app_states)
    }

    // Always allow apps to go background, without any notification.
    // This is consistent with GNOME. (KDE shows a notification.)
    #[zbus(out_args("response", "results"))]
    #[allow(unused_variables)]
    fn notify_background(
        &self,
        handle: ObjectPath<'_>,
        app_id: String,
        name: String,
    ) -> zbus::fdo::Result<(u32, HashMap<String, OwnedValue>)> {
        debug!("NotifyBackground called");
        Ok((
            0,
            HashMap::from([("result".to_string(), OwnedValue::from(1u32))]),
        ))
    }

    // Deprecated. Implemented as no-op.
    #[zbus(out_args("result"))]
    #[allow(unused_variables)]
    fn enable_autostart(
        &self,
        app_id: String,
        enable: bool,
        commandline: Vec<String>,
        flags: u32,
    ) -> zbus::fdo::Result<bool> {
        debug!("EnableAutostart called");
        Ok(false)
    }

    #[zbus(signal)]
    async fn running_applications_changed(signal_ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct StateUpdater {
    config: AppConfig,
    app_id_set: Arc<Mutex<HashSet<String>>>,
    portal_interface: zbus::object_server::InterfaceRef<PortalInterface>,
}

impl StateUpdater {
    fn new(
        app_id_set: Arc<Mutex<HashSet<String>>>,
        portal_interface: zbus::object_server::InterfaceRef<PortalInterface>,
        config: AppConfig,
    ) -> Self {
        Self {
            config,
            app_id_set,
            portal_interface,
        }
    }

    async fn update_app_states_cmdline(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let update_cmdline = self.config.update_cmdline.as_ref().unwrap();
        let update_interval_ms = self.config.update_interval_ms;
        info!("update-cmdline: {update_cmdline}",);
        info!("update-interval-ms: {update_interval_ms}");

        loop {
            let output = Command::new("sh")
                .arg("-c")
                .arg(update_cmdline)
                .output()
                .await?;
            let app_id_list = String::from_utf8(output.stdout)?;
            let app_id_set_new: HashSet<String> =
                app_id_list.lines().map(|s| s.to_string()).collect();
            {
                let app_id_set = self.app_id_set.lock().await;
                if app_id_set_new == *app_id_set {
                    continue;
                }
            }
            info!("App states changed");
            {
                let mut app_id_set = self.app_id_set.lock().await;
                if log::log_enabled!(log::Level::Info) {
                    let mut vec: Vec<_> = app_id_set_new.iter().collect();
                    vec.sort();
                    for app_id in &vec {
                        info!("    app_id: {app_id}");
                    }
                }
                *app_id_set = app_id_set_new;
            }
            self.portal_interface.running_applications_changed().await?;
            sleep(Duration::from_millis(update_interval_ms.into())).await;
        }
    }

    async fn update_app_states_wayland(
        &mut self,
        updates: mpsc::Receiver<HashSet<String>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while let Ok(new_app_id_set) = updates.recv() {
            info!("App states changed");
            {
                let mut app_id_set = self.app_id_set.lock().await;
                for app_id in &new_app_id_set {
                    info!("    app_id: {app_id}");
                }
                *app_id_set = new_app_id_set;
            }
            self.portal_interface.running_applications_changed().await?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config_file = "xdg-desktop-portal-bgapp.toml";
    let config_path = xdg::BaseDirectories::new().find_config_file(config_file);
    info!("Using config file: {:?}", config_path);
    let config =
        AppConfig::from_file(config_path.unwrap_or(PathBuf::from(config_file.to_string())))?;
    let update_cmdline = config.update_cmdline.to_owned();

    let app_id_set: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));

    let connection = zbus::Connection::session().await?;
    let portal_interface = PortalInterface::new(app_id_set.clone());
    connection
        .object_server()
        .at(SERVICE_PATH, portal_interface)
        .await?;
    connection.request_name(SERVICE_NAME).await?;

    let mut state_updater = StateUpdater::new(
        app_id_set.clone(),
        connection.object_server().interface(SERVICE_PATH).await?,
        config,
    );

    if update_cmdline.is_some() {
        state_updater.update_app_states_cmdline().await?;
    } else {
        let updates = wayland::connect()?;
        state_updater.update_app_states_wayland(updates).await?;
    }
    Ok(())
}
