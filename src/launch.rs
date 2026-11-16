//! Starting an installed game, natively or through Proton/Wine.

use std::ffi::OsString;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::checks;
use crate::config::{Config, Runner};
use crate::models::InstalledGame;
use crate::paths::Paths;
use crate::select::Platform;

/// When set, the game is started in its own process group (used by the
/// desktop app; a terminal launch keeps Ctrl+C reaching the game).
pub const OWN_PROCESS_GROUP_ENV: &str = "JOLTUX_GAME_PROCESS_GROUP";

/// Runs the game and waits for it to exit.
pub fn run(game: &InstalledGame, config: &Config, paths: &Paths) -> Result<()> {
    if !game.executable.is_file() {
        bail!(
            "{} is missing. Reinstall with: joltux install {}",
            game.executable.display(),
            game.game_id
        );
    }
    let prefix = paths.prefix_dir(game.game_id);
    match game.platform {
        Platform::Linux => ensure_executable(&game.executable)?,
        Platform::Windows => fs::create_dir_all(&prefix)?,
    }
    let runner = runner_for(game, config);
    let mut cmd = command(game, config, &prefix);
    #[cfg(unix)]
    if std::env::var_os(OWN_PROCESS_GROUP_ENV).is_some() {
        // The desktop app stops a game by signalling this group, which also
        // reaches whatever the game, Wine or umu start, but not this process,
        // so playtime is still recorded.
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    eprintln!("Launching {} ...", game.title);
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => bail!(
            "could not start the launcher program ({e}). Is `{}` installed and on your PATH?",
            match (game.launch.wrapper.first(), game.platform, runner) {
                (Some(wrapper), ..) => wrapper.as_str(),
                (None, Platform::Linux, _) => "the game's executable",
                (None, Platform::Windows, Runner::Umu) => "umu-run",
                (None, Platform::Windows, Runner::Wine) => "wine",
            }
        ),
        Err(e) => return Err(e).context("failed to start game"),
    };
    eprintln!("{} exited ({status})", game.title);
    if !status.success() {
        explain_failure(game.platform, runner);
    }
    Ok(())
}

/// The game's own runner if it has one, otherwise the global setting.
pub fn runner_for(game: &InstalledGame, config: &Config) -> Runner {
    game.launch.runner.unwrap_or(config.runner)
}

/// Prints likely causes after a game exits with an error.
fn explain_failure(platform: Platform, runner: Runner) {
    let problems = checks::find_problems(platform == Platform::Windows);
    if problems.is_empty() {
        if (platform, runner) == (Platform::Windows, Runner::Umu) {
            eprintln!(
                "No known system problem was found. For a detailed log, launch from a terminal \
                 with PROTON_LOG=1 set; Proton writes it to ~/steam-0.log."
            );
        }
        return;
    }
    eprintln!("Possible causes:");
    for problem in problems {
        eprintln!("  - {problem}");
    }
}

fn ensure_executable(exe: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Zip files from Windows machines often lose the executable bit.
        let mut perms = fs::metadata(exe)?.permissions();
        if perms.mode() & 0o111 == 0 {
            perms.set_mode(perms.mode() | 0o755);
            fs::set_permissions(exe, perms).context("could not make the game executable")?;
        }
    }
    Ok(())
}

/// The full launch command: `[wrapper...] [runner] game [args...]`, started
/// in the game's folder, with the game's own environment variables applied last.
fn command(game: &InstalledGame, config: &Config, prefix: &Path) -> Command {
    let exe = OsString::from(&game.executable);
    let (program, mut argv) = match (game.platform, runner_for(game, config)) {
        (Platform::Linux, _) => (exe, Vec::new()),
        (Platform::Windows, Runner::Umu) => (OsString::from(&config.umu_path), vec![exe]),
        (Platform::Windows, Runner::Wine) => (OsString::from(&config.wine_path), vec![exe]),
    };
    argv.extend(game.launch.args.iter().map(OsString::from));

    let mut cmd = match game.launch.wrapper.split_first() {
        Some((wrapper, wrapper_args)) => {
            let mut c = Command::new(wrapper);
            c.args(wrapper_args).arg(program);
            c
        }
        None => Command::new(program),
    };
    cmd.args(argv);

    if game.platform == Platform::Windows {
        cmd.env("WINEPREFIX", prefix);
        if runner_for(game, config) == Runner::Umu {
            // GAMEID "umu-0" means "no game-specific fixes". PROTONPATH picks
            // the Proton build; umu fetches GE-Proton if given that name.
            cmd.env("GAMEID", "umu-0").env("PROTONPATH", &config.proton_path);
        }
    }
    cmd.envs(&game.launch.env);
    // Games usually load assets relative to their own folder, so start there.
    cmd.current_dir(game.executable.parent().unwrap_or(Path::new(".")));
    cmd
}

/// Parses `KEY=VALUE` (the value may be empty or contain `=`).
pub fn parse_env(text: &str) -> Result<(String, String)> {
    let Some((key, value)) = text.split_once('=') else {
        bail!("`{text}` is not KEY=VALUE");
    };
    let key = key.trim();
    let valid = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        bail!("`{key}` is not a valid environment variable name");
    }
    Ok((key.to_owned(), value.to_owned()))
}

/// Splits a command line into words the way a shell would for simple cases:
/// whitespace separates words, quotes group them and a backslash escapes the
/// next character (except inside single quotes).
pub fn split_words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => match chars.next() {
                Some(next) => {
                    word.push(next);
                    in_word = true;
                }
                None => bail!("`{text}` ends with a lone backslash"),
            },
            (Some(_), c) => word.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                in_word = true;
            }
            (None, c) if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            (None, c) => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if quote.is_some() {
        bail!("`{text}` has an unclosed quote");
    }
    if in_word {
        words.push(word);
    }
    Ok(words)
}

/// The reverse of `split_words`: quotes words only where needed.
pub fn join_words(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && word.chars().all(|c| !c.is_whitespace() && !"'\"\\$`".contains(c));
            if plain {
                word.clone()
            } else {
                format!("'{}'", word.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::LaunchSettings;
    use std::path::PathBuf;

    fn game(platform: Platform, launch: LaunchSettings) -> InstalledGame {
        InstalledGame {
            game_id: 1,
            title: "Test".into(),
            build_id: 2,
            platform,
            version: String::new(),
            install_dir: PathBuf::from("/games/1"),
            executable: PathBuf::from("/games/1/Game.exe"),
            package_id: None,
            launch,
            slug: None,
            thumbnail_url: None,
            last_played: None,
            playtime_secs: 0,
        }
    }

    fn argv(cmd: &Command) -> Vec<String> {
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    fn env(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(k, _)| *k == key)
            .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()))
    }

    #[test]
    fn default_windows_launch_uses_global_runner() {
        let cmd = command(&game(Platform::Windows, LaunchSettings::default()), &Config::default(), Path::new("/p"));
        assert_eq!(argv(&cmd), ["umu-run", "/games/1/Game.exe"]);
        assert_eq!(env(&cmd, "WINEPREFIX").as_deref(), Some("/p"));
        assert_eq!(env(&cmd, "GAMEID").as_deref(), Some("umu-0"));
        assert_eq!(cmd.get_current_dir(), Some(Path::new("/games/1")));
    }

    #[test]
    fn per_game_settings_wrap_override_and_extend() {
        let launch = LaunchSettings {
            env: [("PROTONPATH".to_string(), "/my/proton".to_string()), ("DXVK_HUD".into(), "1".into())]
                .into_iter()
                .collect(),
            args: vec!["-windowed".into()],
            wrapper: vec!["gamescope".into(), "-f".into(), "--".into()],
            runner: None,
        };
        let cmd = command(&game(Platform::Windows, launch), &Config::default(), Path::new("/p"));
        assert_eq!(argv(&cmd), ["gamescope", "-f", "--", "umu-run", "/games/1/Game.exe", "-windowed"]);
        // The game's own variables win over Joltux's.
        assert_eq!(env(&cmd, "PROTONPATH").as_deref(), Some("/my/proton"));
        assert_eq!(env(&cmd, "DXVK_HUD").as_deref(), Some("1"));
    }

    #[test]
    fn runner_override_switches_to_wine() {
        let launch = LaunchSettings { runner: Some(Runner::Wine), ..Default::default() };
        let cmd = command(&game(Platform::Windows, launch), &Config::default(), Path::new("/p"));
        assert_eq!(argv(&cmd), ["wine", "/games/1/Game.exe"]);
        assert_eq!(env(&cmd, "GAMEID"), None);
    }

    #[test]
    fn native_launch_has_no_wine_environment() {
        let launch = LaunchSettings { wrapper: vec!["gamemoderun".into()], ..Default::default() };
        let cmd = command(&game(Platform::Linux, launch), &Config::default(), Path::new("/p"));
        assert_eq!(argv(&cmd), ["gamemoderun", "/games/1/Game.exe"]);
        assert_eq!(env(&cmd, "WINEPREFIX"), None);
    }

    #[test]
    fn parses_env_assignments() {
        assert_eq!(parse_env("DXVK_HUD=fps,devinfo").unwrap(), ("DXVK_HUD".into(), "fps,devinfo".into()));
        assert_eq!(parse_env("A=b=c").unwrap(), ("A".into(), "b=c".into()));
        assert_eq!(parse_env("EMPTY=").unwrap(), ("EMPTY".into(), String::new()));
        assert!(parse_env("NOEQUALS").is_err());
        assert!(parse_env("1BAD=x").is_err());
        assert!(parse_env("=x").is_err());
    }

    #[test]
    fn splits_and_joins_words() {
        assert_eq!(split_words("  -a  'b c' \"d \\\"e\\\"\" f\\ g ''").unwrap(), ["-a", "b c", "d \"e\"", "f g", ""]);
        assert!(split_words("'open").is_err());
        assert!(split_words("").unwrap().is_empty());
        let words: Vec<String> = ["-a", "b c", "it's", ""].iter().map(|s| s.to_string()).collect();
        assert_eq!(join_words(&words), r"-a 'b c' 'it'\''s' ''");
        assert_eq!(split_words(&join_words(&words)).unwrap(), words);
    }
}
