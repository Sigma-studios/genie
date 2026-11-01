//! What a plugin is. Plugins are compiled in: each one turns a query into
//! items, on its own thread, and sends them as they come.

use crate::Event;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    /// A name in the icon theme.
    Theme(String),
    File(PathBuf),
    Url(String),
}

#[derive(Clone)]
pub enum Effect {
    /// Start a program, in its own scope. `id` names that scope.
    Spawn { argv: Vec<String>, cwd: Option<PathBuf>, id: String },
    /// A file, folder or URL, in whatever handles it.
    Open(String),
    Copy(String),
    /// Anything else. Runs off the UI thread.
    Call(Arc<dyn Fn() + Send + Sync>),
}

#[derive(Clone)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub effect: Effect,
}

impl Action {
    pub fn new(id: &str, label: &str, effect: Effect) -> Self {
        Self { id: id.into(), label: label.into(), effect }
    }
}

#[derive(Clone)]
pub struct Item {
    /// Stable across queries: what usage history is counted against.
    pub id: String,
    /// Filled in by the query runner.
    pub plugin: &'static str,
    pub title: String,
    pub subtitle: String,
    pub icon: Icon,
    /// Enter runs the first action. Holding Alt lists them all.
    pub actions: Vec<Action>,
    /// What Tab puts in the input.
    pub completion: Option<String>,
    pub score: f32,
    /// Listed after everything else, whatever the score, and doesn't count as
    /// a match when deciding to offer fallbacks.
    pub background: bool,
}

impl Item {
    pub fn new(id: impl Into<String>, title: impl Into<String>, subtitle: impl Into<String>, icon: Icon) -> Self {
        Self {
            id: id.into(),
            plugin: "",
            title: title.into(),
            subtitle: subtitle.into(),
            icon,
            actions: Vec::new(),
            completion: None,
            score: 0.0,
            background: false,
        }
    }

    pub fn action(mut self, action: Action) -> Self {
        self.actions.push(action);
        self
    }

    pub fn score(mut self, score: f32) -> Self {
        self.score = score;
        self
    }

    pub fn background(mut self) -> Self {
        self.background = true;
        self
    }

    pub fn completion(mut self, completion: impl Into<String>) -> Self {
        self.completion = Some(completion.into());
        self
    }
}

pub struct Query {
    /// What's typed, without the trigger.
    pub text: String,
    /// The trigger that selected this plugin, if one did.
    pub trigger: Option<String>,
    generation: u64,
    current: Arc<AtomicU64>,
    plugin: &'static str,
    events: Sender<Event>,
}

impl Query {
    pub fn cancelled(&self) -> bool {
        self.current.load(Ordering::Relaxed) != self.generation
    }

    pub fn send(&self, mut items: Vec<Item>) {
        if items.is_empty() || self.cancelled() {
            return;
        }
        for item in &mut items {
            item.plugin = self.plugin;
        }
        let _ = self.events.send(Event::Results { generation: self.generation, items });
    }
}

pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;

    /// Typing one of these first ("f ", "zd ") sends what follows to this
    /// plugin alone. A plugin with `global` false only answers to them.
    fn triggers(&self) -> Vec<String> {
        Vec::new()
    }

    fn global(&self) -> bool {
        true
    }

    fn query(&self, query: &Query);

    /// Offered when nothing at all matched.
    fn fallbacks(&self, _text: &str) -> Vec<Item> {
        Vec::new()
    }

    /// The launcher is about to show: catch up, off the UI thread.
    fn refresh(&self) {}
}

/// Runs each query on every plugin concerned, and drops the ones still running
/// when the next keystroke comes.
pub struct Runner {
    plugins: Vec<Arc<dyn Plugin>>,
    current: Arc<AtomicU64>,
    generation: u64,
    events: Sender<Event>,
}

impl Runner {
    pub fn new(plugins: Vec<Arc<dyn Plugin>>, events: Sender<Event>) -> Self {
        Self { plugins, current: Arc::new(AtomicU64::new(0)), generation: 0, events }
    }

    pub fn refresh(&self) {
        for plugin in &self.plugins {
            let plugin = plugin.clone();
            std::thread::spawn(move || plugin.refresh());
        }
    }

    pub fn cancel(&mut self) {
        self.generation += 1;
        self.current.store(self.generation, Ordering::Relaxed);
    }

    /// Returns how many plugins will answer `Event::Finished` for this
    /// generation, which is the new `self.generation`.
    pub fn run(&mut self, text: &str) -> (u64, usize) {
        self.cancel();
        let chosen = self.choose(text);
        let count = chosen.len();
        for (plugin, query_text, trigger) in chosen {
            let id = plugin.id();
            let query = Query {
                text: query_text,
                trigger,
                generation: self.generation,
                current: self.current.clone(),
                plugin: id,
                events: self.events.clone(),
            };
            let events = self.events.clone();
            let generation = self.generation;
            std::thread::spawn(move || {
                plugin.query(&query);
                let _ = events.send(Event::Finished { generation });
            });
        }
        (self.generation, count)
    }

    fn choose(&self, text: &str) -> Vec<(Arc<dyn Plugin>, String, Option<String>)> {
        for plugin in &self.plugins {
            for trigger in plugin.triggers() {
                if let Some(rest) = text.strip_prefix(trigger.as_str()) {
                    return vec![(plugin.clone(), rest.to_string(), Some(trigger))];
                }
            }
        }
        if text.trim().is_empty() {
            return Vec::new();
        }
        self.plugins
            .iter()
            .filter(|p| p.global())
            .map(|p| (p.clone(), text.to_string(), None))
            .collect()
    }

    /// Only for an untriggered query: a trigger's plugin has already had its say.
    pub fn fallbacks(&self, text: &str) -> Vec<Item> {
        if self.plugins.iter().any(|p| p.triggers().iter().any(|t| text.starts_with(t.as_str()))) {
            return Vec::new();
        }
        self.plugins
            .iter()
            .flat_map(|p| {
                let id = p.id();
                let mut items = p.fallbacks(text);
                for item in &mut items {
                    item.plugin = id;
                }
                items
            })
            .collect()
    }
}
