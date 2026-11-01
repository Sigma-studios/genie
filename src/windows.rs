//! The windows that are open, from KWin, to bring one forward rather than start
//! a second copy of an app. Its own event queue, like activation.rs: wayapp
//! doesn't speak this protocol.

use std::collections::HashMap;
use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::EventQueue;
use wayland_client::Proxy;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_registry;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::OrgKdePlasmaWindow;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::OrgKdePlasmaWindowManagement;

const ACTIVE: u32 = 0x1;
const MINIMIZED: u32 = 0x2;
const SKIP_TASKBAR: u32 = 0x1000;

struct Open {
    window: OrgKdePlasmaWindow,
    app_id: String,
    skip_taskbar: bool,
    /// When it was last active, to pick the most recent of an app's windows.
    last_active: u64,
}

#[derive(Default)]
struct State {
    manager: Option<OrgKdePlasmaWindowManagement>,
    windows: HashMap<String, Open>,
    clock: u64,
}

pub struct Windows {
    queue: EventQueue<State>,
    state: State,
}

impl Windows {
    pub fn new(conn: &Connection) -> Self {
        let mut queue = conn.new_event_queue();
        let mut state = State::default();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        // One for the global, another for the windows it lists.
        for _ in 0..2 {
            if let Err(e) = queue.roundtrip(&mut state) {
                log::warn!("no list of windows: {e}");
            }
        }
        if state.manager.is_none() {
            log::warn!("the compositor doesn't list windows: apps always start anew");
        }
        Self { queue, state }
    }

    /// Brings forward the latest window of the app with one of these names.
    /// `false` if it has none, or if we can't tell: then it should just start.
    pub fn focus(&mut self, names: &[String]) -> bool {
        // wayapp's thread reads the connection: this just handles what it read.
        let _ = self.queue.dispatch_pending(&mut self.state);
        let found = self
            .state
            .windows
            .values()
            .filter(|open| !open.skip_taskbar && matches(&open.app_id, names))
            .max_by_key(|open| open.last_active);
        let Some(open) = found else {
            log::debug!("no window for {names:?}");
            return false;
        };
        log::debug!("focusing {}", open.app_id);
        open.window.set_state(ACTIVE | MINIMIZED, ACTIVE);
        true
    }
}

fn matches(app_id: &str, names: &[String]) -> bool {
    !app_id.is_empty() && names.iter().any(|name| name.eq_ignore_ascii_case(app_id))
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global { name, interface, version } = event
            && interface == OrgKdePlasmaWindowManagement::interface().name
        {
            let version = version.min(OrgKdePlasmaWindowManagement::interface().version);
            state.manager = Some(registry.bind(name, version, queue, ()));
        }
    }
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for State {
    fn event(
        state: &mut Self,
        manager: &OrgKdePlasmaWindowManagement,
        event: org_kde_plasma_window_management::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let org_kde_plasma_window_management::Event::WindowWithUuid { uuid, .. } = event {
            let window = manager.get_window_by_uuid(uuid.clone(), queue, uuid.clone());
            state.windows.insert(uuid, Open { window, app_id: String::new(), skip_taskbar: false, last_active: 0 });
        }
    }
}

impl Dispatch<OrgKdePlasmaWindow, String> for State {
    fn event(
        state: &mut Self,
        window: &OrgKdePlasmaWindow,
        event: org_kde_plasma_window::Event,
        uuid: &String,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            org_kde_plasma_window::Event::AppIdChanged { app_id } => {
                if let Some(open) = state.windows.get_mut(uuid) {
                    open.app_id = app_id;
                }
            }
            org_kde_plasma_window::Event::StateChanged { flags } => {
                state.clock += 1;
                if let Some(open) = state.windows.get_mut(uuid) {
                    open.skip_taskbar = flags & SKIP_TASKBAR != 0;
                    if flags & ACTIVE != 0 {
                        open.last_active = state.clock;
                    }
                }
            }
            org_kde_plasma_window::Event::Unmapped => {
                state.windows.remove(uuid);
                window.destroy();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_exactly_whatever_the_case() {
        let names = ["vivaldi-stable".to_string(), "crx_abc".to_string()];
        assert!(matches("Vivaldi-Stable", &names));
        assert!(matches("crx_abc", &names));
        assert!(!matches("vivaldi", &names));
        assert!(!matches("crx_other", &names));
        assert!(!matches("", &["".to_string()]));
    }
}
