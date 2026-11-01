//! Your own commands, from ~/.config/genie/commands.toml.

use crate::config::CustomCommand;
use crate::plugin::Action;
use crate::plugin::Command;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;

pub struct Custom(pub Vec<CustomCommand>);

fn action(entry: &CustomCommand) -> Action {
    let spawn = Effect::Spawn { argv: entry.command.clone(), cwd: None, id: format!("genie-{}", entry.name) };
    Action::new("run", &entry.title, spawn)
}

impl Plugin for Custom {
    fn id(&self) -> &'static str {
        "custom"
    }

    fn group(&self) -> &'static str {
        "Custom"
    }

    fn commands(&self) -> Vec<Command> {
        self.0
            .iter()
            .map(|entry| {
                let command = Command::new(&entry.name, &[], &entry.title, &entry.icon).action(action(entry));
                match &entry.shortcut {
                    Some(shortcut) => command.shortcut(shortcut),
                    None => command,
                }
            })
            .collect()
    }

    fn query(&self, query: &Query) {
        let found = self.0.iter().find(|entry| query.command.as_deref() == Some(entry.name.as_str()));
        let item = found.map(|entry| {
            let shortcut = entry.shortcut.as_deref().unwrap_or("");
            Item::new(format!("custom:{}", entry.name), &entry.title, shortcut, Icon::Theme(entry.icon.clone())).action(action(entry))
        });
        query.send(item.into_iter().collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_shows_its_shortcut_only_if_it_has_one() {
        let entry = |shortcut: Option<&str>| CustomCommand {
            name: "record-start".into(),
            title: "Recording: start".into(),
            icon: "media-record".into(),
            command: vec!["gsr-start".into()],
            shortcut: shortcut.map(String::from),
        };
        let commands = Custom(vec![entry(Some("Ctrl+Alt+Home")), entry(None)]).commands();
        assert_eq!(commands[0].detail(), "Recording: start  ·  Ctrl+Alt+Home");
        assert_eq!(commands[1].detail(), "Recording: start");
        assert!(commands.iter().all(|c| c.action.is_some()));
    }
}
