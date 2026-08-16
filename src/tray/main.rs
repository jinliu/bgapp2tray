use futures_util::stream::StreamExt;
use gtk::prelude::*;
use libayatana_appindicator::{AppIndicator, AppIndicatorStatus};

struct TrayIconProp {
    label: String,
    message: String,
    icon: String,
}

enum TrayIconOp {
    Add,
    Update,
    Remove,
}

struct TrayIconMessage {
    op: TrayIconOp,
    id: String,
    prop: Option<TrayIconProp>,
}

enum TrayIconAction {
    Activate,
    Quit,
}

struct TrayIconActionMessage {
    action: TrayIconAction,
    id: String,
}

#[zbus::proxy(
    interface = "org.freedesktop.background.Monitor",
    default_service = "org.freedesktop.background.Monitor",
    default_path = "/org/freedesktop/background/monitor"
)]
trait BackgroundMonitor {
    #[zbus(property)]
    fn background_apps(&self) -> zbus::Result<Vec<String>>;
}

fn update_apps(background_apps: Vec<String>) {
    for app in background_apps {
        println!("Background app: {}", app);
    }
}

async fn monitor_background_apps(
    icon_tx: std::sync::mpsc::Sender<TrayIconMessage>,
    action_rx: std::sync::mpsc::Receiver<TrayIconActionMessage>,
) -> Result<(), zbus::Error> {
    let bus = zbus::Connection::session().await?;
    let proxy = BackgroundMonitorProxy::new(&bus).await?;
    let background_apps = proxy.background_apps().await?;
    update_apps(background_apps);

    let mut background_apps_stream = proxy.receive_background_apps_changed().await;

    loop {
        tokio::select! {
            Some(v) = background_apps_stream.next() => {
                let background_apps = proxy.background_apps().await?;
                update_apps(background_apps);
            },
        }
    }
}

fn manage_trayicon(
    icon_rx: std::sync::mpsc::Receiver<TrayIconMessage>,
    action_tx: std::sync::mpsc::Sender<TrayIconActionMessage>,
) {
    gtk::init().unwrap();
    let mut indicator = AppIndicator::new("libayatana-appindicator test application", "");
    indicator.set_status(AppIndicatorStatus::Active);
    let mut m = gtk::Menu::new();
    let mi = gtk::CheckMenuItem::with_label("Hello RUST");
    mi.connect_activate(|_| {
        gtk::main_quit();
    });
    m.append(&mi);
    indicator.set_menu(&mut m);
    m.show_all();

    gtk::main();
}

fn main() {
    let (icon_tx, icon_rx) = std::sync::mpsc::channel::<TrayIconMessage>();
    let (action_tx, action_rx) = std::sync::mpsc::channel::<TrayIconActionMessage>();
    let threaded_rt = tokio::runtime::Runtime::new().unwrap();
    threaded_rt.spawn(async move {
        monitor_background_apps(icon_tx, action_rx).await.unwrap();
    });
    manage_trayicon(icon_rx, action_tx);
}
