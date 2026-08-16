use std::collections::{HashMap, HashSet};
use tokio::time::{Duration, sleep};
use std::sync::{Arc, RwLock};
use zbus::{
    fdo::{Error, Result},
    object_server::SignalEmitter,
    zvariant::{ObjectPath, OwnedValue},
};

const NAME: &str = "org.freedesktop.impl.portal.desktop.bgapp";
const CMDLINE: &str = r#"niri msg windows | grep -e '^ *App ID: ".*"$' | cut -d \" -f 2"#;

struct PortalInterface {
    app_id_set: Arc<RwLock<HashSet<String>>>
}

impl PortalInterface {
    fn new() -> Self {
        Self {
            app_id_set: Arc::new(RwLock::new(HashSet::new())),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Background", spawn = false)]
impl PortalInterface {
    #[zbus(out_args("apps"))]
    fn get_app_state(&self) -> Result<HashMap<String, OwnedValue>> {
        println!("get_app_state called");
        let mut app_states = HashMap::new();
        for app_id in &*self.app_id_set.read().unwrap() {
            println!("    {}", app_id);
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
    ) -> Result<(u32, HashMap<String, OwnedValue>)> {
        println!("notify_background called");
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
    ) -> Result<bool> {
        Ok(false)
    }

    #[zbus(signal)]
    async fn running_applications_changed(signal_ctxt: &SignalEmitter<'_>) -> zbus::Result<()>;
}

struct StateUpdater {
    app_id_set: Arc<RwLock<HashSet<String>>>
}

impl StateUpdater {
    fn new(portal_interface: &PortalInterface) -> Self {
        Self {
            app_id_set: portal_interface.app_id_set.clone(),
        }
    }

    async fn update_app_states(&mut self) -> std::result::Result<(), Error> {
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(CMDLINE)
            .output()
            .or(Err(Error::Failed("Failed to execute command".into())))?;
        let app_id_list = String::from_utf8(output.stdout)
            .or(Err(Error::Failed("Failed to parse command output".into())))?;
        let mut app_id_set = self.app_id_set.write().unwrap();
        app_id_set.clear();
        for app_id in app_id_list.lines() {
            app_id_set.insert(app_id.to_string());
        }
        Ok(())
    }
}


async fn run_portal() -> std::result::Result<(), Error> {
    let portal_interface = PortalInterface::new();
    let mut state_updater = StateUpdater::new(&portal_interface);
    state_updater.update_app_states().await?;
    let connection = zbus::Connection::session().await?;
    connection
        .object_server()
        .at("/org/freedesktop/portal/desktop", portal_interface)
        .await?;
    connection.request_name(NAME).await?;

    tokio_scoped::scope(|scope| {
        scope.spawn(async {
            loop {
                sleep(Duration::from_millis(1000)).await;
                state_updater.update_app_states().await.unwrap();
            }
        });
    });
    Ok(())
}

fn main() -> zbus::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;

    rt.block_on(run_portal())?;

    Ok(())
}
