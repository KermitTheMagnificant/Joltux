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
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use crate::api::ApiClient;
use crate::config::Config;
use crate::library::Library;
use crate::models::InstalledGame;
use crate::paths::Paths;
use crate::select::{Filter, Platform};

#[derive(Parser)]
#[command(
    name = "joltux",
    version,
    about = "Unofficial Game Jolt launcher for Linux"
)]
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
        /// Install from this package (ID or name, e.g. "demo"); see `joltux info`
        #[arg(long)]
        package: Option<String>,
        /// Install this exact build ID; see `joltux info`
        #[arg(long)]
        build: Option<u64>,
    },
    /// Run an installed game
    Run {
        /// Game ID (see `joltux list`)
        game_id: u64,
    },
    /// List installed games
    List,
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
    /// Set a value: runner (umu|wine), proton_path, session_cookie.
    /// Leave out the value for session_cookie to paste it at a prompt
    /// (or pipe it in) so it doesn't end up in your shell history.
    Set { key: String, value: Option<String> },
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
        Command::Install {
            game,
            windows,
            package,
            build,
        } => cmd_install(&paths, &game, windows, package.as_deref(), build),
        Command::Run { game_id } => cmd_run(&paths, game_id),
        Command::List => cmd_list(&paths),
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

    let candidates = select::candidates(&payload, &Filter::default());
    if candidates.is_empty() {
        println!("  no installable Linux or Windows builds found (web-only, macOS-only, or paid/unowned)");
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
        println!(
            "  [{}] build {} v{} {} [{}]{}",
            c.package_title(),
            c.build.id,
            version,
            describe(c.platform),
            size,
            marker
        );
    }
    Ok(())
}

fn cmd_install(
    paths: &Paths,
    game: &str,
    windows_only: bool,
    package: Option<&str>,
    build: Option<u64>,
) -> Result<()> {
    let id = parse_game_id(game)?;
    let config = Config::load(paths)?;
    let api = ApiClient::new(config.session_cookie.as_deref())?;

    let title = api.game_title(id)?.unwrap_or_else(|| format!("game {id}"));
    let payload = api.packages(id)?;
    let package_id = package
        .map(|q| select::find_package(&payload, q).map(|p| p.id))
        .transpose()?;
    let filter = Filter {
        windows_only,
        package_id,
        build_id: build,
    };
    let Some(choice) = select::choose(&payload, &filter) else {
        if let Some(b) = build {
            bail!("build {b} is not an installable Linux or Windows build of {title} (see `joltux info {id}`)");
        }
        bail!("{title} has no installable Linux or Windows build matching your options");
    };
    let version = choice
        .release
        .map(|r| r.version_number.clone())
        .unwrap_or_default();
    println!(
        "Installing {title} [{}]: build {} ({})",
        choice.package_title(),
        choice.build.id,
        describe(choice.platform)
    );

    // Games with e.g. a demo and a full version: say what else exists.
    if package_id.is_none() && build.is_none() {
        let mut others: Vec<String> = select::candidates(
            &payload,
            &Filter {
                windows_only,
                ..Default::default()
            },
        )
        .iter()
        .filter(|c| c.build.game_package_id != choice.build.game_package_id)
        .map(|c| format!("\"{}\"", c.package_title()))
        .collect();
        others.sort();
        others.dedup();
        if !others.is_empty() {
            println!(
                "Note: other packages available: {} (pick one with --package)",
                others.join(", ")
            );
        }
    }

    // Work out what to launch before downloading, so we fail early if the
    // game has no launch information.
    let Some(option) =
        select::launch_option(&payload.launch_options, choice.build.id, choice.platform)
    else {
        bail!(
            "build {} has no launch option for this platform",
            choice.build.id
        );
    };

    let url = api.download_url(choice.build.id)?;
    let archive = paths
        .downloads_dir
        .join(format!("{}.archive", choice.build.id));
    install::download(&url, &archive)?;

    // Extract next to the real folder and only swap it in once it's known to
    // be good, so a failed reinstall never breaks a working install.
    let dir = paths.game_dir(id);
    let staging = install::sibling(&dir, "new");
    if staging.exists() {
        fs::remove_dir_all(&staging).context("could not clear a leftover partial install")?;
    }
    println!("Extracting...");
    let prepared = (|| -> Result<PathBuf> {
        install::extract(&archive, &staging, &choice.build.archive_type)?;
        let exe =
            install::find_executable(&staging, &option.executable_path).with_context(|| {
                format!("could not find `{}` in the archive", option.executable_path)
            })?;
        Ok(exe.strip_prefix(&staging)?.to_path_buf())
    })();
    let _ = fs::remove_file(&archive); // the archive is no longer needed either way
    let relative_exe = match prepared {
        Ok(rel) => rel,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e.context("install failed; any previous install was left untouched"));
        }
    };
    install::swap_into_place(&staging, &dir)?;

    let mut library = Library::load(paths)?;
    library.upsert(InstalledGame {
        game_id: id,
        title: title.clone(),
        build_id: choice.build.id,
        platform: choice.platform,
        version,
        executable: dir.join(relative_exe),
        install_dir: dir,
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
            let value = match value {
                Some(v) if v != "-" => v,
                _ if key == "session_cookie" => {
                    read_secret("Paste the `frontend` cookie value and press Enter: ")?
                }
                _ => bail!("missing value for `{key}`"),
            };
            config.set(&key, &value)?;
            config.save(paths)?;
            println!("Saved.");
        }
    }
    Ok(())
}

/// Reads one line from stdin, prompting only when stdin is a terminal.
/// Note: the terminal still echoes what you paste; the point is keeping the
/// value out of shell history and the process list.
fn read_secret(prompt: &str) -> Result<String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        eprint!("{prompt}");
        io::stderr().flush()?;
    }
    let mut line = String::new();
    stdin
        .lock()
        .read_line(&mut line)
        .context("could not read from stdin")?;
    Ok(line.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_game_id;

    #[test]
    fn parses_ids_and_urls() {
        assert_eq!(parse_game_id("12345").unwrap(), 12345);
        assert_eq!(
            parse_game_id("https://gamejolt.com/games/cool-game/12345").unwrap(),
            12345
        );
        assert_eq!(
            parse_game_id("https://gamejolt.com/games/cool-game/12345/").unwrap(),
            12345
        );
        assert_eq!(
            parse_game_id("https://gamejolt.com/games/cool-game/12345?ref=x#top").unwrap(),
            12345
        );
        assert!(parse_game_id("https://gamejolt.com/games/cool-game").is_err());
    }
}
