//! Tokens that let a program we start take focus. wayapp doesn't speak this
//! protocol, so it gets its own event queue on the same connection. KWin hands
//! one out while our surface is the active window: ask before hiding.

use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::EventQueue;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::XdgActivationTokenV1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

#[derive(Default)]
struct State {
    manager: Option<XdgActivationV1>,
    token: Option<String>,
}

pub struct Activation {
    queue: EventQueue<State>,
    state: State,
}

impl Activation {
    pub fn new(conn: &Connection) -> Self {
        let mut queue = conn.new_event_queue();
        let mut state = State::default();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        if let Err(e) = queue.roundtrip(&mut state) {
            log::warn!("no activation tokens: {e}");
        }
        Self { queue, state }
    }

    pub fn token(&mut self, surface: &WlSurface) -> Option<String> {
        let manager = self.state.manager.as_ref()?;
        let token = manager.get_activation_token(&self.queue.handle(), ());
        token.set_surface(surface);
        token.commit();
        self.state.token = None;
        while self.state.token.is_none() {
            if let Err(e) = self.queue.blocking_dispatch(&mut self.state) {
                log::warn!("no activation token: {e}");
                break;
            }
        }
        token.destroy();
        self.state.token.take()
    }
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
        if let wl_registry::Event::Global { name, interface, .. } = event
            && interface == "xdg_activation_v1"
        {
            state.manager = Some(registry.bind(name, 1, queue, ()));
        }
    }
}

impl Dispatch<XdgActivationV1, ()> for State {
    fn event(_: &mut Self, _: &XdgActivationV1, _: <XdgActivationV1 as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<XdgActivationTokenV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            state.token = Some(token);
        }
    }
}
