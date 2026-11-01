//! `gg something` searches Google. Typing an engine's name completes its
//! trigger, and some engines are offered when nothing else matched.

use crate::config::Engine;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use percent_encoding::NON_ALPHANUMERIC;
use percent_encoding::utf8_percent_encode;

pub struct WebSearch(pub Vec<Engine>);

fn url(engine: &Engine, text: &str) -> String {
    engine.url.replace("%s", &utf8_percent_encode(text, NON_ALPHANUMERIC).to_string())
}

fn icon(engine: &Engine) -> Icon {
    Icon::Theme(engine.icon.clone().unwrap_or_else(|| "internet-web-browser".into()))
}

fn search(engine: &Engine, text: &str) -> Item {
    let subtitle = format!("Search {}", engine.name);
    Item::new(&engine.id, format!("{}: {text}", engine.name), subtitle, icon(engine))
        .action(Action::new("run", "Search", Effect::Open(url(engine, text))))
}

impl Plugin for WebSearch {
    fn id(&self) -> &'static str {
        "websearch"
    }

    fn triggers(&self) -> Vec<String> {
        self.0.iter().map(|e| format!("{} ", e.trigger)).collect()
    }

    fn query(&self, query: &Query) {
        if let Some(trigger) = &query.trigger {
            let text = query.text.trim();
            let Some(engine) = self.0.iter().find(|e| format!("{} ", e.trigger) == *trigger) else { return };
            if !text.is_empty() {
                query.send(vec![search(engine, text).score(1.0)]);
            }
            return;
        }
        query.send(
            self.0
                .iter()
                .filter_map(|engine| {
                    let score = matcher::score(&query.text, &[(&engine.name, 1.0), (&engine.trigger, 1.0)])?;
                    let subtitle = format!("Type “{} ” to search", engine.trigger);
                    Some(
                        Item::new(format!("{}.engine", engine.id), &engine.name, subtitle, icon(engine))
                            .completion(format!("{} ", engine.trigger))
                            .score(score * 0.8),
                    )
                })
                .collect(),
        );
    }

    fn fallbacks(&self, text: &str) -> Vec<Item> {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        self.0.iter().filter(|e| e.fallback).map(|e| search(e, text)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_are_encoded() {
        let engine = Engine {
            id: "x".into(),
            name: "X".into(),
            trigger: "x".into(),
            url: "https://x.org/?q=%s".into(),
            fallback: false,
            icon: None,
        };
        assert_eq!(url(&engine, "é & b"), "https://x.org/?q=%C3%A9%20%26%20b");
    }
}
