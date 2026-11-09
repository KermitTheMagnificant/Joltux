//! Starting an installed game, natively or through Proton/Wine.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

use crate::config::{Config, Runner};
use crate::models::InstalledGame;
use crate::paths::Paths;
use crate::select::Platform;

/// Runs the game and waits for it to exit.
pub fn run(game: &InstalledGame, config: &Config, paths: &Paths) -> Result<()> {
    if !game.executable.is_file() {
        bail!(
            "{} is missing. Reinstall with: joltux install {}",
            game.executable.display(),
            game.game_id
        );
    }
    // Games usually load assets relative to their own folder, so start there.
    let workdir = game.executable.parent().unwrap_or(Path::new("."));

    let mut cmd = match game.platform {
        Platform::Linux => native_command(&game.executable)?,
        Platform::Windows => windows_command(game, config, paths)?,
    };
    cmd.current_dir(workdir);

    eprintln!("Launching {} ...", game.title);
    let status = match cmd.status() {
        Ok(s) => s,
        Err(e) if e.kind() == ErrorKind::NotFound => bail!(
            "could not start the launcher program ({e}). Is `{}` installed and on your PATH?",
            match (game.platform, config.runner) {
                (Platform::Linux, _) => "the game's executable",
                (Platform::Windows, Runner::Umu) => "umu-run",
                (Platform::Windows, Runner::Wine) => "wine",
            }
        ),
        Err(e) => return Err(e).context("failed to start game"),
    };
    eprintln!("{} exited ({status})", game.title);
    Ok(())
}

fn native_command(exe: &Path) -> Result<Command> {
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
    Ok(Command::new(exe))
}

fn windows_command(game: &InstalledGame, config: &Config, paths: &Paths) -> Result<Command> {
    let prefix = paths.prefix_dir(game.game_id);
    fs::create_dir_all(&prefix)?;

    let mut cmd = match config.runner {
        Runner::Umu => {
            let mut c = Command::new("umu-run");
            // GAMEID "umu-0" means "no game-specific fixes". PROTONPATH picks
            // the Proton build; umu fetches GE-Proton if given that name.
            c.env("GAMEID", "umu-0").env("PROTONPATH", &config.proton_path);
            c
        }
        Runner::Wine => Command::new("wine"),
    };
    cmd.env("WINEPREFIX", &prefix).arg(&game.executable);
    Ok(cmd)
}
