//! Shut down, restart, log out, lock.

use crate::config::PowerEntry;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;

pub struct Power(pub Vec<PowerEntry>);

impl Plugin for Power {
    fn id(&self) -> &'static str {
        "power"
    }

    fn query(&self, query: &Query) {
        let count = self.0.len() as f32;
        query.send(
            self.0
                .iter()
                .enumerate()
                .filter_map(|(i, entry)| {
                    matcher::score(&query.text, &[(&entry.title, 1.0)])?;
                    let spawn = Effect::Spawn {
                        argv: entry.command.clone(),
                        cwd: None,
                        id: "genie-power".into(),
                    };
                    // Above other matches, and in the menu's order: it's a menu.
                    let order = (count - i as f32) * 0.01;
                    Some(
                        Item::new(&entry.id, &entry.title, "", Icon::Theme(entry.icon.clone()))
                            .action(Action::new("run", "Run", spawn))
                            .score(2.0 + order),
                    )
                })
                .collect(),
        );
    }
}
