//! What a plugin is. Plugins are compiled in: each one turns a query into
//! items, on its own thread, and sends them as they come.
//!
//! Typing without a slash is the regular search: apps, then files, then the web.
//! Everything else is a command: `/` lists them, `/maps paris` runs one.

use crate::Event;
use crate::matcher;
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
    /// An app: its window if it has one under these names, or else it starts.
    Focus { names: Vec<String>, argv: Vec<String>, cwd: Option<PathBuf>, id: String },
    /// A file, folder or URL, in whatever handles it.
    Open(String),
    Copy(String),
    /// The window listing every command.
    OpenCommandList,
    /// Anything else. Runs off the UI thread.
    Call(Arc<dyn Fn() + Send + Sync>),
}

#[derive(Clone)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub effect: Effect,
    /// Asked first, for what's hard to undo.
    pub confirm: Option<String>,
}

impl Action {
    pub fn new(id: &str, label: &str, effect: Effect) -> Self {
        Self { id: id.into(), label: label.into(), effect, confirm: None }
    }

    pub fn confirm(mut self, question: impl Into<String>) -> Self {
        self.confirm = Some(question.into());
        self
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
    /// Results sort by this first, lowest on top, then by score: it's what keeps
    /// apps above files above Google, however often something was used.
    pub tier: u8,
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
            tier: 0,
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

    pub fn tier(mut self, tier: u8) -> Self {
        self.tier = tier;
        self
    }

    pub fn completion(mut self, completion: impl Into<String>) -> Self {
        self.completion = Some(completion.into());
        self
    }
}

pub struct Query {
    /// What's typed, without the command.
    pub text: String,
    /// The command this is for, by its name. `None` in the regular search.
    pub command: Option<String>,
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

/// Something typed after a slash.
#[derive(Clone)]
pub struct Command {
    pub name: String,
    /// Other names that also run it, like `yt` for `youtube`.
    pub aliases: Vec<String>,
    pub description: String,
    pub icon: String,
    /// Enter on it in a list of commands runs this, rather than completing it.
    pub action: Option<Action>,
    /// A key that also runs it, to show beside it.
    pub shortcut: Option<String>,
}

impl Command {
    pub fn new(name: &str, aliases: &[&str], description: &str, icon: &str) -> Self {
        Self {
            name: name.into(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
            description: description.into(),
            icon: icon.into(),
            action: None,
            shortcut: None,
        }
    }

    pub fn action(mut self, action: Action) -> Self {
        self.action = Some(action);
        self
    }

    pub fn shortcut(mut self, shortcut: &str) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// The line under its name.
    pub fn detail(&self) -> String {
        match &self.shortcut {
            Some(shortcut) => format!("{}  ·  {shortcut}", self.description),
            None => self.description.clone(),
        }
    }

    fn answers_to(&self, name: &str) -> bool {
        self.name == name || self.aliases.iter().any(|a| a == name)
    }
}

pub trait Plugin: Send + Sync {
    fn id(&self) -> &'static str;

    /// What its commands are listed under in the command list.
    fn group(&self) -> &'static str {
        "Other"
    }

    fn commands(&self) -> Vec<Command> {
        Vec::new()
    }

    /// Whether it answers in the regular search.
    fn regular(&self) -> bool {
        false
    }

    fn query(&self, query: &Query);

    /// The launcher is about to show: catch up, off the UI thread.
    fn refresh(&self) {}
}

/// What was started for a query. `items` are there right away: the list of
/// commands needs no plugin.
/// Items of this tier wait for every plugin to finish: the web search answers at
/// once and files take a moment, and it shouldn't be selected in between.
pub const LAST_TIER: u8 = 2;

pub struct Started {
    pub generation: u64,
    pub pending: usize,
    pub items: Vec<Item>,
}

/// What a line of input asks for.
#[derive(Debug, PartialEq)]
enum Parsed<'a> {
    Regular,
    /// `/ma`: the commands that match.
    List(&'a str),
    /// `/maps` or `/maps paris`.
    Run(&'a str, &'a str),
}

fn parse(text: &str, known: impl Fn(&str) -> bool) -> Parsed<'_> {
    let Some(rest) = text.strip_prefix('/') else { return Parsed::Regular };
    match rest.split_once(char::is_whitespace) {
        Some((name, arguments)) if known(name) => Parsed::Run(name, arguments),
        Some(_) => Parsed::List(rest),
        None if known(rest) => Parsed::Run(rest, ""),
        None => Parsed::List(rest),
    }
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

    pub fn run(&mut self, text: &str) -> Started {
        self.cancel();
        let all = self.commands();
        let parsed = parse(text, |name| name == LIST || all.iter().any(|(_, c)| c.answers_to(name)));
        let mut items = Vec::new();
        let chosen: Vec<(Arc<dyn Plugin>, String, Option<String>)> = match parsed {
            Parsed::Regular if text.trim().is_empty() => Vec::new(),
            Parsed::Regular => {
                self.plugins.iter().filter(|p| p.regular()).map(|p| (p.clone(), text.to_string(), None)).collect()
            }
            Parsed::List(typed) => {
                let list = list_command();
                items = list_commands(all.iter().map(|(_, c)| c).chain([&list]), typed);
                for item in items.iter_mut().filter(|item| item.title == format!("/{LIST}")) {
                    open_list(item);
                }
                Vec::new()
            }
            Parsed::Run(LIST, _) => {
                let list = list_command();
                items = list_commands(std::iter::once(&list), "");
                items.iter_mut().for_each(open_list);
                Vec::new()
            }
            Parsed::Run(name, arguments) => all
                .iter()
                .find(|(_, c)| c.answers_to(name))
                .map(|(p, c)| (p.clone(), arguments.to_string(), Some(c.name.clone())))
                .into_iter()
                .collect(),
        };
        let pending = chosen.len();
        for (plugin, query_text, command) in chosen {
            let id = plugin.id();
            let query = Query {
                text: query_text,
                command,
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
        Started { generation: self.generation, pending, items }
    }

    /// Every plugin's commands, under its group, for the command list.
    pub fn groups(&self) -> Vec<(String, Vec<Command>)> {
        let mut groups: Vec<(String, Vec<Command>)> = Vec::new();
        for plugin in &self.plugins {
            let mut commands = plugin.commands();
            if commands.is_empty() {
                continue;
            }
            commands.sort_by(|a, b| a.name.cmp(&b.name));
            groups.push((plugin.group().to_string(), commands));
        }
        groups
    }

    fn commands(&self) -> Vec<(Arc<dyn Plugin>, Command)> {
        self.plugins.iter().flat_map(|p| p.commands().into_iter().map(|c| (p.clone(), c)).collect::<Vec<_>>()).collect()
    }
}

const LIST: &str = "list";

fn list_command() -> Command {
    Command::new(LIST, &[], "Open a window with every command, by group", "view-list-details")
}

fn open_list(item: &mut Item) {
    item.completion = None;
    item.actions = vec![Action::new("open", "Open", Effect::OpenCommandList)];
}

/// By name, and by what they do, so `/music` finds Spotify. All of them,
/// alphabetically, for a bare slash.
fn list_commands<'a>(commands: impl Iterator<Item = &'a Command>, typed: &str) -> Vec<Item> {
    let typed = typed.trim();
    let mut sorted: Vec<&Command> = commands.collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    let count = sorted.len() as f32;
    sorted
        .into_iter()
        .enumerate()
        .filter_map(|(i, command)| {
            let names: Vec<(&str, f32)> = std::iter::once((command.name.as_str(), 1.0))
                .chain(command.aliases.iter().map(|a| (a.as_str(), 1.0)))
                .chain(std::iter::once((command.description.as_str(), 0.5)))
                .collect();
            let score = if typed.is_empty() { Some(1.0) } else { matcher::score(typed, &names) }?;
            let order = (count - i as f32) * 0.0001;
            let mut item = Item::new(
                format!("/{}", command.name),
                format!("/{}", command.name),
                command.detail(),
                Icon::Theme(command.icon.clone()),
            )
            .completion(format!("/{} ", command.name))
            .score(score + order);
            item.plugin = "commands";
            if let Some(action) = &command.action {
                item.completion = None;
                item.actions = vec![action.clone()];
            }
            Some(item)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known(name: &str) -> bool {
        ["maps", "power"].contains(&name)
    }

    #[test]
    fn input_is_parsed() {
        assert_eq!(parse("firefox", known), Parsed::Regular);
        assert_eq!(parse("a /maps", known), Parsed::Regular);
        assert_eq!(parse("/", known), Parsed::List(""));
        assert_eq!(parse("/ma", known), Parsed::List("ma"));
        assert_eq!(parse("/maps", known), Parsed::Run("maps", ""));
        assert_eq!(parse("/maps paris texas", known), Parsed::Run("maps", "paris texas"));
        assert_eq!(parse("/maps ", known), Parsed::Run("maps", ""));
        assert_eq!(parse("/nope x", known), Parsed::List("nope x"));
    }

    #[test]
    fn commands_are_found_by_name_alias_and_description() {
        let commands = [
            Command::new("spotify", &["sp"], "Control the music", "spotify"),
            Command::new("maps", &[], "Search Google Maps", "map"),
        ];
        let titles = |typed| -> Vec<String> {
            let mut items = list_commands(commands.iter(), typed);
            items.sort_by(|a, b| b.score.total_cmp(&a.score));
            items.into_iter().map(|i| i.title).collect()
        };
        assert_eq!(titles(""), ["/maps", "/spotify"]);
        assert_eq!(titles("sp"), ["/spotify"]);
        assert_eq!(titles("music"), ["/spotify"]);
        assert!(titles("zzz").is_empty());
        assert_eq!(list_commands(commands.iter(), "ma")[0].completion.as_deref(), Some("/maps "));
    }
}
