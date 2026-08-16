use confique::Config;
use futures_util::stream::StreamExt;
use json_escape::escape_str;
use log::{debug, info, warn};
use std::{collections::HashMap, path::PathBuf, time::SystemTime};
use tokio::{
    fs::File,
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, Command},
    spawn,
    time::{Duration, interval, sleep},
};

// DBus services we call

#[zbus::proxy(
    interface = "org.freedesktop.background.Monitor",
    default_path = "/org/freedesktop/background/monitor",
    default_service = "org.freedesktop.background.Monitor"
)]
trait BackgroundMonitor {
    #[zbus(property)]
    fn background_apps(&self) -> zbus::Result<Vec<HashMap<String, String>>>;
}

#[zbus::proxy(
    interface = "org.kde.StatusNotifierWatcher",
    default_path = "/StatusNotifierWatcher",
    default_service = "org.kde.StatusNotifierWatcher"
)]
trait StatusNotifierWatcher {
    /// StatusNotifierItemRegistered signal
    #[zbus(signal)]
    fn status_notifier_item_registered(&self, arg_1: &str) -> zbus::Result<()>;

    /// StatusNotifierItemUnregistered signal
    #[zbus(signal)]
    fn status_notifier_item_unregistered(&self, arg_1: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> zbus::Result<Vec<String>>;
}

#[zbus::proxy(interface = "org.freedesktop.DBus", assume_defaults = true)]
trait DBus {
    #[zbus(name = "GetConnectionUnixProcessID")]
    fn get_connection_unix_process_id(&self, bus_name: &str) -> zbus::Result<u32>;
}

// Main program

#[derive(Config, Clone, Debug)]
struct GlobalConfig {
    #[config(default = 5000)]
    pub update_interval_ms: u32,

    #[config(default = 15000)]
    pub grace_period_ms: u32,

    #[config(default = 1)]
    pub tray_icon_status: u32,
}

type AppId = String;

// Apps can change the message via org.freedesktop.portal.Background.SetStatus.
// The message is displayed in the system tray tooltip.
struct AppState {
    message: String,
    helper: Child,
}

struct BackgroundAppManager {
    config: GlobalConfig,
    apps: HashMap<AppId, AppState>,
    time_removed: HashMap<AppId, SystemTime>,
    bus_addr_to_instance_id: HashMap<String, u32>,
    instance_id_to_bus_addr: HashMap<u32, String>,
}

impl BackgroundAppManager {
    fn new(config: GlobalConfig) -> Self {
        Self {
            config,
            apps: HashMap::new(),
            time_removed: HashMap::new(),
            bus_addr_to_instance_id: HashMap::new(),
            instance_id_to_bus_addr: HashMap::new(),
        }
    }

    // Add/update/remove tray icons based on the latest list from the Background Portal Monitor service.
    async fn update_apps(&mut self, app_list_from_monitor: Vec<HashMap<String, String>>) {
        let now = SystemTime::now();

        // Helpers quit proactively when the user Activates or Quits the app from it,
        // because the app should no longer be in background. Clean up any finished helpers.
        self.apps.retain(|id, app| {
            if let Ok(None) = app.helper.try_wait() {
                true
            } else {
                info!("helper exited: {id}",);
                self.time_removed.insert(id.to_owned(), now);
                false
            }
        });

        let mut apps_new: HashMap<AppId, String> = HashMap::new();
        for map in app_list_from_monitor {
            if let (Some(app_id), Some(instance)) = (map.get("app_id"), map.get("instance"))
                && let Ok(instance_id) = instance.parse::<u32>()
            {
                if let Some(removed_time) = self.time_removed.get(app_id)
                    && now
                        .duration_since(*removed_time)
                        .unwrap_or(Duration::from_millis(0))
                        < Duration::from_millis(self.config.grace_period_ms.into())
                {
                    debug!("skipping recently removed: {app_id}",);
                    continue;
                }
                if self.has_flatpak_icon(instance_id) {
                    continue;
                }
                // If there are multiple instances of the same app_id, we keep an arbitrary one.
                // This seems to happen when an Flatpak app (e.g. Amberol) goes background for the first time.
                // `flatpak ps` reports two instances in this case. So does the Backgound Portal Monitor.
                // Because we do both Activate and Quit by talking to the bus address of the app_id,
                // it's meaningless to have multiple icons for the same app_id, as they will all talk to
                // the same instance (we don't know which one it is).
                apps_new.insert(
                    app_id.to_owned(),
                    map.get("message").unwrap_or(&"".to_string()).to_owned(),
                );
            }
        }

        // Remove icons for apps that are gone (became foreground or exited).
        let mut to_delete: Vec<AppId> = Vec::new();
        for id in self.apps.keys() {
            if !apps_new.contains_key(id) {
                to_delete.push(id.to_owned());
            }
        }
        for id in to_delete {
            if let Some(mut state) = self.apps.remove(&id) {
                info!("DEL {id}");
                self.time_removed.insert(id.to_owned(), now);
                spawn(async move {
                    // Close stdin to signal the helper to exit gracefully.
                    let _ = state.helper.stdin.as_mut().unwrap().shutdown().await;
                    sleep(Duration::from_secs(1)).await;
                    if let Ok(None) = state.helper.try_wait() {
                        state.helper.kill().await.ok();
                    }
                });
            }
        }

        // Add icons for new background apps.
        for (app_id, message) in &apps_new {
            if !self.apps.contains_key(app_id) {
                info!("ADD {app_id}");
                // Find the helper in the same dir. Else fallback to PATH.
                let mut executable = std::env::current_exe().unwrap();
                executable.set_file_name("bgapp2tray-helper");
                if !executable.exists() {
                    executable = std::path::PathBuf::from("bgapp2tray-helper");
                }
                let helper = Command::new(executable)
                    .arg(app_id)
                    .arg(message)
                    .arg(self.config.tray_icon_status.to_string())
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                    .unwrap();
                self.apps.insert(
                    app_id.to_owned(),
                    AppState {
                        message: message.clone(),
                        helper,
                    },
                );
            } else {
                // Update the message for existing background apps if it has changed.
                let app = self.apps.get_mut(app_id).unwrap();
                if app.message != *message {
                    info!("SET-MESSAGE {app_id} message: {message}");
                    app.message = message.to_owned();
                    if let Some(stdin) = &mut app.helper.stdin {
                        let _ = stdin
                            .write_all(format!("SET-MESSAGE {}\n", escape_str(message)).as_bytes())
                            .await;
                    }
                }
            }
        }
    }

    // We want to know which Flatpak apps have tray icons (not created by us),
    // so we don't create a duplicate tray icon for them.
    // The list of tray icon DBus addresses is from the StatusNotifierWatcher service.
    async fn update_flatpak_tray_icon_list(
        &mut self,
        icon_bus_addr_list: Vec<String>,
        get_pid: impl AsyncFn(&str) -> Result<u32, zbus::Error>,
    ) -> Result<(), zbus::Error> {
        let addr_set: std::collections::HashSet<_> = icon_bus_addr_list.iter().cloned().collect();

        // Remove disappeared icons.
        self.bus_addr_to_instance_id
            .retain(|addr, _| addr_set.contains(addr));
        self.instance_id_to_bus_addr
            .retain(|_, addr| addr_set.contains(addr));

        for bus_addr in icon_bus_addr_list {
            if self.bus_addr_to_instance_id.contains_key(&bus_addr) {
                continue;
            }
            // New icon. Try to get its pid, and check if it's a Flatpak app.
            if let Some((bus_name, _path)) = bus_addr.split_once('/') {
                let pid = get_pid(bus_name).await?;
                let flatpak_info_path = format!("/proc/{pid}/root/.flatpak-info");
                if let Ok(file) = File::open(&flatpak_info_path).await {
                    debug!("flatpak_info found at {}", flatpak_info_path);
                    let mut lines = BufReader::new(file).lines();
                    while let Some(line) = lines.next_line().await? {
                        if line.starts_with("instance-id=") {
                            if let Ok(flatpak_id) =
                                line.trim_start_matches("instance-id=").parse::<u32>()
                            {
                                debug!("Parsed instance-id: {}", flatpak_id);
                                self.bus_addr_to_instance_id
                                    .insert(bus_addr.clone(), flatpak_id);
                                self.instance_id_to_bus_addr
                                    .insert(flatpak_id, bus_addr.clone());
                            } else {
                                warn!("Failed to parse instance-id from line: {}", line);
                            }
                            break;
                        }
                    }
                }
            } else {
                warn!("Invalid D-Bus address for tray icon: {}", bus_addr);
            }
        }
        Ok(())
    }

    fn has_flatpak_icon(&self, instance_id: u32) -> bool {
        self.instance_id_to_bus_addr.contains_key(&instance_id)
    }

    async fn run(&mut self) -> Result<(), zbus::Error> {
        let bus = zbus::Connection::session().await?;

        let background_monitor_proxy = BackgroundMonitorProxy::new(&bus).await?;
        let mut background_apps_stream = background_monitor_proxy
            .receive_background_apps_changed()
            .await;

        let status_notifier_watcher_proxy = StatusNotifierWatcherProxy::new(&bus).await?;
        let mut registered_items_stream = status_notifier_watcher_proxy
            .receive_registered_status_notifier_items_changed()
            .await;

        let dbus = DBusProxy::new(&bus).await?;

        // Initial update of the tray icon list, before any update_apps().
        let addr_list = status_notifier_watcher_proxy
            .registered_status_notifier_items()
            .await?;
        self.update_flatpak_tray_icon_list(
            addr_list,
            async |bus_name| -> Result<u32, zbus::Error> {
                dbus.get_connection_unix_process_id(bus_name).await
            },
        )
        .await?;

        let mut timer = interval(Duration::from_millis(self.config.update_interval_ms.into()));
        loop {
            tokio::select! {
                Some(v) = background_apps_stream.next() => {
                    debug!("BackgroundApps change signal {:?}", v.get().await?);
                    let background_apps = background_monitor_proxy.background_apps().await?;
                    self.update_apps(background_apps).await;
                },
                Some(_) = registered_items_stream.next() => {
                    debug!("RegisteredStatusNotifierItems change signal");
                    let addr_list = status_notifier_watcher_proxy
                        .registered_status_notifier_items()
                        .await?;
                    self.update_flatpak_tray_icon_list(addr_list, async |bus_name| -> Result<u32, zbus::Error> {
                        dbus.get_connection_unix_process_id(bus_name).await
                    })
                    .await?;
                },
                _ = timer.tick() => {
                    // Maybe we can update only on the change signals, but we also do it periodically to be safe.
                    let addr_list = status_notifier_watcher_proxy
                        .registered_status_notifier_items()
                        .await?;
                    self.update_flatpak_tray_icon_list(addr_list, async |bus_name| -> Result<u32, zbus::Error> {
                        dbus.get_connection_unix_process_id(bus_name).await
                    })
                    .await?;
                    let background_apps = background_monitor_proxy.background_apps().await?;
                    self.update_apps(background_apps).await;
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let config_file = "bgapp2tray.toml";
    let config_path = xdg::BaseDirectories::new().find_config_file(config_file);
    info!("Using config file: {:?}", config_path);
    let config =
        GlobalConfig::from_file(config_path.unwrap_or(PathBuf::from(config_file.to_string())))?;
    info!("    update-interval-ms: {}", config.update_interval_ms);
    info!("    grace-period-ms: {}", config.grace_period_ms);
    info!("    tray-icon-status: {}", config.tray_icon_status);

    let mut app = BackgroundAppManager::new(config);
    app.run().await?;
    Ok(())
}
