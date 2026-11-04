//! `/kill name`: your running programs, most memory first, each name once with
//! all its processes. Tab completes the name. Enter ends them, asking first,
//! and Alt's list has forcing them.

use crate::launch;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Command;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

const ICON: &str = "process-stop";
/// More is scrolling past what nobody looks for: typing narrows it.
const SHOWN: usize = 40;

pub struct Processes;

#[derive(Debug, PartialEq)]
struct Program {
    name: String,
    pids: Vec<u32>,
    /// Resident memory of them all, in kB.
    memory: u64,
}

/// What it's known as, from its command line: `/usr/lib/firefox/firefox` is
/// firefox, Nix's `.firefox-wrapped` too, and a login shell's `-zsh` is zsh.
/// Chromium rewrites its command line as one string with spaces in it.
/// `None` for kernel threads, which have none.
fn name(cmdline: &[u8]) -> Option<String> {
    let first = cmdline.split(|&b| b == 0).next().filter(|first| !first.is_empty())?;
    let mut program = String::from_utf8_lossy(first).into_owned();
    if program.contains(' ') && !Path::new(&program).exists() {
        program.truncate(program.find(' ').unwrap_or(program.len()));
    }
    let base = program.rsplit('/').next().unwrap_or(&program);
    let base = base.trim_start_matches('-');
    let base = base.strip_prefix('.').and_then(|b| b.strip_suffix("-wrapped")).unwrap_or(base);
    (!base.is_empty()).then(|| base.to_string())
}

/// From `/proc/<pid>/status`.
fn resident_kb(status: &str) -> u64 {
    status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|kb| kb.parse().ok())
        .unwrap_or(0)
}

fn name_of(pid: u32) -> Option<String> {
    name(&fs::read(format!("/proc/{pid}/cmdline")).ok()?)
}

/// The user's own, but not Génie. Grouped by name, most memory first.
fn programs() -> Vec<Program> {
    let Ok(me) = fs::metadata("/proc/self") else { return Vec::new() };
    let own = std::process::id();
    let mut by_name: HashMap<String, Program> = HashMap::new();
    for entry in fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if pid == own || entry.metadata().map_or(true, |m| m.uid() != me.uid()) {
            continue;
        }
        let Some(name) = name_of(pid) else { continue };
        let memory = fs::read_to_string(entry.path().join("status")).map_or(0, |status| resident_kb(&status));
        let program = by_name.entry(name.clone()).or_insert_with(|| Program { name, pids: Vec::new(), memory: 0 });
        program.pids.push(pid);
        program.memory += memory;
    }
    let mut programs: Vec<Program> = by_name.into_values().collect();
    programs.sort_by(|a, b| b.memory.cmp(&a.memory).then_with(|| a.name.cmp(&b.name)));
    programs
}

fn memory(kb: u64) -> String {
    match kb {
        0..1024 => format!("{kb} kB"),
        1024..1_048_576 => format!("{} MB", kb / 1024),
        _ => format!("{:.1} GB", kb as f64 / 1_048_576.0),
    }
}

fn describe(program: &Program) -> String {
    match program.pids.as_slice() {
        [pid] => format!("PID {pid}  ·  {}", memory(program.memory)),
        pids => format!("{} processes  ·  {}", pids.len(), memory(program.memory)),
    }
}

/// Those still running under that name: between listing and Enter, a pid can
/// have gone, and been taken by something else.
fn signal(name: &str, pids: &[u32], signal: &str) {
    let alive: Vec<String> = pids.iter().filter(|&&pid| name_of(pid).as_deref() == Some(name)).map(u32::to_string).collect();
    if alive.is_empty() {
        return;
    }
    let ok = std::process::Command::new("kill")
        .arg(format!("-{signal}"))
        .args(&alive)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !ok {
        launch::notify(&format!("Couldn't end every {name} process"));
    }
}

fn item(program: &Program, score: f32) -> Item {
    let (name, count) = (program.name.clone(), program.pids.len());
    let what = if count == 1 { name.clone() } else { format!("{name} ({count} processes)") };
    let effect = |sig: &'static str| {
        let (name, pids) = (name.clone(), program.pids.clone());
        Effect::Call(Arc::new(move || signal(&name, &pids, sig)))
    };
    Item::new(format!("kill.{name}"), &name, describe(program), Icon::Theme(name.clone()))
        .action(Action::new("end", "End it", effect("TERM")).confirm(format!("End {what}?")))
        .action(Action::new("force", "Force it to quit", effect("KILL")).confirm(format!("Force {what} to quit? Unsaved work is lost")))
        .completion(format!("/kill {name}"))
        .score(score)
}

/// Most memory first when nothing's typed, or else the best match. A pid
/// finds its program too.
fn matching(programs: &[Program], text: &str) -> Vec<Item> {
    let mut items: Vec<Item> = programs
        .iter()
        .enumerate()
        .filter_map(|(rank, program)| {
            // Ties go to the biggest, without ever beating a better match.
            let tiebreak = 0.001 * (1.0 - rank as f32 / programs.len() as f32);
            if text.is_empty() {
                return Some(item(program, 1.0 - rank as f32 * 0.001));
            }
            if program.pids.iter().any(|pid| pid.to_string() == text) {
                return Some(item(program, 1.0));
            }
            matcher::score(text, &[(&program.name, 1.0)]).map(|score| item(program, score + tiebreak))
        })
        .collect();
    items.sort_by(|a, b| b.score.total_cmp(&a.score));
    items.truncate(SHOWN);
    items
}

impl Plugin for Processes {
    fn id(&self) -> &'static str {
        "processes"
    }

    fn group(&self) -> &'static str {
        "System"
    }

    fn commands(&self) -> Vec<Command> {
        vec![Command::new("kill", &[], "End a running program, asking first", ICON)]
    }

    fn query(&self, query: &Query) {
        query.send(matching(&programs(), query.text.trim()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_command_line() {
        assert_eq!(name(b"/usr/lib/firefox/firefox\0-contentproc\0").as_deref(), Some("firefox"));
        assert_eq!(name(b"/nix/store/abc-firefox/bin/.firefox-wrapped\0").as_deref(), Some("firefox"));
        assert_eq!(name(b"-zsh\0").as_deref(), Some("zsh"));
        assert_eq!(name(b"/opt/google/chrome/chrome --type=renderer --lang=fr").as_deref(), Some("chrome"));
        assert_eq!(name(b"python3\0script.py\0").as_deref(), Some("python3"));
        assert_eq!(name(b""), None);
        assert_eq!(name(b"\0"), None);
    }

    #[test]
    fn resident_memory_is_read_from_status() {
        assert_eq!(resident_kb("Name:\tzsh\nVmRSS:\t    5120 kB\nThreads:\t1\n"), 5120);
        assert_eq!(resident_kb("Name:\tkthreadd\n"), 0);
        assert_eq!(memory(512), "512 kB");
        assert_eq!(memory(5120), "5 MB");
        assert_eq!(memory(3 * 1_048_576 / 2), "1.5 GB");
    }

    fn program(name: &str, pids: &[u32], memory: u64) -> Program {
        Program { name: name.into(), pids: pids.to_vec(), memory }
    }

    #[test]
    fn listed_by_memory_found_by_name_or_pid() {
        let programs = [program("firefox", &[10, 11], 900_000), program("zed", &[20], 400_000), program("firewalld-ui", &[30], 1000)];
        let titles = |text: &str| matching(&programs, text).into_iter().map(|i| i.title).collect::<Vec<_>>();
        assert_eq!(titles(""), ["firefox", "zed", "firewalld-ui"]);
        assert_eq!(titles("fire"), ["firefox", "firewalld-ui"]);
        assert_eq!(titles("firewalld"), ["firewalld-ui"]);
        assert_eq!(titles("20"), ["zed"]);
        assert!(titles("chrome").is_empty());
    }

    #[test]
    fn ending_asks_first_and_tab_completes() {
        let item = item(&program("firefox", &[10, 11], 900_000), 1.0);
        assert_eq!(item.subtitle, "2 processes  ·  878 MB");
        assert_eq!(item.completion.as_deref(), Some("/kill firefox"));
        assert_eq!(item.actions[0].confirm.as_deref(), Some("End firefox (2 processes)?"));
        assert!(item.actions.iter().all(|a| a.confirm.is_some()));
    }
}
