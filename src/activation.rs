//! Tokens that let a program we start take focus. wayapp doesn't speak this
//! protocol, so it gets its own event queue on the same connection. KWin hands
//! one out while our surface is the active window, so ask before hiding. It only
//! honours one made with a serial at least as new as the last user input, and
//! wayapp drops serials, so this also listens to the keyboard and pointer.

use wayland_client::Connection;
use wayland_client::Dispatch;
use wayland_client::EventQueue;
use wayland_client::QueueHandle;
use wayland_client::protocol::wl_keyboard;
use wayland_client::protocol::wl_keyboard::WlKeyboard;
use wayland_client::protocol::wl_pointer;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_token_v1::XdgActivationTokenV1;
use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

#[derive(Default)]
struct State {
    manager: Option<XdgActivationV1>,
    seat: Option<WlSeat>,
    keyboard: Option<WlKeyboard>,
    pointer: Option<WlPointer>,
    /// From the latest input on our surface.
    serial: Option<u32>,
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
        // wayapp's thread reads the connection: this just handles what it read.
        let _ = self.queue.dispatch_pending(&mut self.state);
        let manager = self.state.manager.as_ref()?;
        let token = manager.get_activation_token(&self.queue.handle(), ());
        token.set_surface(surface);
        if let (Some(serial), Some(seat)) = (self.state.serial, &self.state.seat) {
            token.set_serial(serial, seat);
        }
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
        let wl_registry::Event::Global { name, interface, version } = event else { return };
        match interface.as_str() {
            "xdg_activation_v1" => state.manager = Some(registry.bind(name, 1, queue, ())),
            "wl_seat" if state.seat.is_none() => state.seat = Some(registry.bind(name, version.min(5), queue, ())),
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, queue: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: wayland_client::WEnum::Value(capabilities) } = event {
            if capabilities.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(queue, ()));
            }
            if capabilities.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(queue, ()));
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(state: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Enter { serial, .. } | wl_keyboard::Event::Key { serial, .. } => state.serial = Some(serial),
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { serial, .. } = event {
            state.serial = Some(serial);
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
