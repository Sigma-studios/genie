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
    /// `/this something` searches it.
    pub command: String,
    /// A shorter way to type it.
    #[serde(default)]
    pub alias: Option<String>,
    /// `%s` is the query.
    pub url: String,
    /// Offered in the regular search, after apps and files.
    #[serde(default)]
    pub regular: bool,
    /// In the icon theme.
    #[serde(default)]
    pub icon: Option<String>,
}

#[derive(Deserialize, Clone)]
pub struct PowerEntry {
    pub id: String,
    /// `/this` runs it.
    pub name: String,
    pub title: String,
    pub icon: String,
    pub command: Vec<String>,
    /// Asks before running it.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Deserialize, Clone)]
pub struct CustomCommand {
    /// `/this` runs it.
    pub name: String,
    pub title: String,
    pub icon: String,
    pub command: Vec<String>,
    /// Shown beside it, for a key bound elsewhere. Génie doesn't bind it.
    #[serde(default)]
    pub shortcut: Option<String>,
}

#[derive(Deserialize)]
struct CustomCommands {
    command: Vec<CustomCommand>,
}

/// ~/.config/genie/commands.toml, apart from config.toml so that one can be
/// generated while the other stays yours to edit.
pub fn load_commands() -> Vec<CustomCommand> {
    let path = config_home().join("genie/commands.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match toml::from_str::<CustomCommands>(&text) {
        Ok(commands) => commands.command,
        Err(e) => {
            log::warn!("{}: {e}", path.display());
            Vec::new()
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_commands_parse_with_and_without_a_shortcut() {
        let text = r#"
            [[command]]
            name = "record-start"
            title = "Recording: start"
            icon = "media-record"
            command = ["gsr-start"]
            shortcut = "Ctrl+Alt+Home"

            [[command]]
            name = "hello"
            title = "Hello"
            icon = "face-smile"
            command = ["notify-send", "hello"]
        "#;
        let commands = toml::from_str::<CustomCommands>(text).unwrap().command;
        assert_eq!(commands[0].shortcut.as_deref(), Some("Ctrl+Alt+Home"));
        assert_eq!(commands[1].shortcut, None);
        assert_eq!(commands[1].command, ["notify-send", "hello"]);
    }
}

/// The ids are Albert's, so usage history imported from it still applies.
fn engines() -> Vec<Engine> {
    let engine = |id: &str, name: &str, command: &str, alias: &str, url: &str, icon: &str| Engine {
        id: id.into(),
        name: name.into(),
        command: command.into(),
        alias: Some(alias.to_string()).filter(|a| a != command),
        url: url.into(),
        regular: command == "ecosia",
        icon: Some(format!("genie-{icon}")),
    };
    vec![
        engine("247c2e7e", "Amazon", "amazon", "ama", "https://www.amazon.fr/s?k=%s", "amazon"),
        engine("bdf36e73", "eBay", "ebay", "eb", "https://www.ebay.com/sch/i.html?_nkw=%s", "ebay"),
        engine("ecosia", "Ecosia", "ecosia", "ec", "https://www.ecosia.org/search?q=%s", "ecosia"),
        engine("7f41c049", "GitHub", "github", "gh", "https://github.com/search?utf8=✓&q=%s", "github"),
        engine("48998825", "Google Maps", "maps", "maps", "https://www.google.com/maps/search/%s/", "googlemaps"),
        engine("9498795b", "Google Scholar", "scholar", "scholar", "https://scholar.google.com/scholar?q=%s", "googlescholar"),
        engine("74d48b98", "Google Translate", "translate", "gt", "https://translate.google.com/?sl=auto&tl=fr&text=%s&op=translate", "googletranslate"),
        engine("2b062a76", "Wolfram Alpha", "wolfram", "wa", "https://www.wolframalpha.com/input/?i=%s", "wolfram"),
        engine("e7021408", "YouTube", "youtube", "yt", "https://www.youtube.com/results?search_query=%s", "youtube"),
    ]
}

/// Each is its own command. Their descriptions all say "Power", so `/power`
/// lists them, which is the menu behind Ctrl+Alt+Delete. org.kde.Shutdown acts
/// without KDE's prompt.
fn power() -> Vec<PowerEntry> {
    let entry = |id: &str, name: &str, title: &str, icon: &str, command: &[&str]| PowerEntry {
        id: id.into(),
        name: name.into(),
        title: title.into(),
        icon: icon.into(),
        command: command.iter().map(|s| s.to_string()).collect(),
        confirm: id != "Lock",
    };
    let shutdown = |method: &'static str| ["qdbus", "org.kde.Shutdown", "/Shutdown", method];
    vec![
        entry("Poweroff", "shutdown", "Shut down", "system-shutdown", &shutdown("org.kde.Shutdown.logoutAndShutdown")),
        entry("Reboot", "restart", "Restart", "system-reboot", &shutdown("org.kde.Shutdown.logoutAndReboot")),
        entry("Logout", "logout", "Log out", "system-log-out", &shutdown("org.kde.Shutdown.logout")),
        entry("Lock", "lock", "Lock", "system-lock-screen", &["loginctl", "lock-session"]),
    ]
}
