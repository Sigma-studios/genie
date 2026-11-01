//! Shut down, restart, log out, lock.

use crate::config::PowerEntry;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Command;
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

    fn group(&self) -> &'static str {
        "System"
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command::new("power", &[], "Shut down, restart, log out or lock", "system-shutdown")]
    }

    fn query(&self, query: &Query) {
        let count = self.0.len() as f32;
        query.send(
            self.0
                .iter()
                .enumerate()
                .filter_map(|(i, entry)| {
                    if !query.text.trim().is_empty() {
                        matcher::score(&query.text, &[(&entry.title, 1.0)])?;
                    }
                    let spawn = Effect::Spawn {
                        argv: entry.command.clone(),
                        cwd: None,
                        id: "genie-power".into(),
                    };
                    // In the menu's order, however often each was used: it's a menu.
                    let order = (count - i as f32) * 10.0;
                    Some(
                        Item::new(&entry.id, &entry.title, "", Icon::Theme(entry.icon.clone()))
                            .action(Action::new("run", "Run", spawn))
                            .score(order),
                    )
                })
                .collect(),
        );
    }
}
