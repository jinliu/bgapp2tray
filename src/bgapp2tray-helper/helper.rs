use freedesktop_desktop_entry::{desktop_entries, find_app_by_id, get_languages_from_env};
use json_escape::unescape;
use libappindicator_zbus::{
    tray,
    utils::{ButtonOptions, Category, EventUpdate, MenuTree, MenuUnit, NotifierStatus, ToolTip},
};
use log::{debug, error, info, warn};
use std::{cell::OnceCell, collections::HashMap, sync::Mutex};
use tokio::{
    io::{AsyncBufReadExt, BufReader, stdin},
    process::Command,
};
use unicase::Ascii;
use zbus::{
    fdo,
    names::BusName,
    zvariant::{ObjectPath, OwnedValue},
};

#[zbus::proxy(interface = "org.freedesktop.Application")]
trait FdoApplication {
    async fn Activate(&self, platform_data: HashMap<String, OwnedValue>) -> fdo::Result<()>;

    async fn ActivateAction(
        &self,
        action_name: &str,
        parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) -> fdo::Result<()>;
}

#[derive(Debug)]
struct Config {
    app_id: String,
    name: String,
    icon_name: String,
    message: String,
    status: NotifierStatus,
}

static CONFIG: Mutex<OnceCell<Config>> = Mutex::new(OnceCell::new());

struct Base {}

impl Base {
    fn new() -> Self {
        Self {}
    }

    fn id() -> String {
        CONFIG.lock().unwrap().get().unwrap().app_id.to_owned()
    }

    fn title(&self) -> fdo::Result<String> {
        Ok(CONFIG.lock().unwrap().get().unwrap().name.to_owned())
    }

    fn icon_name(&self) -> fdo::Result<String> {
        Ok(CONFIG.lock().unwrap().get().unwrap().icon_name.to_owned())
    }

    fn on_click(&mut self, _x: i32, _y: i32) -> fdo::Result<()> {
        info!("Icon clicked");
        Self::activate();
        Ok(())
    }

    #[allow(dead_code)]
    fn status(&self) -> NotifierStatus {
        CONFIG.lock().unwrap().get().unwrap().status.to_owned()
    }

    fn tool_tip(&self) -> fdo::Result<ToolTip> {
        Ok(ToolTip {
            icon: self.icon_name()?,
            data: Vec::new(),
            title: self.title()?,
            description: CONFIG.lock().unwrap().get().unwrap().message.to_owned(),
        })
    }

    fn activate() {
        tokio::spawn(async { Self::invoke_action(Action::Activate).await });
    }

    fn quit() {
        tokio::spawn(async { Self::invoke_action(Action::Quit).await });
    }

    async fn invoke_action(action: Action) -> fdo::Result<()> {
        let app_id;
        let service_name;
        let service_path;
        {
            let guard = CONFIG.lock().unwrap();
            let config = guard.get().unwrap();
            app_id = config.app_id.to_owned();
            service_name = app_id.to_owned();
            service_path = "/".to_string() + service_name.replace('.', "/").as_str();
        }
        debug!("Invoking action: {action:?} on service: {service_name} at path: {service_path}");
        let bus = zbus::Connection::session().await?;
        let proxy = FdoApplicationProxy::new(
            &bus,
            BusName::try_from(service_name.as_str()).unwrap(),
            ObjectPath::try_from(service_path.as_str()).unwrap(),
        )
        .await?;
        match action {
            Action::Activate => {
                if let Err(e) = proxy.Activate(HashMap::new()).await {
                    warn!("Failed to activate: {e:?}");
                } else {
                    std::process::exit(0);
                }
            }
            Action::Quit => {
                if let Err(e) = proxy.ActivateAction("quit", vec![], HashMap::new()).await {
                    warn!("Failed to invoke quit action: {e:?}; try `flatpak kill`");
                    let mut child = Command::new("flatpak")
                        .arg("kill")
                        .arg(app_id)
                        .spawn()
                        .expect("failed to spawn flatpak kill command");
                    let status = child.wait().await.unwrap();
                    info!("the command exited with: {status}");
                }
                std::process::exit(0);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
enum Action {
    Activate,
    Quit,
}

struct Menu {
    revision: u32,
}

impl Menu {
    fn new() -> Self {
        Menu { revision: 0 }
    }

    fn revision(&self) -> u32 {
        self.revision
    }

    fn menu() -> MenuTree<Action> {
        MenuTree::new()
            .push_sub_menu(MenuUnit::button(
                ButtonOptions {
                    label: "Activate".to_owned(),
                    icon_name: "open-link".to_owned(),
                    enabled: true,
                },
                Action::Activate,
            ))
            .push_sub_menu(MenuUnit::button(
                ButtonOptions {
                    label: "Quit".to_owned(),
                    icon_name: "application-exit".to_owned(),
                    enabled: true,
                },
                Action::Quit,
            ))
    }

    fn on_click(
        &mut self,
        _button: &mut MenuUnit<Action>,
        forward_message: Action,
        _timestamp: u32,
    ) -> EventUpdate {
        info!("Menu clicked: {forward_message:?}");
        match forward_message {
            Action::Activate => Base::activate(),
            Action::Quit => Base::quit(),
        }
        EventUpdate::None
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args: Vec<String> = std::env::args().collect();
    debug!("Args: {args:?}");
    if args.len() != 4 {
        error!("Usage: bgapp2tray <app-id> <message> <status 0:passive, 1:active>");
        std::process::exit(1);
    }
    let app_id = args[1].to_owned();
    let message = args[2].to_owned();
    let status = match args[3].as_str() {
        "0" => NotifierStatus::Passive,
        "1" => NotifierStatus::Active,
        _ => {
            error!("Invalid status: {}", args[3]);
            std::process::exit(1);
        }
    };

    let mut name = app_id.split('.').next_back().unwrap().to_owned();
    let mut icon_name = app_id.to_owned();

    // Try to get the name and icon name from .desktop file.
    let languages = get_languages_from_env();
    let languages = languages
        .iter()
        .map(|s| s.split('.').next().unwrap_or(s).to_string())
        .collect::<Vec<String>>();
    debug!("Languages from env: {:?}", languages);
    let desktop_entries = desktop_entries(&languages);
    if let Some(desktop_entry) = find_app_by_id(&desktop_entries, Ascii::new(&app_id)) {
        info!("Found desktop entry for app_id: {}", app_id);
        if let Some(entry) = desktop_entry.groups.desktop_entry() {
            if let Some(entry_name) = entry.localized_entry("Name", &languages) {
                name = entry_name.to_owned();
                info!("Found localized name in desktop entry: {}", name);
            } else if let Some(entry_name) = entry.entry("Name") {
                name = entry_name.to_owned();
                info!("Found name in desktop entry: {}", name);
            }
            if let Some(entry_icon_name) = entry.entry("Icon") {
                icon_name = entry_icon_name.to_owned();
                info!("Found icon name in desktop entry: {}", icon_name);
            }
        }
    }

    {
        CONFIG.lock().unwrap().get_or_init(|| Config {
            app_id,
            name,
            icon_name,
            message,
            status,
        });
    }

    let connection = tray(
        Base::new,
        Base::id,
        Base::title,
        Menu::new,
        Menu::menu,
        Menu::revision,
    )
    .with_icon_name(Base::icon_name)
    .with_category(Category::ApplicationStatus)
    .with_on_clicked(Menu::on_click)
    .with_activate(Base::on_click)
    .with_tool_tip(Base::tool_tip)
    //.with_tray_status(Self::status) // doesn't build
    .run()
    .await?;
    info!(
        "StatusNotifierItem registered at {}",
        connection
            .unique_name()
            .ok_or("Failed to get unique name")?
    );

    let mut lines = BufReader::new(stdin()).lines();
    // Quit when parent closes stdin.
    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if let Some(("SET-MESSAGE", arg)) = line.split_once(' ') {
            CONFIG.lock().unwrap().get_mut().unwrap().message =
                unescape(arg).decode_utf8()?.to_string();
        } else {
            error!("Unknown command: {line}");
        }
    }

    Ok(())
}
