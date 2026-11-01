//! Génie, a launcher that stays resident and shows on a socket command.
//! `genie` runs it, `genie toggle`, `genie show [text]`, `genie hide`,
//! `genie activate` and `genie quit` talk to it, starting it first if need be.

mod activation;
mod config;
mod gpu;
mod history;
mod icons;
mod launch;
mod launcher;
mod matcher;
mod plugin;
mod plugins;

use activation::Activation;
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
    Icon { icon: plugin::Icon, image: Option<egui::ColorImage> },
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

struct Daemon {
    app: Application,
    gpu: Gpu,
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
        Arc::new(plugins::power::Power(config.power.clone())),
        Arc::new(plugins::websearch::WebSearch(config.websearch.clone())),
        Arc::new(plugins::files::Files),
        Arc::new(plugins::zed::Zed::new(config.terminal.clone())),
        Arc::new(plugins::spotify::Spotify::new(config.spotify)),
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
    // SAFETY: the connection outlives the clipboard: both last as long as the
    // daemon.
    let clipboard = unsafe { Clipboard::new(app.conn.display().id().as_ptr() as *mut _) };
    let input = WaylandToEguiInput::new(clipboard);
    app.run_dispatcher();

    let ctx = egui::Context::default();
    launcher::fonts(&ctx);
    let repaint_at = Arc::new(Mutex::new(None));
    {
        let (repaint_at, events) = (repaint_at.clone(), events.clone());
        ctx.set_request_repaint_callback(move |info| {
            *repaint_at.lock().unwrap() = Some(Instant::now() + info.delay);
            let _ = events.send(Event::Wake);
        });
    }
    let mut daemon = Daemon {
        app,
        gpu,
        activation,
        ctx,
        input,
        launcher: Launcher::new(plugin::Runner::new(plugins, events.clone()), history::History::open()),
        icons: icons::Icons::new(events.clone()),
        shown: None,
        scale: 1,
        hidden_on_focus_loss: None,
        repaint_at,
        dirty: false,
    };
    log::info!("ready");
    daemon.run(&incoming);
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
            }
        }
    }

    /// How long to sleep. Nothing's scheduled while hidden.
    fn next_repaint(&self) -> Option<Duration> {
        if self.shown.is_none() {
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
            Event::Icon { icon, image } => {
                self.icons.loaded(&self.ctx, icon, image);
                self.dirty = true;
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
                self.dirty = true;
            }
            WaylandEvent::PointerEvent((surface, position, kind)) => {
                self.input.handle_pointer_event(&smithay_client_toolkit::seat::pointer::PointerEvent { surface, position, kind });
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn draw(&mut self) {
        let Some(shown) = &self.shown else { return };
        if !shown.configured {
            return;
        }
        let (width, height) = launcher::surface_size();
        self.input.set_screen_size(width, height);
        let raw = self.input.take_raw_input();
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
        let presented = self.gpu.render(&mut shown.target, &self.ctx, output, (width * scale, height * scale));
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
            Effect::Open(target) => launch::open(&target, token.as_deref()),
            Effect::Copy(text) => self.app.clipboard.store(text),
            Effect::Call(call) => {
                std::thread::spawn(move || call());
            }
        }
        self.hide();
    }
}
