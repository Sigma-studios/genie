//! `/maps paris` searches Google Maps. Ecosia also comes last in the regular
//! search.

use crate::config::Engine;
use crate::plugin::Action;
use crate::plugin::Command;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::LAST_TIER;
use crate::plugin::Plugin;
use crate::plugin::Query;
use percent_encoding::NON_ALPHANUMERIC;
use percent_encoding::utf8_percent_encode;

const ICON: &str = "internet-web-browser";
pub struct WebSearch(pub Vec<Engine>);

fn url(engine: &Engine, text: &str) -> String {
    engine.url.replace("%s", &utf8_percent_encode(text, NON_ALPHANUMERIC).to_string())
}

fn icon(engine: &Engine) -> Icon {
    Icon::Theme(engine.icon.clone().unwrap_or_else(|| ICON.into()))
}

fn search(engine: &Engine, text: &str) -> Item {
    Item::new(&engine.id, format!("{}: {text}", engine.name), format!("Search {}", engine.name), icon(engine))
        .action(Action::new("run", "Search", Effect::Open(url(engine, text))))
}

impl Plugin for WebSearch {
    fn id(&self) -> &'static str {
        "websearch"
    }

    fn group(&self) -> &'static str {
        "Web search"
    }

    fn commands(&self) -> Vec<Command> {
        self.0
            .iter()
            .map(|engine| {
                let aliases: Vec<&str> = engine.alias.iter().map(String::as_str).collect();
                let description = format!("Search {}", engine.name);
                Command::new(&engine.command, &aliases, &description, engine.icon.as_deref().unwrap_or(ICON))
            })
            .collect()
    }

    fn regular(&self) -> bool {
        true
    }

    fn query(&self, query: &Query) {
        let text = query.text.trim();
        let Some(command) = &query.command else {
            if !text.is_empty() {
                query.send(self.0.iter().filter(|e| e.regular).map(|e| search(e, text).tier(LAST_TIER)).collect());
            }
            return;
        };
        let Some(engine) = self.0.iter().find(|e| e.command == *command) else { return };
        if text.is_empty() {
            let hint = format!("Type what to search on {}", engine.name);
            query.send(vec![Item::new(format!("{}.hint", engine.id), &engine.name, hint, icon(engine)).completion(format!("/{command} "))]);
        } else {
            query.send(vec![search(engine, text)]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn translate_goes_to_french() {
        let engines = crate::config::Config::default().websearch;
        let translate = engines.iter().find(|e| e.command == "translate").unwrap();
        assert_eq!(url(translate, "good morning"), "https://translate.google.com/?sl=auto&tl=fr&text=good%20morning&op=translate");
    }

    #[test]
    fn queries_are_encoded() {
        let engine = Engine {
            id: "x".into(),
            name: "X".into(),
            command: "x".into(),
            alias: None,
            url: "https://x.org/?q=%s".into(),
            regular: false,
            icon: None,
        };
        assert_eq!(url(&engine, "é & b"), "https://x.org/?q=%C3%A9%20%26%20b");
    }
}
