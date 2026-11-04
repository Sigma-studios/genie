//! Génie, a launcher that stays resident and shows on a socket command.
//! `genie` runs it, `genie toggle`, `genie show [text]`, `genie hide`,
//! `genie activate` and `genie quit` talk to it, starting it first if need be.

mod activation;
mod command_list;
mod config;
mod gpu;
mod history;
mod icons;
mod launch;
mod launcher;
mod matcher;
mod plugin;
mod plugins;
mod windows;

use activation::Activation;
use command_list::CommandList;
use command_list::Outcome;
use gpu::Gpu;
use gpu::Target;
use launcher::Launcher;
use launcher::Request;
use plugin::Effect;
use plugin::Plugin;
use smithay_client_toolkit::compositor::Region;
use smithay_clipboard::Clipboard;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity;
use smithay_client_toolkit::shell::wlr_layer::Layer;
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::xdg::window::Window;
use smithay_client_toolkit::shell::xdg::window::WindowDecorations;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::net::UnixListener;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::Command as Process;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;
use std::sync::mpsc::Sender;
use std::time::Duration;
use std::time::Instant;
use wayapp::Application;
use wayapp::DispatchToken;
use wayapp::WaylandEvent;
use wayapp::WaylandToEguiInput;
use wayland_client::Proxy;

/// A toggle this soon after hiding on focus loss is the same gesture: pressing
/// the shortcut while showing can take the focus away first.
const TOGGLE_GUARD: Duration = Duration::from_millis(150);

pub enum Event {
    Wayland(DispatchToken),
    Command(Command),
    Results { generation: u64, items: Vec<plugin::Item> },
    Finished { generation: u64 },
    Icon { window: icons::Window, icon: plugin::Icon, image: Option<egui::ColorImage> },
    /// Something wants drawing.
    Wake,
}

pub enum Command {
    Toggle,
    Show(Option<String>),
    Hide,
    /// Runs the selected result, as Enter would.
    Activate,
    Quit,
}

impl Command {
    fn parse(line: &str) -> Option<Self> {
        let (word, rest) = line.trim_end_matches('\n').split_once(' ').unwrap_or((line.trim_end_matches('\n'), ""));
        match word {
            "toggle" => Some(Self::Toggle),
            "show" => Some(Self::Show((!rest.is_empty()).then(|| rest.to_string()))),
            "hide" => Some(Self::Hide),
            "activate" => Some(Self::Activate),
            "quit" => Some(Self::Quit),
            _ => None,
        }
    }
}

fn socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    dir.join("genie.sock")
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("genie=info")).init();
    let mut arguments = std::env::args().skip(1);
    let Some(command) = arguments.next() else {
        daemon();
        return;
    };
    let rest: Vec<String> = arguments.collect();
    let line = match command.as_str() {
        "toggle" | "hide" | "activate" | "quit" => command.clone(),
        "show" if rest.is_empty() => "show".to_string(),
        "show" => format!("show {}", rest.join(" ")),
        _ => {
            eprintln!("usage: genie [toggle | show [text] | hide | activate | quit]");
            std::process::exit(2);
        }
    };
    if !send(&line) {
        if matches!(command.as_str(), "hide" | "activate" | "quit") {
            return;
        }
        start_daemon();
        let started = Instant::now();
        while !send(&line) {
            if started.elapsed() > Duration::from_secs(5) {
                eprintln!("genie: the daemon didn't start");
                std::process::exit(1);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn send(line: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else { return false };
    stream.write_all(format!("{line}\n").as_bytes()).is_ok()
}

fn start_daemon() {
    use std::os::unix::process::CommandExt;
    let Ok(program) = std::env::current_exe() else { return };
    let result = Process::new(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    if let Err(e) = result {
        eprintln!("genie: couldn't start the daemon: {e}");
    }
}

fn listen(events: Sender<Event>) {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        eprintln!("genie is already running");
        std::process::exit(0);
    }
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap_or_else(|e| {
        eprintln!("genie: can't listen on {}: {e}", path.display());
        std::process::exit(1);
    });
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok()
                && let Some(command) = Command::parse(&line)
            {
                let _ = events.send(Event::Command(command));
            }
        }
    });
}

/// What exists only while the launcher shows. Fields drop in order, and the
/// swapchain has to go before the surface it's made from.
struct Shown {
    target: Target,
    layer: LayerSurface,
    configured: bool,
    panel: Option<egui::Rect>,
    show_requested: Instant,
    first_frame: bool,
}

/// The command list's window, for as long as it's open. Fields drop in order:
/// the swapchain before the surface, as in Shown.
struct ListWindow {
    target: Target,
    window: Window,
    renderer: egui_wgpu::Renderer,
    ctx: egui::Context,
    input: WaylandToEguiInput,
    modifiers: egui::Modifiers,
    icons: icons::Icons,
    list: CommandList,
    size: (u32, u32),
    scale: i32,
    configured: bool,
    focused: bool,
}

struct Daemon {
    modifiers: egui::Modifiers,
    app: Application,
    gpu: Gpu,
    windows: windows::Windows,
    renderer: egui_wgpu::Renderer,
    list: Option<ListWindow>,
    events: Sender<Event>,
    activation: Activation,
    ctx: egui::Context,
    input: WaylandToEguiInput,
    launcher: Launcher,
    icons: icons::Icons,
    shown: Option<Shown>,
    /// The last scale we were told, to start the next surface sharp.
    scale: i32,
    hidden_on_focus_loss: Option<Instant>,
    repaint_at: Arc<Mutex<Option<Instant>>>,
    dirty: bool,
}

fn daemon() {
    let (events, incoming) = mpsc::channel();
    listen(events.clone());

    let config = config::load();
    let plugins: Vec<Arc<dyn Plugin>> = vec![
        Arc::new(plugins::apps::Apps::new(config.terminal.clone())),
        Arc::new(plugins::files::Files),
        Arc::new(plugins::spotify::Spotify::new(config.spotify)),
        Arc::new(plugins::zed::Zed::new(config.terminal.clone())),
        Arc::new(plugins::power::Power(config.power.clone())),
        Arc::new(plugins::processes::Processes),
        Arc::new(plugins::calc::Calc),
        Arc::new(plugins::custom::Custom(config::load_commands())),
        Arc::new(plugins::websearch::WebSearch(config.websearch.clone())),
    ];

    let wayland_events = events.clone();
    let mut app = Application::new(move |token| {
        let _ = wayland_events.send(Event::Wayland(token));
    });
    let probe = app.compositor_state.create_surface(&app.qh);
    let started = Instant::now();
    let gpu = Gpu::new(&app.conn, &probe);
    probe.destroy();
    log::info!("GPU ready in {:?}", started.elapsed());
    let activation = Activation::new(&app.conn);
    let windows = windows::Windows::new(&app.conn);
    // SAFETY: the connection outlives the clipboard: both last as long as the
    // daemon.
    let clipboard = unsafe { Clipboard::new(app.conn.display().id().as_ptr() as *mut _) };
    let input = WaylandToEguiInput::new(clipboard);
    app.run_dispatcher();

    let ctx = egui::Context::default();
    launcher::fonts(&ctx);
    let repaint_at = Arc::new(Mutex::new(None));
    connect_repaint(&ctx, &repaint_at, &events);
    let renderer = gpu.renderer();
    let mut daemon = Daemon {
        modifiers: egui::Modifiers::NONE,
        app,
        gpu,
        windows,
        renderer,
        list: None,
        events: events.clone(),
        activation,
        ctx,
        input,
        launcher: Launcher::new(plugin::Runner::new(plugins, events.clone()), history::History::open()),
        icons: icons::Icons::new(events.clone(), icons::Window::Launcher),
        shown: None,
        scale: 1,
        hidden_on_focus_loss: None,
        repaint_at,
        dirty: false,
    };
    log::info!("ready");
    daemon.run(&incoming);
}

/// wayapp keeps track of the modifiers but doesn't tell egui, which only knows
/// them from this event: without it, Alt never reads as held.
fn with_modifiers(mut raw: egui::RawInput, modifiers: egui::Modifiers) -> egui::RawInput {
    raw.events.insert(0, egui::Event::ModifiersChanged(modifiers));
    raw
}

fn to_egui(modifiers: &smithay_client_toolkit::seat::keyboard::Modifiers) -> egui::Modifiers {
    egui::Modifiers { alt: modifiers.alt, ctrl: modifiers.ctrl, shift: modifiers.shift, mac_cmd: false, command: modifiers.ctrl }
}

fn connect_repaint(ctx: &egui::Context, repaint_at: &Arc<Mutex<Option<Instant>>>, events: &Sender<Event>) {
    let (repaint_at, events) = (repaint_at.clone(), events.clone());
    ctx.set_request_repaint_callback(move |info| {
        let at = Instant::now() + info.delay;
        let mut scheduled = repaint_at.lock().unwrap();
        *scheduled = Some(scheduled.map_or(at, |current: Instant| current.min(at)));
        let _ = events.send(Event::Wake);
    });
}

impl Daemon {
    fn run(&mut self, incoming: &mpsc::Receiver<Event>) {
        loop {
            let timeout = self.next_repaint();
            let first = match timeout {
                Some(timeout) => match incoming.recv_timeout(timeout) {
                    Ok(event) => Some(event),
                    Err(mpsc::RecvTimeoutError::Timeout) => None,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                },
                None => match incoming.recv() {
                    Ok(event) => Some(event),
                    Err(_) => return,
                },
            };
            // All that's queued, then one frame: faster than one per event.
            for event in first.into_iter().chain(std::iter::from_fn(|| incoming.try_recv().ok())) {
                self.handle(event);
            }
            let due = self.repaint_at.lock().unwrap().is_some_and(|at| at <= Instant::now());
            if self.dirty || due {
                *self.repaint_at.lock().unwrap() = None;
                self.dirty = false;
                self.draw();
                self.draw_list();
            }
        }
    }

    /// How long to sleep. Nothing's scheduled while hidden.
    fn next_repaint(&self) -> Option<Duration> {
        if self.shown.is_none() && self.list.is_none() {
            return None;
        }
        let at = (*self.repaint_at.lock().unwrap())?;
        Some(at.saturating_duration_since(Instant::now()))
    }

    fn handle(&mut self, event: Event) {
        match event {
            Event::Wayland(token) => {
                for event in self.app.dispatch_pending(token) {
                    self.wayland(event);
                }
            }
            Event::Command(Command::Toggle) => {
                if self.shown.is_some() {
                    self.hide();
                } else if self.hidden_on_focus_loss.is_none_or(|at| at.elapsed() > TOGGLE_GUARD) {
                    self.show(None);
                }
            }
            Event::Command(Command::Show(text)) => self.show(text),
            Event::Command(Command::Hide) => self.hide(),
            Event::Command(Command::Activate) => {
                for request in self.launcher.activate_selected() {
                    self.request(request);
                }
            }
            Event::Command(Command::Quit) => {
                self.hide();
                let _ = std::fs::remove_file(socket_path());
                std::process::exit(0);
            }
            Event::Results { generation, items } => {
                self.launcher.results(generation, items);
                self.dirty = true;
            }
            Event::Finished { generation } => {
                self.launcher.finished(generation);
                self.dirty = true;
            }
            Event::Icon { window: icons::Window::Launcher, icon, image } => {
                self.icons.loaded(&self.ctx, icon, image);
                self.dirty = true;
            }
            Event::Icon { window: icons::Window::CommandList, icon, image } => {
                if let Some(list) = &mut self.list {
                    list.icons.loaded(&list.ctx, icon, image);
                    self.dirty = true;
                }
            }
            Event::Wake => self.dirty = true,
        }
    }

    fn show(&mut self, text: Option<String>) {
        if self.shown.is_none() {
            self.launcher.runner.refresh();
            let (width, height) = launcher::surface_size();
            let surface = self.app.compositor_state.create_surface(&self.app.qh);
            surface.set_buffer_scale(self.scale);
            let target = self.gpu.target(&self.app.conn, &surface);
            // No anchors: centred, on the screen KWin considers active.
            let layer = self.app.layer_shell.create_layer_surface(&self.app.qh, surface, Layer::Overlay, Some("genie"), None);
            layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
            layer.set_size(width, height);
            layer.set_exclusive_zone(-1);
            layer.commit();
            self.shown = Some(Shown {
                target,
                layer,
                configured: false,
                panel: None,
                show_requested: Instant::now(),
                first_frame: true,
            });
        }
        if let Some(text) = text {
            self.launcher.set_input(text);
        }
        let _ = self.app.conn.flush();
    }

    fn hide(&mut self) {
        // Swapchain, then surface: the order of Shown's fields.
        if self.shown.take().is_some() {
            self.launcher.reset();
            self.input.handle_keyboard_leave();
            let _ = self.app.conn.flush();
        }
    }

    fn wayland(&mut self, event: WaylandEvent) {
        let for_list = self.list.as_ref().is_some_and(|list| match event.get_wl_surface() {
            Some(surface) => surface.id() == list.window.wl_surface().id(),
            None => list.focused,
        });
        if for_list {
            return self.list_event(event);
        }
        let Some(shown) = &mut self.shown else { return };
        let surface = shown.layer.wl_surface().clone();
        if event.get_wl_surface().is_some_and(|s| s.id() != surface.id()) {
            return;
        }
        match event {
            WaylandEvent::LayerShellConfigure(..) => {
                shown.configured = true;
                self.dirty = true;
            }
            // The compositor's decision, like when the screen went away.
            WaylandEvent::LayerShellClosed(_) => self.hide(),
            WaylandEvent::ScaleFactorChanged(_, scale) => {
                surface.set_buffer_scale(scale);
                self.scale = scale.max(1);
                self.dirty = true;
            }
            WaylandEvent::KeyboardEnter(..) => {
                log::debug!("focus {:?} after the command", shown.show_requested.elapsed());
                self.input.handle_keyboard_enter();
                self.dirty = true;
            }
            WaylandEvent::KeyboardLeave(_) => {
                log::debug!("focus lost, hiding");
                self.hidden_on_focus_loss = Some(Instant::now());
                self.hide();
            }
            WaylandEvent::KeyPress(key) => {
                self.input.handle_keyboard_event(&key, true, false);
                self.dirty = true;
            }
            WaylandEvent::KeyRelease(key) => {
                self.input.handle_keyboard_event(&key, false, false);
                self.dirty = true;
            }
            WaylandEvent::KeyRepeat(key) => {
                self.input.handle_keyboard_event(&key, true, true);
                self.dirty = true;
            }
            WaylandEvent::ModifiersChanged(modifiers) => {
                self.input.update_modifiers(&modifiers);
                self.modifiers = to_egui(&modifiers);
                self.dirty = true;
            }
            WaylandEvent::PointerEvent((surface, position, kind)) => {
                self.input.handle_pointer_event(&smithay_client_toolkit::seat::pointer::PointerEvent { surface, position, kind });
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn open_list(&mut self, token: Option<&str>) {
        if let Some(list) = &self.list {
            if let Some(token) = token {
                self.activation.activate(token, list.window.wl_surface());
            }
            return;
        }
        let (width, height) = (640, 720);
        let surface = self.app.compositor_state.create_surface(&self.app.qh);
        let target = self.gpu.target(&self.app.conn, &surface);
        let window = self.app.xdg_shell.create_window(surface, WindowDecorations::ServerDefault, &self.app.qh);
        window.set_title("Génie commands");
        window.set_app_id("genie");
        window.set_min_size(Some((360, 300)));
        window.commit();
        let ctx = egui::Context::default();
        launcher::fonts(&ctx);
        connect_repaint(&ctx, &self.repaint_at, &self.events);
        // SAFETY: the connection outlives the clipboard, as for the launcher's.
        let clipboard = unsafe { Clipboard::new(self.app.conn.display().id().as_ptr() as *mut _) };
        self.list = Some(ListWindow {
            target,
            window,
            renderer: self.gpu.renderer(),
            ctx,
            input: WaylandToEguiInput::new(clipboard),
            modifiers: egui::Modifiers::NONE,
            icons: icons::Icons::new(self.events.clone(), icons::Window::CommandList),
            list: CommandList::new(self.launcher.runner.groups()),
            size: (width, height),
            scale: self.scale,
            configured: false,
            focused: false,
        });
        let _ = self.app.conn.flush();
    }

    fn list_event(&mut self, event: WaylandEvent) {
        let Some(list) = &mut self.list else { return };
        match event {
            WaylandEvent::WindowConfigure(_, configure) => {
                if let (Some(width), Some(height)) = configure.new_size {
                    list.size = (width.get(), height.get());
                }
                list.configured = true;
            }
            WaylandEvent::WindowRequestClose(_) => {
                self.list = None;
                let _ = self.app.conn.flush();
            }
            WaylandEvent::ScaleFactorChanged(surface, scale) => {
                surface.set_buffer_scale(scale);
                list.scale = scale.max(1);
            }
            WaylandEvent::KeyboardEnter(..) => {
                list.focused = true;
                list.input.handle_keyboard_enter();
            }
            WaylandEvent::KeyboardLeave(_) => {
                list.focused = false;
                list.input.handle_keyboard_leave();
            }
            WaylandEvent::KeyPress(key) => list.input.handle_keyboard_event(&key, true, false),
            WaylandEvent::KeyRelease(key) => list.input.handle_keyboard_event(&key, false, false),
            WaylandEvent::KeyRepeat(key) => list.input.handle_keyboard_event(&key, true, true),
            WaylandEvent::ModifiersChanged(modifiers) => {
                list.input.update_modifiers(&modifiers);
                list.modifiers = to_egui(&modifiers);
            }
            WaylandEvent::PointerEvent((surface, position, kind)) => {
                list.input.handle_pointer_event(&smithay_client_toolkit::seat::pointer::PointerEvent { surface, position, kind });
            }
            _ => return,
        }
        self.dirty = true;
    }

    fn draw_list(&mut self) {
        let Some(list) = &mut self.list else { return };
        if !list.configured {
            return;
        }
        list.input.set_screen_size(list.size.0, list.size.1);
        let raw = with_modifiers(list.input.take_raw_input(), list.modifiers);
        list.ctx.set_pixels_per_point(list.scale as f32);
        let mut outcome = None;
        let (commands, icons) = (&mut list.list, &mut list.icons);
        let output = list.ctx.run_ui(raw, |ui| outcome = commands.draw(ui, icons));
        for command in &output.platform_output.commands {
            list.input.handle_output_command(command);
        }
        self.app.set_cursor(wayapp::egui_to_cursor_shape(output.platform_output.cursor_icon));
        let scale = list.scale as u32;
        let size = (list.size.0 * scale, list.size.1 * scale);
        self.gpu.render(&mut list.renderer, &mut list.target, &list.ctx, output, size);
        let _ = self.app.conn.flush();
        match outcome {
            Some(Outcome::Close) => {
                self.list = None;
                let _ = self.app.conn.flush();
            }
            Some(Outcome::Run(command)) => self.show(Some(command)),
            None => {}
        }
    }

    fn draw(&mut self) {
        let Some(shown) = &self.shown else { return };
        if !shown.configured {
            return;
        }
        let (width, height) = launcher::surface_size();
        self.input.set_screen_size(width, height);
        let raw = with_modifiers(self.input.take_raw_input(), self.modifiers);
        self.ctx.set_pixels_per_point(self.scale as f32);
        let mut requests = Vec::new();
        let mut panel = None;
        let (launcher, icons) = (&mut self.launcher, &mut self.icons);
        let output = self.ctx.run_ui(raw, |ui| {
            let (asked, rect) = launcher.draw(ui, icons);
            requests = asked;
            panel = Some(rect);
        });
        for command in &output.platform_output.commands {
            self.input.handle_output_command(command);
        }
        let Some(shown) = &mut self.shown else { return };
        if let Some(rect) = panel
            && shown.panel != Some(rect)
        {
            shown.panel = Some(rect);
            if let Ok(region) = Region::new(&self.app.compositor_state) {
                region.add(rect.left() as i32, rect.top() as i32, rect.width().ceil() as i32, rect.height().ceil() as i32);
                shown.layer.wl_surface().set_input_region(Some(region.wl_region()));
            }
        }
        let scale = self.scale as u32;
        let presented = self.gpu.render(&mut self.renderer, &mut shown.target, &self.ctx, output, (width * scale, height * scale));
        if presented && shown.first_frame {
            shown.first_frame = false;
            log::info!("first frame {:?} after the command", shown.show_requested.elapsed());
        }
        let _ = self.app.conn.flush();
        for request in requests {
            self.request(request);
        }
    }

    fn request(&mut self, request: Request) {
        match request {
            Request::Hide => self.hide(),
            Request::Run(action, item) => self.run_action(action, item),
        }
    }

    fn run_action(&mut self, action: plugin::Action, item: plugin::Item) {
        let query = self.launcher.history_query();
        self.launcher.history.record(&query, item.plugin, &item.id, &action.id);
        let token = self.shown.as_ref().and_then(|shown| self.activation.token(shown.layer.wl_surface()));
        log::debug!("running {} of {}, token: {}", action.id, item.id, token.is_some());
        match action.effect {
            Effect::Spawn { argv, cwd, id } => launch::spawn(&argv, cwd.as_deref(), &id, token.as_deref()),
            Effect::Focus { names, argv, cwd, id } => {
                if !self.windows.focus(&names) {
                    launch::spawn(&argv, cwd.as_deref(), &id, token.as_deref());
                }
            }
            Effect::Open(target) => launch::open(&target, token.as_deref()),
            Effect::Copy(text) => self.app.clipboard.store(text),
            Effect::OpenCommandList => self.open_list(token.as_deref()),
            Effect::Call(call) => {
                std::thread::spawn(move || call());
            }
        }
        self.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egui_sees_alt_held_and_released() {
        let ctx = egui::Context::default();
        let held = |modifiers| {
            let mut alt = false;
            let mut output = ctx.run_ui(with_modifiers(egui::RawInput::default(), modifiers), |ui| {
                alt = ui.ctx().input(|i| i.modifiers.alt);
            });
            output.textures_delta.clear();
            alt
        };
        assert!(!held(egui::Modifiers::NONE));
        assert!(held(egui::Modifiers::ALT));
        assert!(held(egui::Modifiers::ALT), "still held on a frame with no key event");
        assert!(!held(egui::Modifiers::NONE));
    }
}
