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

/// What runs: the command, plus what was typed after its name when it takes
/// arguments.
fn argv(entry: &CustomCommand, typed: &str) -> Vec<String> {
    let mut argv = entry.command.clone();
    if entry.arguments {
        argv.extend(typed.split_whitespace().map(String::from));
    }
    argv
}

fn action(entry: &CustomCommand, argv: Vec<String>) -> Action {
    let spawn = Effect::Spawn { argv, cwd: None, id: format!("genie-{}", entry.name) };
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
                let command = Command::new(&entry.name, &[], &entry.title, &entry.icon).action(action(entry, argv(entry, "")));
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
            let argv = argv(entry, &query.text);
            // With arguments typed, the line underneath is what will run, so the
            // numbers can be checked before Enter; otherwise the shortcut.
            let subtitle = if argv.len() > entry.command.len() {
                argv.join(" ")
            } else {
                entry.shortcut.clone().unwrap_or_default()
            };
            Item::new(format!("custom:{}", entry.name), &entry.title, subtitle, Icon::Theme(entry.icon.clone()))
                .action(action(entry, argv))
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
            arguments: false,
        };
        let commands = Custom(vec![entry(Some("Ctrl+Alt+Home")), entry(None)]).commands();
        assert_eq!(commands[0].detail(), "Recording: start  ·  Ctrl+Alt+Home");
        assert_eq!(commands[1].detail(), "Recording: start");
        assert!(commands.iter().all(|c| c.action.is_some()));
    }

    #[test]
    fn typed_words_are_passed_on_only_when_it_takes_arguments() {
        let entry = |arguments| CustomCommand {
            name: "clip".into(),
            title: "Recording: save replay".into(),
            icon: "document-save".into(),
            command: vec!["gsr-replay".into()],
            shortcut: None,
            arguments,
        };
        assert_eq!(argv(&entry(true), " 30 "), ["gsr-replay", "30"]);
        assert_eq!(argv(&entry(true), ""), ["gsr-replay"]);
        assert_eq!(argv(&entry(false), "30"), ["gsr-replay"]);
    }
}
