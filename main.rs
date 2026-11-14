//! Joltux: an unofficial Game Jolt launcher for Linux.
//!
//! Downloads games from Game Jolt, prefers native Linux builds, and runs
//! Windows builds through Proton (via umu-launcher) or Wine.

mod api;
mod config;
mod gui;
mod install;
mod launch;
mod library;
mod models;
mod paths;
mod runtime;
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
    Update {
        /// Rebuild even if the installed commit already matches GitHub
        #[arg(long)]
        force: bool,
    },
    /// Install Wine and umu-launcher, then configure Proton
    Setup,
    /// Open the desktop launcher
    Gui,
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
    /// Set a value: runner (umu|wine), umu_path, wine_path, proton_path, session_cookie
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
        Command::Update { force } => cmd_update(force),
        Command::Setup => cmd_setup(&paths),
        Command::Gui => gui::run(),
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
        println!("  no installable Linux or Windows builds found (web-only, macOS-only, paid/unowned, installers, or unsupported raw files)");
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
        bail!(
            "{title} has no installable Linux or Windows build (Joltux supports .zip and .tar.gz archives, plus standalone Windows .exe files; installers are not supported)"
        );
    };
    let version = choice.release.map(|r| r.version_number.clone()).unwrap_or_default();
    println!("Installing {title}: build {} ({})", choice.build.id, describe(choice.platform));

    let standalone_exe = choice.platform == Platform::Windows
        && choice
            .build
            .primary_file
            .as_ref()
            .and_then(|file| file.is_archive)
            == Some(false)
        && select::is_standalone_windows_executable(choice.build);
    let executable_path = if standalone_exe {
        choice
            .build
            .primary_file
            .as_ref()
            .and_then(|file| file.filename.clone())
            .context("standalone Windows build has no filename")?
    } else {
        select::launch_option(&payload.launch_options, choice.build.id, choice.platform)
            .map(|option| option.executable_path.clone())
            .filter(|path| !path.is_empty())
            .with_context(|| {
                format!(
                    "build {} has no launch path for this platform",
                    choice.build.id
                )
            })?
    };

    let url = api.download_url(choice.build.id)?;
    let archive = paths.downloads_dir.join(format!("{}.archive", choice.build.id));
    install::download(&url, &archive)?;

    let dir = paths.game_dir(id);
    if dir.exists() {
        fs::remove_dir_all(&dir).context("could not clear the previous install")?;
    }
    let executable = if standalone_exe {
        install::install_raw_executable(&archive, &dir, &executable_path)?
    } else {
        println!("Extracting...");
        let extracted = install::extract(&archive, &dir, &choice.build.archive_type);
        let _ = fs::remove_file(&archive); // the archive is no longer needed either way
        extracted?;
        install::find_executable(&dir, &executable_path)
            .with_context(|| {
                format!("installed, but could not find `{executable_path}` in the archive")
            })?
    };

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

const REPO_URL: &str = "https://github.com/KermitTheMagnificant/Joltux.git";
const LATEST_COMMIT_API: &str =
    "https://api.github.com/repos/KermitTheMagnificant/Joltux/commits/main";

fn cmd_update(force: bool) -> Result<()> {
    if !force {
        match (installed_commit(), latest_commit()) {
            (Some(installed), Ok(latest)) if installed == latest => {
                println!("Joltux is already up to date (commit {}).", short_sha(&latest));
                return Ok(());
            }
            (Some(installed), Ok(latest)) => {
                println!(
                    "Update available: {} -> {}.",
                    short_sha(&installed),
                    short_sha(&latest)
                );
            }
            (None, Ok(latest)) => {
                println!(
                    "Can't tell which commit is installed (it was probably installed from a local copy). \
                     Installing the latest commit, {}.",
                    short_sha(&latest)
                );
            }
            (_, Err(err)) => {
                println!("Couldn't check GitHub for the latest version ({err:#}). Updating anyway.");
            }
        }
    }

    println!("Updating Joltux from GitHub...");
    let status = ProcessCommand::new("cargo")
        .args(["install", "--git", REPO_URL, "--locked", "--force", "--bin", "joltux"])
        .status()
        .context("could not start Cargo; install Rust and Cargo to update Joltux")?;
    if !status.success() {
        bail!("Joltux update failed (Cargo exited with {status})");
    }
    println!("Joltux updated.");
    Ok(())
}

fn short_sha(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// The commit `cargo install --git` recorded for Joltux, if it was installed that way.
fn installed_commit() -> Option<String> {
    let cargo_root = std::env::var_os("CARGO_INSTALL_ROOT")
        .or_else(|| std::env::var_os("CARGO_HOME"))
        .map(std::path::PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|d| d.home_dir().join(".cargo")))?;
    let text = fs::read_to_string(cargo_root.join(".crates2.json")).ok()?;
    let json: serde_json::Value = serde_json::from_str(&text).ok()?;
    commit_from_crates2(&json)
}

/// Finds the Joltux entry, keyed like `joltux 0.1.0 (git+https://...Joltux.git#<sha>)`.
fn commit_from_crates2(json: &serde_json::Value) -> Option<String> {
    let repo = REPO_URL.trim_end_matches(".git").to_ascii_lowercase();
    json.get("installs")?.as_object()?.keys().find_map(|key| {
        let source = key.strip_prefix("joltux ")?.split_once(" (git+")?.1.strip_suffix(')')?;
        let (url, sha) = source.rsplit_once('#')?;
        let url = url.split('?').next()?.trim_end_matches(".git").to_ascii_lowercase();
        (url == repo).then(|| sha.to_owned())
    })
}

/// The latest commit on `main` according to the GitHub API.
fn latest_commit() -> Result<String> {
    let resp = reqwest::blocking::Client::builder()
        .user_agent(api::USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .context("failed to build HTTP client")?
        .get(LATEST_COMMIT_API)
        .header("Accept", "application/vnd.github.sha")
        .send()
        .context("could not reach GitHub")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("GitHub returned HTTP {status}");
    }
    let sha = resp.text().context("could not read GitHub's response")?.trim().to_owned();
    if sha.len() != 40 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("GitHub returned an unexpected response");
    }
    Ok(sha)
}

fn cmd_setup(paths: &Paths) -> Result<()> {
    let mut config = Config::load(paths)?;
    runtime::setup(paths, &mut config)?;
    println!("Runtime setup complete. Windows games will use umu with GE-Proton.");
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
    use super::{commit_from_crates2, parse_game_id};

    #[test]
    fn parses_ids_and_urls() {
        assert_eq!(parse_game_id("12345").unwrap(), 12345);
        assert_eq!(parse_game_id("https://gamejolt.com/games/cool-game/12345").unwrap(), 12345);
        assert_eq!(parse_game_id("https://gamejolt.com/games/cool-game/12345/").unwrap(), 12345);
        assert_eq!(parse_game_id("https://gamejolt.com/games/cool-game/12345?ref=x#top").unwrap(), 12345);
        assert!(parse_game_id("https://gamejolt.com/games/cool-game").is_err());
    }

    #[test]
    fn finds_installed_commit_in_crates2() {
        let json = serde_json::json!({"installs": {
            "rls 0.1.0 (git+https://github.com/KermitTheMagnificant/GRNU.git#aaaa)": {},
            "joltux 0.1.0 (git+https://github.com/KermitTheMagnificant/Joltux.git#bbbb)": {}
        }});
        assert_eq!(commit_from_crates2(&json).as_deref(), Some("bbbb"));

        let path_install = serde_json::json!({"installs": {
            "joltux 0.1.0 (path+file:///home/me/Joltux)": {}
        }});
        assert_eq!(commit_from_crates2(&path_install), None);
    }
}
