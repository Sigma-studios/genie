//! `zd name`: Zed's recent workspaces, from its own database, most recent
//! first. A schema change upstream shows up as an item saying so.

use crate::config;
use crate::launch;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use std::path::Path;

pub struct Zed {
    terminal: Vec<String>,
}

impl Zed {
    pub fn new(terminal: Vec<String>) -> Self {
        Self { terminal }
    }
}

struct Workspace {
    id: i64,
    /// Zed keeps one path per line, for a workspace with several folders.
    roots: Vec<String>,
}

fn workspaces() -> Result<Vec<Workspace>, rusqlite::Error> {
    let path = config::data_home().join("zed/db/0-stable/db.sqlite");
    let db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut statement = db.prepare(
        "SELECT workspace_id, paths FROM workspaces
         WHERE paths IS NOT NULL AND paths != '' AND remote_connection_id IS NULL
         ORDER BY timestamp DESC",
    )?;
    let rows = statement.query_map([], |row| {
        let paths: String = row.get(1)?;
        Ok(Workspace { id: row.get(0)?, roots: split_roots(&paths) })
    })?;
    Ok(rows.filter_map(Result::ok).filter(|w| w.roots.iter().any(|r| Path::new(r).exists())).collect())
}

fn split_roots(paths: &str) -> Vec<String> {
    paths.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect()
}

fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl Plugin for Zed {
    fn id(&self) -> &'static str {
        "zed"
    }

    fn triggers(&self) -> Vec<String> {
        vec!["zd ".into()]
    }

    fn global(&self) -> bool {
        false
    }

    fn query(&self, query: &Query) {
        let Some(zed) = launch::which(&["zeditor", "zed"]) else {
            query.send(vec![Item::new("zed.missing", "Zed isn't installed", "", Icon::Theme("dialog-warning".into())).score(1.0)]);
            return;
        };
        let workspaces = match workspaces() {
            Ok(workspaces) => workspaces,
            Err(e) => {
                let item = Item::new("zed.error", "Couldn't read Zed's workspaces", e.to_string(), Icon::Theme("dialog-warning".into()));
                query.send(vec![item.score(1.0)]);
                return;
            }
        };
        let text = query.text.trim();
        let count = workspaces.len().max(1) as f32;
        let items = workspaces.iter().enumerate().filter_map(|(i, workspace)| {
            let name = workspace.roots.iter().map(|r| base_name(r)).collect::<Vec<_>>().join(", ");
            let path = workspace.roots.join(", ");
            let score = if text.is_empty() { 0.5 } else { matcher::score(text, &[(&name, 1.0), (&path, 0.6)])? };
            // Most recent first among equals.
            let recency = (count - i as f32) / count * 0.1;
            let first = workspace.roots[0].clone();
            let open = Effect::Spawn {
                argv: std::iter::once(zed.clone()).chain(workspace.roots.iter().cloned()).collect(),
                cwd: None,
                id: "dev.zed.Zed".into(),
            };
            let shell = Effect::Spawn {
                argv: self.terminal.iter().cloned().chain(std::iter::once("fish".to_string())).collect(),
                cwd: Some(first.clone().into()),
                id: "genie-terminal".into(),
            };
            Some(
                Item::new(workspace.id.to_string(), &name, &path, Icon::Theme("zed".into()))
                    .action(Action::new("open", "Open in Zed", open))
                    .action(Action::new("terminal", "Open a terminal there", shell))
                    .action(Action::new("folder", "Open the folder", Effect::Open(first)))
                    .completion(name.clone())
                    .score(score + recency),
            )
        });
        query.send(items.collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_roots() {
        assert_eq!(split_roots("/a/b\n/c\n"), ["/a/b", "/c"]);
        assert_eq!(base_name("/a/b"), "b");
    }
}
