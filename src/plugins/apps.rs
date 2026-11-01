//! Desktop entries. Read at startup and again whenever the launcher shows, so
//! a rebuild's new apps are there without restarting. Enter brings an app's
//! window forward if it has one, and Alt offers a new window.

use crate::config;
use crate::launch;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Clone, Debug, PartialEq)]
struct App {
    /// File name without .desktop.
    id: String,
    name: String,
    localized_name: Option<String>,
    generic_name: String,
    comment: String,
    keywords: Vec<String>,
    icon: Option<String>,
    /// What its windows call themselves, when the entry says.
    wm_class: Option<String>,
    argv: Vec<String>,
    cwd: Option<PathBuf>,
    /// Name and command of the entry's extra actions.
    actions: Vec<(String, Vec<String>)>,
}

pub struct Apps {
    apps: RwLock<Vec<App>>,
    terminal: Vec<String>,
}

impl Apps {
    pub fn new(terminal: Vec<String>) -> Self {
        let apps = RwLock::new(scan(&terminal));
        Self { apps, terminal }
    }
}

impl Plugin for Apps {
    fn id(&self) -> &'static str {
        "applications"
    }

    fn regular(&self) -> bool {
        true
    }

    fn refresh(&self) {
        let apps = scan(&self.terminal);
        *self.apps.write().unwrap() = apps;
    }

    fn query(&self, query: &Query) {
        let apps = self.apps.read().unwrap();
        let items = apps.iter().filter_map(|app| {
            let executable = app.argv.first().and_then(|a| a.rsplit('/').next()).unwrap_or("");
            let mut fields = vec![(app.name.as_str(), 1.0), (app.generic_name.as_str(), 0.7), (executable, 0.6)];
            if let Some(name) = &app.localized_name {
                fields.push((name, 1.0));
            }
            fields.extend(app.keywords.iter().map(|k| (k.as_str(), 0.8)));
            let score = matcher::score(&query.text, &fields)?;
            let title = app.localized_name.as_ref().unwrap_or(&app.name);
            let subtitle = if app.comment.is_empty() { &app.generic_name } else { &app.comment };
            let icon = app.icon.as_ref().map_or_else(|| Icon::Theme("application-x-executable".into()), |i| {
                if i.starts_with('/') { Icon::File(i.into()) } else { Icon::Theme(i.clone()) }
            });
            let spawn = |argv: &Vec<String>| Effect::Spawn { argv: argv.clone(), cwd: app.cwd.clone(), id: app.id.clone() };
            let mut item = Item::new(&app.id, title, subtitle, icon)
                .action(Action::new("launch", "Open", Effect::Focus {
                    names: app.window_names(),
                    argv: app.argv.clone(),
                    cwd: app.cwd.clone(),
                    id: app.id.clone(),
                }))
                .action(Action::new("new", "New window", spawn(&app.argv)))
                .completion(title.clone())
                .score(score);
            for (name, argv) in &app.actions {
                item = item.action(Action::new(name, name, spawn(argv)));
            }
            Some(item)
        });
        query.send(items.collect());
    }
}

impl App {
    /// What a window of this app can be called. The program's name only counts
    /// when it runs without arguments: a Steam game's entry runs `steam` with
    /// the game, and shouldn't just bring Steam forward.
    fn window_names(&self) -> Vec<String> {
        let mut names = vec![self.id.clone()];
        names.extend(self.wm_class.clone());
        if let [program] = self.argv.as_slice() {
            names.extend(program.rsplit('/').next().map(String::from));
        }
        names
    }
}

fn data_dirs() -> Vec<PathBuf> {
    let others = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(config::data_home())
        .chain(others.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .collect()
}

fn scan(terminal: &[String]) -> Vec<App> {
    let desktops = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let desktops: Vec<&str> = desktops.split(':').collect();
    let locale = std::env::var("LC_MESSAGES").or_else(|_| std::env::var("LANG")).unwrap_or_default();
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in data_dirs() {
        let root = dir.join("applications");
        let mut files = Vec::new();
        collect(&root, &mut files);
        for file in files {
            let Ok(relative) = file.strip_prefix(&root) else { continue };
            let id = relative.to_string_lossy().trim_end_matches(".desktop").replace('/', "-");
            // The first one found hides later ones, even when it hides the app.
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else { continue };
            if let Some(app) = parse(&id, &text, &locale, &desktops, terminal, &file) {
                apps.push(app);
            }
        }
    }
    apps
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "desktop") {
            out.push(path);
        }
    }
}

fn parse(id: &str, text: &str, locale: &str, desktops: &[&str], terminal: &[String], file: &Path) -> Option<App> {
    let groups = groups(text);
    let main = groups.get("Desktop Entry")?;
    let get = |key: &str| main.get(key).map(String::as_str).unwrap_or("");
    let yes = |key: &str| get(key) == "true";
    if get("Type") != "Application" || yes("Hidden") || yes("NoDisplay") {
        return None;
    }
    let listed = |key: &str| get(key).split(';').any(|d| !d.is_empty() && desktops.contains(&d));
    if (main.contains_key("OnlyShowIn") && !listed("OnlyShowIn")) || listed("NotShowIn") {
        return None;
    }
    if main.get("TryExec").is_some_and(|program| !launch::on_path(program)) {
        return None;
    }
    let name = get("Name").to_string();
    if name.is_empty() {
        return None;
    }
    let icon = main.get("Icon").filter(|i| !i.is_empty()).cloned();
    let command = |exec: &str| -> Option<Vec<String>> {
        let argv = parse_exec(exec, icon.as_deref(), &name, file)?;
        if yes("Terminal") { Some(terminal.iter().cloned().chain(argv).collect()) } else { Some(argv) }
    };
    let argv = command(get("Exec"))?;
    let actions = get("Actions")
        .split(';')
        .filter(|a| !a.is_empty())
        .filter_map(|action| {
            let group = groups.get(format!("Desktop Action {action}").as_str())?;
            let label = localized(group, "Name", locale).or_else(|| group.get("Name").cloned())?;
            Some((label, command(group.get("Exec")?)?))
        })
        .collect();
    let keywords = localized(main, "Keywords", locale).into_iter().chain(main.get("Keywords").cloned());
    Some(App {
        id: id.to_string(),
        localized_name: localized(main, "Name", locale).filter(|n| *n != name),
        generic_name: localized(main, "GenericName", locale).unwrap_or_else(|| get("GenericName").to_string()),
        comment: localized(main, "Comment", locale).unwrap_or_else(|| get("Comment").to_string()),
        keywords: keywords.flat_map(|k| k.split(';').filter(|k| !k.is_empty()).map(String::from).collect::<Vec<_>>()).collect(),
        cwd: main.get("Path").filter(|p| !p.is_empty()).map(PathBuf::from),
        name,
        icon,
        wm_class: main.get("StartupWMClass").filter(|c| !c.is_empty()).cloned(),
        argv,
        actions,
    })
}

fn groups(text: &str) -> HashMap<&str, HashMap<String, String>> {
    let mut groups: HashMap<&str, HashMap<String, String>> = HashMap::new();
    let mut current = None;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name);
            groups.entry(name).or_default();
        } else if let (Some(group), Some((key, value))) = (current, line.split_once('=')) {
            let map = groups.get_mut(group).unwrap();
            // The first of a repeated key wins.
            map.entry(key.trim().to_string()).or_insert_with(|| unescape(value.trim()));
        }
    }
    groups
}

fn unescape(value: &str) -> String {
    let mut out = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `fr_BE.UTF-8` looks for key[fr_BE], then key[fr].
fn localized(group: &HashMap<String, String>, key: &str, locale: &str) -> Option<String> {
    let locale = locale.split(['.', '@']).next().unwrap_or("");
    if locale.is_empty() {
        return None;
    }
    let language = locale.split('_').next().unwrap_or("");
    group.get(&format!("{key}[{locale}]")).or_else(|| group.get(&format!("{key}[{language}]"))).cloned()
}

/// Exec's quoting and field codes. Génie opens nothing with an app, so the
/// codes for files and URLs just go away.
fn parse_exec(exec: &str, icon: Option<&str>, name: &str, file: &Path) -> Option<Vec<String>> {
    let mut words: Vec<(String, bool)> = Vec::new();
    let mut word = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut in_quotes = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                started = true;
                quoted = true;
            }
            '\\' if in_quotes => word.push(chars.next()?),
            c if c.is_whitespace() && !in_quotes => {
                if started {
                    words.push((std::mem::take(&mut word), quoted));
                    started = false;
                    quoted = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if in_quotes {
        return None;
    }
    if started {
        words.push((word, quoted));
    }
    let mut argv = Vec::new();
    for (word, _) in words {
        match word.as_str() {
            "%f" | "%F" | "%u" | "%U" | "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => {}
            "%i" => {
                if let Some(icon) = icon {
                    argv.push("--icon".to_string());
                    argv.push(icon.to_string());
                }
            }
            _ => argv.push(expand_codes(&word, name, file)),
        }
    }
    if argv.is_empty() { None } else { Some(argv) }
}

fn expand_codes(word: &str, name: &str, file: &Path) -> String {
    let mut out = String::new();
    let mut chars = word.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('c') => out.push_str(name),
            Some('k') => out.push_str(&file.to_string_lossy()),
            Some('f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm' | 'i') | None => {}
            Some(other) => {
                out.push('%');
                out.push(other);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exec(text: &str) -> Vec<String> {
        parse_exec(text, Some("icon"), "App", Path::new("/a.desktop")).unwrap()
    }

    #[test]
    fn exec_quotes_and_codes() {
        assert_eq!(exec("heroic %u"), ["heroic"]);
        assert_eq!(exec(r#""/a b/c" --x="1 2" %F"#), ["/a b/c", "--x=1 2"]);
        assert_eq!(exec(r#"sh -c "echo \"hi\"""#), ["sh", "-c", r#"echo "hi""#]);
        assert_eq!(exec("a %i %c 100%%"), ["a", "--icon", "icon", "App", "100%"]);
        assert_eq!(exec("env A=%k b"), ["env", "A=/a.desktop", "b"]);
        assert!(parse_exec(r#"a "b"#, None, "", Path::new("")).is_none());
        assert!(parse_exec("%U", None, "", Path::new("")).is_none());
    }

    fn entry(text: &str) -> Option<App> {
        parse("app", text, "fr_BE.UTF-8", &["KDE"], &["term".into(), "--".into()], Path::new("/a.desktop"))
    }

    #[test]
    fn entries_are_read() {
        let app = entry(
            "# comment\n[Desktop Entry]\nType=Application\nName=Files\nName[fr]=Fichiers\nComment=Browse\nComment[fr_BE]=Parcourir\n\
             Keywords=epic;epic games;\nExec=files %U\nIcon=folder\nActions=new;\n\
             [Desktop Action new]\nName=New\nName[fr]=Nouveau\nExec=files --new\n",
        )
        .unwrap();
        assert_eq!(app.name, "Files");
        assert_eq!(app.localized_name.as_deref(), Some("Fichiers"));
        assert_eq!(app.comment, "Parcourir");
        assert_eq!(app.keywords, ["epic", "epic games"]);
        assert_eq!(app.argv, ["files"]);
        assert_eq!(app.actions, [("Nouveau".to_string(), vec!["files".to_string(), "--new".to_string()])]);
    }

    #[test]
    fn windows_are_known_by_id_class_and_lone_program() {
        let names = |text: &str| entry(&format!("[Desktop Entry]\nType=Application\nName=A\n{text}\n")).unwrap().window_names();
        assert_eq!(names("Exec=/nix/bin/valet %U\nStartupWMClass=crx_x"), ["app", "crx_x", "valet"]);
        // A game launched through Steam mustn't count as Steam's window.
        assert_eq!(names("Exec=steam steam://rungameid/1"), ["app"]);
    }

    #[test]
    fn hidden_entries_stay_out() {
        let base = "[Desktop Entry]\nType=Application\nName=A\nExec=a\n";
        assert!(entry(base).is_some());
        assert!(entry(&format!("{base}NoDisplay=true\n")).is_none());
        assert!(entry(&format!("{base}Hidden=true\n")).is_none());
        assert!(entry(&format!("{base}OnlyShowIn=GNOME;\n")).is_none());
        assert!(entry(&format!("{base}OnlyShowIn=GNOME;KDE;\n")).is_some());
        assert!(entry(&format!("{base}NotShowIn=KDE;\n")).is_none());
        assert!(entry(&format!("{base}TryExec=/does/not/exist\n")).is_none());
        assert!(entry("[Desktop Entry]\nType=Link\nName=A\nExec=a\n").is_none());
    }

    #[test]
    fn terminal_apps_get_the_prefix() {
        let app = entry("[Desktop Entry]\nType=Application\nName=A\nExec=htop\nTerminal=true\n").unwrap();
        assert_eq!(app.argv, ["term", "--", "htop"]);
    }
}
