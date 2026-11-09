//! Joltux: an unofficial Game Jolt launcher for Linux.
//!
//! Downloads games from Game Jolt, prefers native Linux builds, and runs
//! Windows builds through Proton (via umu-launcher) or Wine.

mod api;
mod config;
mod install;
mod launch;
mod library;
mod models;
mod paths;
mod select;

use std::fs;
use std::io::{self, Write};
use std::process::Command as ProcessCommand;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use crate::api::{ApiClient, LoginOutcome};
use crate::config::Config;
use crate::library::Library;
use crate::models::InstalledGame;
use crate::paths::Paths;
use crate::select::Platform;

#[derive(Parser)]
#[command(name = "joltux", version, about = "Unofficial Game Jolt launcher for Linux")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show a game's title and the builds that are available
    Info {
        /// Game ID or a gamejolt.com game URL
        game: String,
    },
    /// Download and install a game
    Install {
        /// Game ID or a gamejolt.com game URL
        game: String,
        /// Ignore native Linux builds and install the Windows build
        #[arg(long)]
        windows: bool,
    },
    /// Run an installed game
    Run {
        /// Game ID (see `joltux list`)
        game_id: u64,
    },
    /// List installed games
    List,
    /// Log in to Game Jolt
    Login,
    /// Log out of Game Jolt
    Logout,
    /// Update Joltux from its GitHub repository
    Update,
    /// Remove an installed game (and, with --prefix, its Wine prefix)
    Uninstall {
        game_id: u64,
        /// Also delete the game's Wine/Proton prefix (saves may live there)
        #[arg(long)]
        prefix: bool,
    },
    /// View or change settings
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print current settings
    Show,
    /// Set a value: runner (umu|wine), proton_path, session_cookie
    Set { key: String, value: String },
}

fn main() {
    if let Err(err) = run() {
        // `{:#}` prints the whole chain: "outer context: inner cause".
        eprintln!("error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let paths = Paths::discover()?;

    match cli.command {
        Command::Info { game } => cmd_info(&paths, &game),
        Command::Install { game, windows } => cmd_install(&paths, &game, windows),
        Command::Run { game_id } => cmd_run(&paths, game_id),
        Command::List => cmd_list(&paths),
        Command::Login => cmd_login(&paths),
        Command::Logout => cmd_logout(&paths),
        Command::Update => cmd_update(),
        Command::Uninstall { game_id, prefix } => cmd_uninstall(&paths, game_id, prefix),
        Command::Config { action } => cmd_config(&paths, action),
    }
}

/// Accepts `12345`, `https://gamejolt.com/games/some-slug/12345`, with or
/// without a trailing slash, query string or fragment.
fn parse_game_id(input: &str) -> Result<u64> {
    let trimmed = input.trim();
    let path = trimmed.split(['?', '#']).next().unwrap_or(trimmed);
    path.rsplit('/')
        .find(|seg| !seg.is_empty())
        .and_then(|seg| seg.parse::<u64>().ok())
        .with_context(|| format!("could not find a game ID in `{input}`"))
}

fn describe(platform: Platform) -> &'static str {
    match platform {
        Platform::Linux => "native Linux",
        Platform::Windows => "Windows (via Proton/Wine)",
    }
}

fn cmd_info(paths: &Paths, game: &str) -> Result<()> {
    let id = parse_game_id(game)?;
    let config = Config::load(paths)?;
    let api = ApiClient::new(config.session_cookie.as_deref())?;

    let title = api.game_title(id)?.unwrap_or_else(|| format!("game {id}"));
    let payload = api.packages(id)?;
    println!("{title} (id {id})");

    let candidates = select::candidates(&payload, false);
    if candidates.is_empty() {
        println!("  no installable Linux or Windows archive builds found (web-only, macOS-only, paid/unowned, or raw files)");
        return Ok(());
    }
    for (i, c) in candidates.iter().enumerate() {
        let version = c.release.map(|r| r.version_number.as_str()).unwrap_or("?");
        let size = c
            .build
            .primary_file
            .as_ref()
            .and_then(|f| f.filesize)
            .map(|b| format!("{:.1} MiB", b as f64 / (1024.0 * 1024.0)))
            .unwrap_or_else(|| "unknown size".into());
        let marker = if i == 0 { "  <- would install" } else { "" };
        println!("  build {} v{} {} [{}]{}", c.build.id, version, describe(c.platform), size, marker);
    }
    Ok(())
}

fn cmd_install(paths: &Paths, game: &str, windows_only: bool) -> Result<()> {
    let id = parse_game_id(game)?;
    let config = Config::load(paths)?;
    let api = ApiClient::new(config.session_cookie.as_deref())?;

    let title = api.game_title(id)?.unwrap_or_else(|| format!("game {id}"));
    let payload = api.packages(id)?;
    let Some(choice) = select::choose(&payload, windows_only) else {
        bail!("{title} has no installable Linux or Windows archive build (Joltux supports .zip and .tar.gz archives, not raw executables or installers)");
    };
    let version = choice.release.map(|r| r.version_number.clone()).unwrap_or_default();
    println!("Installing {title}: build {} ({})", choice.build.id, describe(choice.platform));

    // Work out what to launch before downloading, so we fail early if the
    // game has no launch information.
    let Some(option) = select::launch_option(&payload.launch_options, choice.build.id, choice.platform) else {
        bail!("build {} has no launch option for this platform", choice.build.id);
    };

    let url = api.download_url(choice.build.id)?;
    let archive = paths.downloads_dir.join(format!("{}.archive", choice.build.id));
    install::download(&url, &archive)?;

    let dir = paths.game_dir(id);
    if dir.exists() {
        fs::remove_dir_all(&dir).context("could not clear the previous install")?;
    }
    println!("Extracting...");
    let extracted = install::extract(&archive, &dir, &choice.build.archive_type);
    let _ = fs::remove_file(&archive); // the archive is no longer needed either way
    extracted?;

    let executable = install::find_executable(&dir, &option.executable_path)
        .with_context(|| format!("installed, but could not find `{}` in the archive", option.executable_path))?;

    let mut library = Library::load(paths)?;
    library.upsert(InstalledGame {
        game_id: id,
        title: title.clone(),
        build_id: choice.build.id,
        platform: choice.platform,
        version,
        install_dir: dir,
        executable,
    });
    library.save(paths)?;
    println!("Done. Play with: joltux run {id}");
    Ok(())
}

fn cmd_run(paths: &Paths, game_id: u64) -> Result<()> {
    let library = Library::load(paths)?;
    let game = library
        .find(game_id)
        .with_context(|| format!("game {game_id} is not installed (see `joltux list`)"))?;
    let config = Config::load(paths)?;
    launch::run(game, &config, paths)
}

fn cmd_list(paths: &Paths) -> Result<()> {
    let library = Library::load(paths)?;
    if library.games.is_empty() {
        println!("No games installed. Try: joltux install <game id or URL>");
    }
    for g in &library.games {
        println!("{:>8}  {}  [{}]", g.game_id, g.title, describe(g.platform));
    }
    Ok(())
}

fn cmd_login(paths: &Paths) -> Result<()> {
    let mut username = String::new();
    print!("Game Jolt username: ");
    io::stdout().flush()?;
    io::stdin().read_line(&mut username)?;
    let username = username.trim();
    if username.is_empty() {
        bail!("username cannot be empty");
    }
    let password = rpassword::prompt_password("Password: ").context("could not read password")?;
    let api = ApiClient::new(None)?;

    let cookie = match api.login(username, &password)? {
        LoginOutcome::Authenticated(cookie) => cookie,
        LoginOutcome::CaptchaRequired => {
            println!("Game Jolt requires a CAPTCHA. Sign in through your browser, then copy its frontend session cookie here.");
            let cookie = rpassword::prompt_password("frontend cookie: ")
                .context("could not read frontend cookie")?;
            if cookie.is_empty() {
                bail!("frontend cookie cannot be empty");
            }
            cookie
        }
        LoginOutcome::ApprovalRequired(token) => {
            println!("Approve this login using the link Game Jolt emailed you. Waiting up to 10 minutes...");
            api.poll_login_approval(&token)?
        }
    };

    let mut config = Config::load(paths)?;
    config.session_cookie = Some(cookie);
    config.save(paths)?;
    println!("Logged in to Game Jolt.");
    Ok(())
}

fn cmd_logout(paths: &Paths) -> Result<()> {
    let mut config = Config::load(paths)?;
    config.session_cookie = None;
    config.save(paths)?;
    println!("Logged out of Game Jolt.");
    Ok(())
}

fn cmd_update() -> Result<()> {
    println!("Updating Joltux from GitHub...");
    let status = ProcessCommand::new("cargo")
        .args([
            "install",
            "--git",
            "https://github.com/KermitTheMagnificant/Joltux.git",
            "--locked",
            "--force",
            "--bin",
            "joltux",
        ])
        .status()
        .context("could not start Cargo; install Rust and Cargo to update Joltux")?;
    if !status.success() {
        bail!("Joltux update failed (Cargo exited with {status})");
    }
    println!("Joltux is up to date.");
    Ok(())
}

fn cmd_uninstall(paths: &Paths, game_id: u64, prefix: bool) -> Result<()> {
    let mut library = Library::load(paths)?;
    let game = library
        .remove(game_id)
        .with_context(|| format!("game {game_id} is not installed"))?;
    if game.install_dir.exists() {
        fs::remove_dir_all(&game.install_dir)?;
    }
    if prefix {
        let p = paths.prefix_dir(game_id);
        if p.exists() {
            fs::remove_dir_all(p)?;
        }
    }
    library.save(paths)?;
    println!("Removed {}", game.title);
    Ok(())
}

fn cmd_config(paths: &Paths, action: ConfigAction) -> Result<()> {
    let mut config = Config::load(paths)?;
    match action {
        ConfigAction::Show => println!("{}", config.redacted()),
        ConfigAction::Set { key, value } => {
            config.set(&key, &value)?;
            config.save(paths)?;
            println!("Saved.");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::parse_game_id;

    #[test]
    fn parses_ids_and_urls() {
        assert_eq!(parse_game_id("12345").unwrap(), 12345);
        assert_eq!(parse_game_id("https://gamejolt.com/games/cool-game/12345").unwrap(), 12345);
        assert_eq!(parse_game_id("https://gamejolt.com/games/cool-game/12345/").unwrap(), 12345);
        assert_eq!(parse_game_id("https://gamejolt.com/games/cool-game/12345?ref=x#top").unwrap(), 12345);
        assert!(parse_game_id("https://gamejolt.com/games/cool-game").is_err());
    }
}
