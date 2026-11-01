//! `/shutdown`, `/restart`, `/logout`, `/lock`.

use crate::config::PowerEntry;
use crate::plugin::Action;
use crate::plugin::Command;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;

pub struct Power(pub Vec<PowerEntry>);

fn action(entry: &PowerEntry) -> Action {
    let spawn = Effect::Spawn { argv: entry.command.clone(), cwd: None, id: "genie-power".into() };
    let action = Action::new("run", &entry.title, spawn);
    if entry.confirm { action.confirm(format!("{}?", entry.title)) } else { action }
}

impl Plugin for Power {
    fn id(&self) -> &'static str {
        "power"
    }

    fn group(&self) -> &'static str {
        "System"
    }

    fn commands(&self) -> Vec<Command> {
        self.0
            .iter()
            .map(|entry| Command::new(&entry.name, &[], &format!("Power: {}", entry.title), &entry.icon).action(action(entry)))
            .collect()
    }

    fn query(&self, query: &Query) {
        let found = self.0.iter().find(|entry| query.command.as_deref() == Some(entry.name.as_str()));
        query.send(found.map(|entry| Item::new(&entry.id, &entry.title, "", Icon::Theme(entry.icon.clone())).action(action(entry))).into_iter().collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn every_entry_is_a_command_that_power_finds() {
        let power = Power(Config::default().power);
        let commands = power.commands();
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["shutdown", "restart", "logout", "lock"]);
        assert!(commands.iter().all(|c| c.description.starts_with("Power:") && c.action.is_some()));
        let asks: Vec<bool> = commands.iter().map(|c| c.action.as_ref().unwrap().confirm.is_some()).collect();
        assert_eq!(asks, [true, true, true, false], "all but locking the screen ask first");
    }
}
