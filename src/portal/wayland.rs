use log::{debug, info};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::fd::OwnedFd,
    sync::{Arc, Mutex, mpsc},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle,
    backend::{Backend, ObjectData, ObjectId, protocol::Message},
    event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::wl_registry,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};

struct Toplevel {
    app_id: Mutex<RefCell<String>>,
}

impl Toplevel {
    fn new() -> Self {
        Self {
            app_id: Mutex::new(RefCell::new(String::new())),
        }
    }
}

impl ObjectData for Toplevel {
    fn event(
        self: Arc<Self>,
        _backend: &Backend,
        _msg: Message<ObjectId, OwnedFd>,
    ) -> Option<Arc<dyn ObjectData>> {
        None
    }

    fn destroyed(&self, _object_id: ObjectId) {}
}

struct State {
    toplevels: Mutex<HashMap<String, i32>>, // appid -> count
    tx: mpsc::Sender<HashSet<String>>,
}

impl State {
    fn new(tx: mpsc::Sender<HashSet<String>>) -> Self {
        Self {
            toplevels: Mutex::new(HashMap::new()),
            tx,
        }
    }

    fn notify_change(&mut self) {
        let app_ids;
        {
            let mut toplevels = self.toplevels.lock().unwrap();
            toplevels.retain(|_k, v| *v > 0);
            app_ids = toplevels.keys().cloned().collect::<HashSet<String>>();
            debug!("Toplevels updated: {:?}", toplevels);
        }
        self.tx.send(app_ids).unwrap();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

#[allow(non_snake_case, unused_variables)]
impl Dispatch<ExtForeignToplevelListV1, ()> for State {
    fn event(
        _state: &mut Self,
        _proxy: &ExtForeignToplevelListV1,
        event: ext_foreign_toplevel_list_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } => {
                debug!("Toplevel event received");
            }
            ext_foreign_toplevel_list_v1::Event::Finished => {
                debug!("Finished event received");
            }
            _ => {}
        }
    }

    event_created_child!(State, ExtForeignToplevelListV1, [
        0 => (ExtForeignToplevelHandleV1, Toplevel::new()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, Toplevel> for State {
    fn event(
        state: &mut Self,
        _proxy: &ExtForeignToplevelHandleV1,
        event: ext_foreign_toplevel_handle_v1::Event,
        data: &Toplevel,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_foreign_toplevel_handle_v1::Event::Closed => {
                {
                    let guard = data.app_id.lock().unwrap();
                    let app_id = guard.borrow();
                    debug!("Toplevel handle closed : {}", app_id);
                    let mut toplevels = state.toplevels.lock().unwrap();
                    toplevels.entry(app_id.clone()).and_modify(|count| {
                        *count -= 1;
                    });
                }
                state.notify_change();
            }
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                {
                    let old_id = data.app_id.lock().unwrap();
                    let mut toplevels = state.toplevels.lock().unwrap();
                    if old_id.borrow().is_empty() {
                        debug!("Toplevel handle app_id set: {app_id}");
                    } else {
                        debug!(
                            "Toplevel handle app_id changed from {} to {app_id}",
                            old_id.borrow()
                        );
                        toplevels
                            .entry(old_id.borrow().to_owned())
                            .and_modify(|count| {
                                *count -= 1;
                            });
                    }
                    old_id.replace(app_id);
                    toplevels
                        .entry(old_id.borrow().to_owned())
                        .and_modify(|count| {
                            *count += 1;
                        })
                        .or_insert(1);
                }
                state.notify_change();
            }
            _ => {}
        }
    }
}

pub fn connect() -> Result<mpsc::Receiver<HashSet<String>>, Box<dyn std::error::Error>> {
    let connect = Connection::connect_to_env()?;
    info!("Connected to Wayland compositor");
    let (globals, mut event_queue) = registry_queue_init::<State>(&connect)?;

    let (tx, rx) = mpsc::channel();
    let mut state = State::new(tx);
    // Perform an initial roundtrip to ensure all globals are received
    event_queue.roundtrip(&mut state)?;

    let qh = event_queue.handle();
    let _manager = globals.bind::<ExtForeignToplevelListV1, _, _>(&qh, 1..=1, ())?;
    info!("Compositor supports ext-foreign-toplevel-list-v1");

    std::thread::spawn(move || {
        loop {
            if let Err(e) = event_queue.blocking_dispatch(&mut state) {
                log::error!("Wayland dispatch stopped {e}");
                break;
            }
        }
        log::info!("Wayland event loop exited");
    });

    Ok(rx)
}
