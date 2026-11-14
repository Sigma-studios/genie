//! `/layout fr`: switch the keyboard layout, among those set up in the
//! desktop's keyboard settings. A bare `/layout` has the next one on top, so
//! Enter goes round them.
//!
//! Plasma says which layouts there are, and switches, over D-Bus. COSMIC has no
//! such call: its layouts are a list in the compositor's config, which it
//! watches, and the first one is the one in use. Switching moves it first.

use crate::config;
use crate::launch;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Command;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

const ICON: &str = "input-keyboard";
const BUS: &str = "org.kde.keyboard";
const OBJECT: &str = "/Layouts";
const INTERFACE: &str = "org.kde.KeyboardLayouts";

pub struct Layout;

#[derive(Clone, Debug, PartialEq)]
struct Entry {
    /// `fr`, `us`: what to type.
    short: String,
    /// What tells two of the same apart: COSMIC's variant, as `intl`, or the
    /// name it's been given in Plasma's settings.
    tag: String,
    /// `French`, `English (US)`.
    long: String,
}

fn cosmic() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.split(':').any(|d| d.eq_ignore_ascii_case("COSMIC")))
}

/// The layouts, and which is in use.
fn layouts() -> Result<(Vec<Entry>, Option<usize>), &'static str> {
    if cosmic() {
        let text = std::fs::read_to_string(cosmic_config()).map_err(|_| "COSMIC's keyboard settings have a single layout")?;
        let names = std::fs::read_to_string(xkb_list()).unwrap_or_default();
        Ok((cosmic_entries(&text, &names), Some(0)))
    } else {
        let list = busctl(&["getLayoutsList"]).ok_or("Plasma's keyboard service isn't answering")?;
        Ok((plasma_entries(&list), busctl(&["getLayout"]).as_ref().and_then(plasma_index)))
    }
}

fn switch(index: usize, entry: Entry) {
    let ok = if cosmic() { cosmic_switch(&entry) } else { plasma_switch(index) };
    if !ok {
        launch::notify(&format!("Couldn't switch to {}", entry.long));
    }
}

fn busctl(arguments: &[&str]) -> Option<Value> {
    let output = std::process::Command::new("busctl")
        .args(["--user", "--timeout=1", "-j", "call", BUS, OBJECT, INTERFACE])
        .args(arguments)
        .env_remove("LD_LIBRARY_PATH")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// From busctl's JSON of `getLayoutsList`: `{"type":"a(sss)","data":[[["us","","English (US)"]]]}`.
fn plasma_entries(answer: &Value) -> Vec<Entry> {
    let field = |entry: &Value, i: usize| entry[i].as_str().unwrap_or_default().to_string();
    answer["data"][0]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| Entry { short: field(entry, 0), tag: field(entry, 1), long: field(entry, 2) })
        .filter(|entry| !entry.short.is_empty())
        .collect()
}

/// From busctl's JSON of `getLayout`: `{"type":"u","data":[0]}`.
fn plasma_index(answer: &Value) -> Option<usize> {
    answer["data"][0].as_u64().map(|i| i as usize)
}

fn plasma_switch(index: usize) -> bool {
    busctl(&["setLayout", "u", &index.to_string()]).is_some_and(|answer| answer["data"][0].as_bool() != Some(false))
}

fn cosmic_config() -> PathBuf {
    config::config_home().join("cosmic/com.system76.CosmicComp/v1/xkb_config")
}

/// XKB's names for layouts and variants. NixOS has no /usr/share.
fn xkb_list() -> PathBuf {
    ["/usr/share/X11/xkb/rules/evdev.lst", "/run/current-system/sw/share/X11/xkb/rules/evdev.lst"]
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .unwrap_or_default()
}

/// A string field of COSMIC's RON, `layout: "us,fr"`: where its value is, and it.
fn ron_string<'a>(text: &'a str, key: &str) -> Option<(std::ops::Range<usize>, &'a str)> {
    let mut from = 0;
    while let Some(at) = text[from..].find(key).map(|at| at + from) {
        from = at + key.len();
        let starts_word = text[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        let rest = text[from..].trim_start();
        let Some(rest) = rest.strip_prefix(':').map(str::trim_start).filter(|_| starts_word) else { continue };
        let Some(value) = rest.strip_prefix('"') else { continue };
        let start = text.len() - value.len();
        let end = start + value.find('"')?;
        return Some((start..end, &text[start..end]));
    }
    None
}

/// `layout: "us,fr"` and `variant: "intl,"`, named from XKB's list.
fn cosmic_entries(text: &str, names: &str) -> Vec<Entry> {
    let Some((_, layouts)) = ron_string(text, "layout") else { return Vec::new() };
    let variants: Vec<&str> = ron_string(text, "variant").map_or(Vec::new(), |(_, v)| v.split(',').collect());
    layouts
        .split(',')
        .enumerate()
        .map(|(i, short)| {
            let tag = variants.get(i).copied().unwrap_or("").trim();
            let short = short.trim();
            Entry { short: short.into(), tag: tag.into(), long: xkb_name(names, short, tag).unwrap_or_else(|| short.into()) }
        })
        .filter(|entry| !entry.short.is_empty())
        .collect()
}

/// From evdev.lst: `  fr   French` under `! layout`, `  intl   us: English (US, intl.)` under `! variant`.
fn xkb_name(names: &str, short: &str, variant: &str) -> Option<String> {
    let mut section = "";
    for line in names.lines() {
        if let Some(name) = line.strip_prefix('!') {
            section = name.trim();
            continue;
        }
        let Some((key, value)) = line.trim().split_once(char::is_whitespace) else { continue };
        let value = value.trim();
        let found = match (section, variant.is_empty()) {
            ("layout", true) => (key == short).then_some(value),
            ("variant", false) => (key == variant).then(|| value.strip_prefix(&format!("{short}:"))).flatten(),
            _ => None,
        };
        if let Some(found) = found {
            return Some(found.trim().to_string());
        }
    }
    None
}

/// The config with this layout, and its variant, moved first.
fn cosmic_reorder(text: &str, entry: &Entry) -> Option<String> {
    let (layout_at, layouts) = ron_string(text, "layout")?;
    let mut layouts: Vec<&str> = layouts.split(',').map(str::trim).collect();
    let variant = ron_string(text, "variant");
    let mut variants: Vec<&str> = variant.as_ref().map_or(Vec::new(), |(_, v)| v.split(',').map(str::trim).collect());
    variants.resize(layouts.len(), "");
    let at = layouts.iter().zip(&variants).position(|(l, v)| *l == entry.short && *v == entry.tag)?;
    let layout = layouts.remove(at);
    layouts.insert(0, layout);
    let moved = variants.remove(at);
    variants.insert(0, moved);
    let mut text = text.to_string();
    let mut edits = vec![(layout_at, layouts.join(","))];
    if let Some((variant_at, _)) = variant {
        edits.push((variant_at, variants.join(",")));
    }
    // The later one first, so the earlier one's place doesn't move.
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, value) in edits {
        text.replace_range(range, &value);
    }
    Some(text)
}

/// Written whole and renamed into place, as COSMIC does itself.
fn cosmic_switch(entry: &Entry) -> bool {
    let path = cosmic_config();
    let Some(text) = std::fs::read_to_string(&path).ok().and_then(|text| cosmic_reorder(&text, entry)) else { return false };
    let temporary = path.with_extension("genie");
    std::fs::write(&temporary, text).is_ok() && std::fs::rename(&temporary, &path).is_ok()
}

/// What's typed matches the short name, as `fr`, or the long one, as `french`.
/// Not two letters in the long name, or `de` would find `with dead keys`.
/// Typing nothing puts the next layout on top and the current one last.
/// Typing keeps the order they're set up in.
fn items(entries: &[Entry], current: Option<usize>, typed: &str) -> Vec<Item> {
    let typed = typed.trim();
    let count = entries.len();
    entries
        .iter()
        .enumerate()
        .filter_map(|(i, entry)| {
            let is_current = current == Some(i);
            // How far after the current one: the next is 1, the current one `count`.
            let after = current.map_or(i + 1, |current| (i + count - current - 1) % count + 1);
            let (score, tier) = if typed.is_empty() {
                (1.0 - after as f32 * 0.01, if after == 1 { 0 } else { 1 })
            } else {
                let mut fields = vec![(entry.short.as_str(), 1.0), (entry.tag.as_str(), 0.8)];
                if typed.chars().count() > 2 {
                    fields.push((entry.long.as_str(), 0.6));
                }
                (matcher::score(typed, &fields)? - i as f32 * 0.001, 0)
            };
            let title = if entry.tag.is_empty() || entry.tag == entry.short { entry.short.clone() } else { format!("{} ({})", entry.short, entry.tag) };
            let subtitle = if is_current { format!("{}  ·  Current", entry.long) } else { entry.long.clone() };
            let chosen = entry.clone();
            let call = Effect::Call(Arc::new(move || switch(i, chosen.clone())));
            Some(
                Item::new(format!("layout:{}:{}", entry.short, entry.tag), title, subtitle, Icon::Theme(ICON.into()))
                    .action(Action::new("switch", "Switch", call))
                    .score(score)
                    .tier(tier),
            )
        })
        .collect()
}

impl Plugin for Layout {
    fn id(&self) -> &'static str {
        "layout"
    }

    fn group(&self) -> &'static str {
        "System"
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command::new("layout", &["kb"], "Switch the keyboard layout", ICON)]
    }

    fn query(&self, query: &Query) {
        let (entries, current) = match layouts() {
            Ok(found) => found,
            Err(why) => return query.send(vec![Item::new("layout:none", "No keyboard layouts to switch", why, Icon::Theme(ICON.into()))]),
        };
        let found = items(&entries, current, &query.text);
        if found.is_empty() {
            let title = format!("{} isn't one of your layouts", query.text.trim());
            query.send(vec![Item::new("layout:missing", title, "Add it in the keyboard settings", Icon::Theme(ICON.into()))]);
        } else {
            query.send(found);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured() -> Vec<Entry> {
        let answer = r#"{"type":"a(sss)","data":[[["us","","English (US)"],["fr","","French"],["us","intl","English (US, intl., with dead keys)"]]]}"#;
        plasma_entries(&serde_json::from_str(answer).unwrap())
    }

    fn titles(current: Option<usize>, typed: &str) -> Vec<String> {
        let mut items = items(&configured(), current, typed);
        items.sort_by(|a, b| a.tier.cmp(&b.tier).then(b.score.total_cmp(&a.score)));
        items.into_iter().map(|i| i.title).collect()
    }

    #[test]
    fn busctl_json_is_read() {
        let entries = configured();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[2], Entry { short: "us".into(), tag: "intl".into(), long: "English (US, intl., with dead keys)".into() });
        assert_eq!(plasma_index(&serde_json::from_str(r#"{"type":"u","data":[1]}"#).unwrap()), Some(1));
    }

    #[test]
    fn nothing_typed_puts_the_next_layout_on_top() {
        assert_eq!(titles(Some(0), ""), ["fr", "us (intl)", "us"]);
        assert_eq!(titles(Some(2), ""), ["us", "fr", "us (intl)"]);
        assert_eq!(items(&configured(), Some(0), "")[0].subtitle, "English (US)  ·  Current");
    }

    #[test]
    fn layouts_are_found_by_short_and_long_name() {
        assert_eq!(titles(Some(0), "fr"), ["fr"]);
        assert_eq!(titles(Some(0), "french"), ["fr"]);
        assert_eq!(titles(Some(1), "us")[0], "us");
        assert_eq!(titles(Some(1), "intl"), ["us (intl)"]);
        assert!(titles(Some(0), "de").is_empty(), "German isn't set up, whatever `dead keys` says");
        assert_eq!(titles(Some(0), "eng"), ["us", "us (intl)"]);
    }

    const COSMIC: &str = r#"(
    rules: "",
    model: "pc105",
    layout: "us,fr,us",
    variant: ",,intl",
    options: Some("grp:win_space_toggle"),
    repeat_delay: 600,
    repeat_rate: 25,
)"#;

    const NAMES: &str = "! model\n  pc105           Generic 105-key PC\n\n! layout\n  us              English (US)\n  fr              French\n\n! variant\n  intl            us: English (US, intl., with dead keys)\n  azerty          fr: French (AZERTY)\n";

    #[test]
    fn cosmic_layouts_are_read_and_named() {
        let entries = cosmic_entries(COSMIC, NAMES);
        let read: Vec<(&str, &str, &str)> = entries.iter().map(|e| (e.short.as_str(), e.tag.as_str(), e.long.as_str())).collect();
        assert_eq!(read, [("us", "", "English (US)"), ("fr", "", "French"), ("us", "intl", "English (US, intl., with dead keys)")]);
        let unnamed = cosmic_entries("(layout: \"de\")", "");
        assert_eq!(unnamed, [Entry { short: "de".into(), tag: "".into(), long: "de".into() }]);
    }

    #[test]
    fn switching_on_cosmic_moves_the_layout_first() {
        let entries = cosmic_entries(COSMIC, NAMES);
        let moved = cosmic_reorder(COSMIC, &entries[2]).unwrap();
        assert!(moved.contains("layout: \"us,us,fr\",\n    variant: \"intl,,\","), "{moved}");
        assert!(moved.contains("options: Some(\"grp:win_space_toggle\")"));
        let french = cosmic_reorder(COSMIC, &entries[1]).unwrap();
        assert!(french.contains("layout: \"fr,us,us\",\n    variant: \",,intl\","), "{french}");
        // No variants at all.
        let plain = "(layout: \"us,fr\", model: \"pc105\")";
        assert_eq!(cosmic_reorder(plain, &cosmic_entries(plain, "")[1]).unwrap(), "(layout: \"fr,us\", model: \"pc105\")");
    }
}
