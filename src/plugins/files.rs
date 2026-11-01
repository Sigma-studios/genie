//! `f name`: every file by name, through the répertoire daemon, which answers
//! in milliseconds (newline-delimited JSON, see docs/PROTOCOL.md there).
//!
//! Queries with a slash are paths. Every segment before the last slash is an
//! exact folder name and only the last one matches loosely, so `genie/s` lists
//! what's in folders named genie that matches "s". The first folder is found
//! by a search, the rest by listing one level at a time. A query starting with
//! / or ~ is a real path and is listed from the filesystem. Tab on a folder
//! completes it with a slash, to go one level down.

use crate::config;
use crate::plugin::Action;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use base64::Engine;
use serde_json::Value;
use serde_json::json;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Write;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

const LIMIT: usize = 40;
/// Folders followed per path segment. Only exact names are followed, so this
/// only bites on common names (`src/`, `target/`).
const ANCHORS: usize = 20;
/// Entries asked for per listed folder.
const LISTING: usize = 5000;

pub struct Files;

#[derive(Debug, Clone, PartialEq)]
struct Hit {
    path: String,
    kind: String,
    bytes: Option<u64>,
    rank: Rank,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    Exact,
    Prefix,
    Word,
    Substring,
}

impl Rank {
    fn parse(name: &str) -> Self {
        match name {
            "exact" => Self::Exact,
            "prefix" => Self::Prefix,
            "word" => Self::Word,
            _ => Self::Substring,
        }
    }

    fn score(self) -> f32 {
        match self {
            Self::Exact => 1.0,
            Self::Prefix => 0.8,
            Self::Word => 0.6,
            Self::Substring => 0.4,
        }
    }
}

struct Daemon {
    writer: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Daemon {
    fn connect(timeout: Duration) -> Result<Self, String> {
        let socket = std::env::var("REPERTOIRE_SOCKET").unwrap_or_else(|_| "/run/repertoire/repertoire.sock".into());
        let writer = UnixStream::connect(socket).map_err(|e| format!("répertoire isn't running ({e})"))?;
        writer.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        writer.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
        let reader = BufReader::new(writer.try_clone().map_err(|e| e.to_string())?);
        Ok(Self { writer, reader })
    }

    fn request(&mut self, mut message: Value) -> Result<Value, String> {
        message["v"] = json!(1);
        let mut line = message.to_string();
        line.push('\n');
        self.writer.write_all(line.as_bytes()).map_err(|e| format!("répertoire isn't answering ({e})"))?;
        let mut answer = String::new();
        self.reader.read_line(&mut answer).map_err(|e| format!("répertoire isn't answering ({e})"))?;
        let answer: Value = serde_json::from_str(&answer).map_err(|_| "unexpected answer from répertoire".to_string())?;
        if answer["type"] == "Error" {
            return Err(answer["message"].as_str().unwrap_or("error").to_string());
        }
        Ok(answer)
    }

    /// (path, kind, bytes) of what's in a folder.
    fn children(&mut self, folder: &str, files: bool) -> Result<Vec<(String, String, Option<u64>)>, String> {
        let answer = self.request(json!({
            "type": "Children", "path": folder, "files": files, "sort": "size", "limit": LISTING,
        }))?;
        let join = |name: String| format!("{}/{}", folder.trim_end_matches('/'), name);
        let mut out: Vec<_> = list(&answer["dirs"])
            .iter()
            .map(|d| (join(wire_path(d, "name")), "dir".to_string(), d["total"].as_u64()))
            .collect();
        out.extend(list(&answer["files"]).iter().map(|f| {
            (join(wire_path(f, "name")), f["kind"].as_str().unwrap_or("file").to_string(), f["bytes"].as_u64())
        }));
        Ok(out)
    }
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

/// Paths are plain strings, or base64 bytes when not valid UTF-8. Those get
/// lossy text here: they display, though they won't open.
fn wire_path(object: &Value, field: &str) -> String {
    if let Some(text) = object[field].as_str() {
        return text.to_string();
    }
    let encoded = object[format!("{field}_b64")].as_str().unwrap_or("");
    let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap_or_default();
    std::ffi::OsString::from_vec(bytes).to_string_lossy().into_owned()
}

fn word_start(name: &str, at: usize) -> bool {
    let Some(a) = name[..at].chars().next_back() else { return true };
    let Some(b) = name[at..].chars().next() else { return false };
    (a.is_ascii() && !a.is_alphanumeric()) || (a.is_lowercase() && b.is_uppercase()) || (a.is_alphabetic() && b.is_numeric())
}

/// How `query` matches `name`, like the daemon's Search ranks it.
fn rank(name: &str, query: &str) -> Option<Rank> {
    let (lower, query) = (name.to_lowercase(), query.to_lowercase());
    if query.is_empty() {
        return Some(Rank::Prefix);
    }
    // Lowercasing can change lengths; word starts are only checked if it didn't.
    let same_layout = lower.len() == name.len();
    let mut best = None;
    for (at, _) in lower.match_indices(&query) {
        if at == 0 {
            return Some(if query.len() == lower.len() { Rank::Exact } else { Rank::Prefix });
        }
        if same_layout && name.is_char_boundary(at) && word_start(name, at) {
            return Some(Rank::Word);
        }
        best = Some(Rank::Substring);
    }
    best
}

/// Daemon order: rank, then shorter names, then larger sizes. With nothing
/// after the slash it's a listing, so just largest first.
fn best_first(hits: &mut [Hit], query: &str) {
    if query.is_empty() {
        hits.sort_by_key(|h| std::cmp::Reverse(h.bytes.unwrap_or(0)));
    } else {
        hits.sort_by_key(|h| (h.rank, base_name(&h.path).len(), std::cmp::Reverse(h.bytes.unwrap_or(0))));
    }
}

fn base_name(path: &str) -> &str {
    Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or(path)
}

fn search(query: &str) -> Result<Vec<Hit>, String> {
    let mut daemon = Daemon::connect(Duration::from_secs(2))?;
    let answer = daemon.request(json!({"type": "Search", "query": query, "limit": LIMIT}))?;
    Ok(list(&answer["results"])
        .iter()
        .map(|r| Hit {
            path: wire_path(r, "path"),
            kind: r["kind"].as_str().unwrap_or("file").to_string(),
            bytes: r["bytes"].as_u64(),
            rank: Rank::parse(r["rank"].as_str().unwrap_or("")),
        })
        .collect())
}

/// `a/b/c`: folders named a (anywhere), then b directly inside, then whatever
/// in those matches c.
fn search_path(query: &str) -> Result<Vec<Hit>, String> {
    let (folders, last) = query.rsplit_once('/').unwrap_or(("", query));
    let mut folders = folders.split('/').filter(|f| !f.is_empty());
    let Some(first) = folders.next() else { return Ok(Vec::new()) };
    let mut daemon = Daemon::connect(Duration::from_secs(2))?;
    let answer = daemon.request(json!({"type": "Search", "query": first, "limit": ANCHORS, "kinds": ["dir"]}))?;
    let mut found: Vec<String> = list(&answer["results"])
        .iter()
        .filter(|r| r["rank"] == "exact")
        .map(|r| wire_path(r, "path"))
        .collect();
    for name in folders {
        let name = name.to_lowercase();
        let mut next = Vec::new();
        for folder in &found {
            next.extend(
                daemon.children(folder, false)?.into_iter().map(|c| c.0).filter(|p| base_name(p).to_lowercase() == name),
            );
        }
        next.truncate(ANCHORS);
        found = next;
    }
    let mut hits = Vec::new();
    for folder in &found {
        for (path, kind, bytes) in daemon.children(folder, true)? {
            if let Some(rank) = rank(base_name(&path), last) {
                hits.push(Hit { path, kind, bytes, rank });
            }
        }
    }
    best_first(&mut hits, last);
    hits.truncate(LIMIT);
    Ok(hits)
}

fn expand_home(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", config::home().display()),
        _ => path.to_string(),
    }
}

/// `/x/y/z` or `~/y/z`: what's in /x/y (or ~/y) matching z, from the
/// filesystem. Folder totals come from the daemon when it runs.
fn list_real(query: &str) -> Vec<Hit> {
    let (base, last) = query.rsplit_once('/').unwrap_or((query, ""));
    let folder = expand_home(if base.is_empty() { "/" } else { base });
    let Ok(entries) = std::fs::read_dir(&folder) else { return Vec::new() };
    let totals: std::collections::HashMap<String, Option<u64>> = Daemon::connect(Duration::from_millis(500))
        .and_then(|mut d| d.children(&folder, false))
        .map(|children| children.into_iter().map(|(p, _, b)| (p, b)).collect())
        .unwrap_or_default();
    let mut hits = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rank) = name.to_str().and_then(|n| rank(n, last)) else { continue };
        let path = entry.path().to_string_lossy().into_owned();
        let Ok(kind) = entry.file_type() else { continue };
        let (kind, bytes) = if kind.is_dir() {
            ("dir", totals.get(&path).copied().flatten())
        } else {
            use std::os::unix::fs::MetadataExt;
            // Allocated, as the daemon counts.
            let bytes = entry.metadata().ok().map(|m| m.blocks() * 512);
            (if kind.is_symlink() { "symlink" } else { "file" }, bytes)
        };
        hits.push(Hit { path, kind: kind.to_string(), bytes, rank });
    }
    best_first(&mut hits, last);
    hits.truncate(LIMIT);
    hits
}

fn human(bytes: u64) -> String {
    let mut n = bytes as f64;
    for unit in ["B", "KiB", "MiB", "GiB"] {
        if n < 1024.0 {
            return format!("{n:.0} {unit}");
        }
        n /= 1024.0;
    }
    format!("{n:.0} TiB")
}

fn file_icon(path: &str, kind: &str) -> Icon {
    if kind == "dir" {
        return Icon::Theme("folder".into());
    }
    let name = mime_guess::from_path(path).first().map_or_else(
        || "text-x-generic".to_string(),
        |mime| format!("{}-{}", mime.type_(), mime.subtype()),
    );
    Icon::Theme(name)
}

fn make_item(hit: Hit, typed: &str, position: usize) -> Item {
    let dir = hit.kind == "dir";
    let name = base_name(&hit.path).to_string();
    let parent = Path::new(&hit.path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let subtitle = hit.bytes.map_or_else(|| parent.clone(), |b| format!("{} · {parent}", human(b)));
    let slash = if dir { "/" } else { "" };
    let mut item = Item::new(&hit.path, format!("{name}{slash}"), subtitle, file_icon(&hit.path, &hit.kind))
        .action(Action::new("open", "Open", Effect::Open(hit.path.clone())));
    if !dir {
        item = item.action(Action::new("folder", "Open the containing folder", Effect::Open(parent)));
    }
    // Keeps the daemon's order among results of the same rank.
    item.action(Action::new("copy", "Copy the path", Effect::Copy(hit.path)))
        .completion(format!("f {typed}{name}{slash}"))
        .score(hit.rank.score() - position as f32 * 0.001)
}

impl Plugin for Files {
    fn id(&self) -> &'static str {
        "files"
    }

    fn triggers(&self) -> Vec<String> {
        vec!["f ".into()]
    }

    fn global(&self) -> bool {
        false
    }

    fn query(&self, query: &Query) {
        let text = query.text.trim();
        let typed = text.rsplit_once('/').map_or(String::new(), |(base, _)| format!("{base}/"));
        let hits = if text.starts_with(['/', '~']) {
            Ok(list_real(text))
        } else if text.contains('/') {
            search_path(text)
        } else if text.chars().count() >= 2 {
            search(text)
        } else {
            return;
        };
        match hits {
            Ok(hits) => query.send(hits.into_iter().enumerate().map(|(i, h)| make_item(h, &typed, i)).collect()),
            Err(error) => {
                query.send(vec![Item::new("files.error", "Répertoire", error, Icon::Theme("dialog-warning".into())).score(1.0)]);
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_like_the_daemon() {
        assert_eq!(rank("main.rs", "main.rs"), Some(Rank::Exact));
        assert_eq!(rank("main.rs", "MAIN"), Some(Rank::Prefix));
        assert_eq!(rank("my_file", "file"), Some(Rank::Word));
        assert_eq!(rank("myFile", "file"), Some(Rank::Word));
        assert_eq!(rank("profile", "file"), Some(Rank::Substring));
        assert_eq!(rank("main.rs", "zzz"), None);
        assert_eq!(rank("anything", ""), Some(Rank::Prefix));
    }

    #[test]
    fn non_ascii_names_dont_panic() {
        assert_eq!(rank("répertoire", "pert"), Some(Rank::Substring));
        assert_eq!(rank("İstanbul", "stan"), Some(Rank::Substring));
    }

    #[test]
    fn listings_sort_by_size_and_searches_by_rank() {
        let hit = |path: &str, bytes, rank| Hit { path: path.into(), kind: "file".into(), bytes: Some(bytes), rank };
        let mut hits = vec![hit("/a/longer", 9, Rank::Substring), hit("/a/b", 1, Rank::Prefix)];
        best_first(&mut hits, "x");
        assert_eq!(hits[0].path, "/a/b");
        best_first(&mut hits, "");
        assert_eq!(hits[0].path, "/a/longer");
    }

    #[test]
    fn sizes_and_mime_icons() {
        assert_eq!(human(512), "512 B");
        assert_eq!(human(2048), "2 KiB");
        assert_eq!(file_icon("/x/a.pdf", "file"), Icon::Theme("application-pdf".into()));
        assert_eq!(file_icon("/x/a", "dir"), Icon::Theme("folder".into()));
    }

    #[test]
    fn wire_paths_can_be_bytes() {
        assert_eq!(wire_path(&json!({"path": "/a"}), "path"), "/a");
        assert_eq!(wire_path(&json!({"path_b64": "L2E="}), "path"), "/a");
    }
}
