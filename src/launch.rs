//! Starting things. Each program gets its own app-<id>-<rand>.scope, the XDG
//! convention Plasma follows, or task managers file it under the launcher.

use std::path::Path;
use std::process::Command;
use std::process::Stdio;

pub fn spawn(argv: &[String], cwd: Option<&Path>, id: &str, token: Option<&str>) {
    let Some(program) = argv.first() else { return };
    let mut command = Command::new("systemd-run");
    command
        .args(["--user", "--scope", "--quiet", "--collect", "--slice=app.slice"])
        .arg(format!("--unit=app-{}-{:08x}", escape_unit(id), random()))
        .arg("--")
        .arg(program)
        .args(&argv[1..]);
    run(command, cwd, token);
}

pub fn open(target: &str, token: Option<&str>) {
    let mut command = Command::new("xdg-open");
    command.arg(target);
    run(command, None, token);
}

fn run(mut command: Command, cwd: Option<&Path>, token: Option<&str>) {
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    // The token lets the program take focus. LD_LIBRARY_PATH is ours, from the
    // Nix wrapper that finds Vulkan: it isn't for the programs we start.
    command.env_remove("LD_LIBRARY_PATH").env_remove("XDG_ACTIVATION_TOKEN").env_remove("DESKTOP_STARTUP_ID");
    if let Some(token) = token {
        command.env("XDG_ACTIVATION_TOKEN", token).env("DESKTOP_STARTUP_ID", token);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    match command.spawn() {
        // The scope execs into the program, so this waits for as long as it runs.
        Ok(mut child) => {
            std::thread::spawn(move || child.wait());
        }
        Err(e) => log::warn!("couldn't start {:?}: {e}", command.get_program()),
    }
}

/// What systemd accepts in a unit name, the rest as \xNN like systemd-escape.
fn escape_unit(id: &str) -> String {
    let mut out = String::new();
    for byte in id.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b':' | b'_' | b'.' => out.push(byte as char),
            b'/' | b'-' => out.push('_'),
            _ => out.push_str(&format!("_{byte:02x}")),
        }
    }
    if out.is_empty() { "app".into() } else { out }
}

fn random() -> u32 {
    use std::hash::BuildHasher;
    std::collections::hash_map::RandomState::new().hash_one(std::process::id()) as u32
}

/// The first of these programs on PATH.
pub fn which(names: &[&str]) -> Option<String> {
    names.iter().find(|name| on_path(name)).map(|name| name.to_string())
}

pub fn on_path(name: &str) -> bool {
    if name.contains('/') {
        return Path::new(name).exists();
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).exists()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names_stay_valid() {
        assert_eq!(escape_unit("org.kde.dolphin"), "org.kde.dolphin");
        assert_eq!(escape_unit("com/foo bar"), "com_foo_20bar");
        assert_eq!(escape_unit(""), "app");
    }
}
