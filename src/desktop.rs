//! Desktop integration for the GUI: games running in the background, opening
//! folders and links, and notifications.

use std::fs::{self, File};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Instant;

use anyhow::{Context, Result};

use crate::launch;
use crate::paths::Paths;

/// A game started from the desktop app. It runs through `joltux run` (which
/// records playtime) with its output going to the game's log file.
pub struct RunningGame {
    child: Child,
    pub started: Instant,
    /// Set once Stop was pressed; pressing it again force-stops.
    pub stopping: bool,
}

impl RunningGame {
    pub fn start(paths: &Paths, game_id: u64) -> Result<Self> {
        fs::create_dir_all(&paths.logs_dir).context("could not create the logs folder")?;
        let log = File::create(paths.log_file(game_id)).context("could not create the game's log file")?;
        let exe = std::env::current_exe().context("could not find the Joltux executable")?;
        let child = Command::new(exe)
            .args(["run", &game_id.to_string()])
            .env(launch::OWN_PROCESS_GROUP_ENV, "1")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()
            .context("could not start Joltux to run the game")?;
        Ok(Self { child, started: Instant::now(), stopping: false })
    }

    /// The exit status once `joltux run` has finished.
    pub fn finished(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Asks the game to quit (SIGTERM); the second call kills it (SIGKILL).
    /// `joltux run` started the game in its own process group, so the signal
    /// also reaches whatever the game, Wine or umu started.
    pub fn stop(&mut self) {
        let signal = if self.stopping { "-KILL" } else { "-TERM" };
        self.stopping = true;
        let children = children_of(self.child.id());
        if children.is_empty() {
            // The game hasn't started yet (or is already gone): stop Joltux itself.
            let _ = self.child.kill();
            return;
        }
        for pid in children {
            let group = format!("-{pid}");
            let _ = Command::new("kill")
                .args([signal, "--", &group])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// Process IDs whose parent is `parent`, read from /proc.
fn children_of(parent: u32) -> Vec<u32> {
    let Ok(entries) = fs::read_dir("/proc") else { return Vec::new() };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let stat = fs::read_to_string(entry.path().join("stat")).ok()?;
            (parent_pid(&stat)? == parent).then_some(pid)
        })
        .collect()
}

/// The parent PID from a /proc/<pid>/stat line. The command name in
/// parentheses may contain spaces or parentheses, so parse after the last `)`.
fn parent_pid(stat: &str) -> Option<u32> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// The last few non-empty lines of a log file, for error messages.
pub fn log_tail(path: &Path, lines: usize) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut tail: Vec<&str> = text.lines().rev().filter(|l| !l.trim().is_empty()).take(lines).collect();
    tail.reverse();
    tail.join("\n")
}

/// Opens a folder, file or URL with the desktop's default application.
pub fn open(target: impl AsRef<std::ffi::OsStr>) -> Result<()> {
    spawn_detached(Command::new("xdg-open").arg(target)).context("could not run xdg-open")
}

/// Shows a desktop notification. Missing `notify-send` is silently ignored.
pub fn notify(summary: &str, body: &str) {
    let _ = spawn_detached(
        Command::new("notify-send")
            .args(["--app-name=Joltux", "--icon=joltux", summary, body]),
    );
}

/// Starts a short-lived helper and reaps it on a thread, so it doesn't
/// linger as a zombie.
fn spawn_detached(cmd: &mut Command) -> std::io::Result<()> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_parent_pid_with_odd_command_names() {
        assert_eq!(parent_pid("1234 (bash) S 1000 1234 1234 0 -1"), Some(1000));
        assert_eq!(parent_pid("42 (Game (x) .exe) R 7 42 42"), Some(7));
        assert_eq!(parent_pid("garbage"), None);
    }

    #[test]
    fn finds_own_child_process() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        assert!(children_of(std::process::id()).contains(&child.id()));
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn tails_logs() {
        let path = std::env::temp_dir().join(format!("joltux-test-log-{}", std::process::id()));
        fs::write(&path, "a\n\nb\nc\n\n").unwrap();
        assert_eq!(log_tail(&path, 2), "b\nc");
        assert_eq!(log_tail(Path::new("/nonexistent/log"), 3), "");
        fs::remove_file(path).unwrap();
    }
}
