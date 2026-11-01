//! ~/.config/genie/config.toml. Everything has a default, so the file is
//! optional. Reads once, at startup.

use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(default)]
pub struct Config {
    /// Prefix for desktop entries with Terminal=true.
    pub terminal: Vec<String>,
    pub spotify: Spotify,
    pub websearch: Vec<Engine>,
    pub power: Vec<PowerEntry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Spotify {
    /// From an app at developer.spotify.com, whose redirect URI is
    /// http://127.0.0.1:47813/callback. Without one, `sp` only controls the
    /// player. Signing in needs no secret, so this isn't one.
    pub client_id: String,
}

#[derive(Deserialize, Clone)]
pub struct Engine {
    pub id: String,
    pub name: String,
    pub trigger: String,
    /// `%s` is the query.
    pub url: String,
    /// Offered when nothing else matched.
    #[serde(default)]
    pub fallback: bool,
    /// In the icon theme.
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct PowerEntry {
    pub id: String,
    pub title: String,
    pub icon: String,
    pub command: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            terminal: ["wezterm", "start", "--"].map(String::from).to_vec(),
            spotify: Spotify::default(),
            websearch: engines(),
            power: power(),
        }
    }
}

pub fn load() -> Config {
    let path = config_home().join("genie/config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Config::default();
    };
    toml::from_str(&text).unwrap_or_else(|e| {
        log::warn!("{}: {e}", path.display());
        Config::default()
    })
}

pub fn config_home() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share")
}

pub fn cache_home() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

fn xdg_dir(variable: &str, fallback: &str) -> PathBuf {
    // Set but empty counts as unset, says the XDG spec.
    std::env::var_os(variable).filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(fallback))
}

/// The ids are Albert's, so usage history imported from it still applies.
fn engines() -> Vec<Engine> {
    let engine = |id: &str, name: &str, trigger: &str, url: &str, fallback| Engine {
        id: id.into(),
        name: name.into(),
        trigger: trigger.into(),
        url: url.into(),
        fallback,
        icon: None,
    };
    vec![
        engine("247c2e7e", "Amazon", "ama", "https://www.amazon.com/s/?field-keywords=%s", false),
        engine("c51c2a67", "Chat GPT", "gpt", "https://chat.openai.com/?q=%s", true),
        engine("4962f117", "DuckDuckGo", "dd", "https://duckduckgo.com/?q=%s", false),
        engine("bdf36e73", "Ebay", "eb", "https://www.ebay.com/sch/i.html?_nkw=%s", false),
        engine("7f41c049", "GitHub", "gh", "https://github.com/search?utf8=✓&q=%s", false),
        engine("02fb8f09", "Google", "gg", "https://www.google.com/search?q=%s", true),
        engine("48998825", "Google Maps", "maps", "https://www.google.com/maps/search/%s/", true),
        engine("9498795b", "Google Scholar", "scholar", "https://scholar.google.com/scholar?q=%s", false),
        engine("74d48b98", "Google Translate", "gt", "https://translate.google.com/?text=%s", false),
        engine("2b062a76", "Wolfram Alpha", "wa", "https://www.wolframalpha.com/input/?i=%s", false),
        engine("e7021408", "YouTube", "yt", "https://www.youtube.com/results?search_query=%s", true),
    ]
}

/// The power menu behind Ctrl+Alt+Delete: `genie show Power` lists them all,
/// thanks to the shared prefix. org.kde.Shutdown acts without KDE's prompt.
fn power() -> Vec<PowerEntry> {
    let entry = |id: &str, title: &str, icon: &str, command: &[&str]| PowerEntry {
        id: id.into(),
        title: title.into(),
        icon: icon.into(),
        command: command.iter().map(|s| s.to_string()).collect(),
    };
    let shutdown = |method: &'static str| ["qdbus", "org.kde.Shutdown", "/Shutdown", method];
    vec![
        entry("Poweroff", "Power: Shut down", "system-shutdown", &shutdown("org.kde.Shutdown.logoutAndShutdown")),
        entry("Reboot", "Power: Restart", "system-reboot", &shutdown("org.kde.Shutdown.logoutAndReboot")),
        entry("Logout", "Power: Log out", "system-log-out", &shutdown("org.kde.Shutdown.logout")),
        entry("Lock", "Power: Lock", "system-lock-screen", &["loginctl", "lock-session"]),
    ]
}
