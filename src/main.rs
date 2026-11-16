//! Joltux: an unofficial Game Jolt launcher for Linux.
//!
//! Downloads games from Game Jolt, prefers native Linux builds, and runs
//! Windows builds through Proton (via umu-launcher) or Wine.

mod api;
mod checks;
mod config;
mod gui;
mod install;
mod launch;
mod library;
mod models;
mod paths;
mod runtime;
mod select;
mod store;
mod theme;
mod updates;

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::PathBuf;
use std::process::Command as ProcessCommand;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};

use crate::api::{ApiClient, LoginOutcome};
use crate::config::Config;
use crate::library::Library;
use crate::models::InstalledGame;
use crate::paths::Paths;
use crate::select::{Filter, Platform};

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
    /// Check installed games for newer builds
    Outdated,
    /// Update installed games to their newest build (all of them if no IDs are given)
    Upgrade {
        /// Game IDs (see `joltux list`)
        game_ids: Vec<u64>,
    },
    /// Show or change a game's launch options (with no flags, shows them)
    Options {
        /// Game ID (see `joltux list`)
        game_id: u64,
        /// Set an environment variable, e.g. --env DXVK_HUD=1 (repeatable)
        #[arg(long, value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// Remove an environment variable set with --env (repeatable)
        #[arg(long, value_name = "KEY")]
        unset_env: Vec<String>,
        /// Arguments passed to the game, e.g. --args="-windowed -nosound" ("" clears)
        #[arg(long, allow_hyphen_values = true)]
        args: Option<String>,
        /// Command to wrap the launch in, e.g. --wrapper="gamescope -f --" ("" clears)
        #[arg(long, allow_hyphen_values = true)]
        wrapper: Option<String>,
        /// Runner for this Windows game: umu, wine, or default (use the global setting)
        #[arg(long)]
        runner: Option<String>,
        /// Clear all of this game's launch options (applied before other flags)
        #[arg(long)]
        reset: bool,
    },
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
    /// Set a value: runner (umu|wine), umu_path, wine_path, proton_path, session_cookie.
    /// Leave out the value for session_cookie to paste it at a hidden prompt
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
        Command::Outdated => cmd_outdated(&paths),
        Command::Upgrade { game_ids } => cmd_upgrade(&paths, &game_ids),
        Command::Options {
            game_id,
            env,
            unset_env,
            args,
            wrapper,
            runner,
            reset,
        } => cmd_options(
            &paths,
            game_id,
            OptionChanges {
                env,
                unset_env,
                args,
                wrapper,
                runner,
                reset,
            },
        ),
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

    let candidates = select::candidates(&payload, &Filter::default());
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
        if package_id.is_some() {
            bail!("that package of {title} has no installable Linux or Windows build (see `joltux info {id}`)");
        }
        bail!(
            "{title} has no installable Linux or Windows build (Joltux supports .zip and .tar.gz archives, plus standalone Windows .exe files; installers are not supported)"
        );
    };
    let version = choice.release.map(|r| r.version_number.clone()).unwrap_or_default();
    println!(
        "Installing {title} [{}]: build {} ({})",
        choice.package_title(),
        choice.build.id,
        describe(choice.platform)
    );

    // Games with e.g. a demo and a full version: say what else exists.
    if package_id.is_none() && build.is_none() {
        let any_package = Filter {
            windows_only,
            ..Default::default()
        };
        let mut others: Vec<String> = select::candidates(&payload, &any_package)
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

    // Unpack next to the real folder and only swap it in once it's known to
    // be good, so a failed reinstall never breaks a working install.
    let dir = paths.game_dir(id);
    let staging = install::sibling(&dir, "new");
    if staging.exists() {
        fs::remove_dir_all(&staging).context("could not clear a leftover partial install")?;
    }
    let prepared = (|| -> Result<PathBuf> {
        let exe = if standalone_exe {
            install::install_raw_executable(&archive, &staging, &executable_path)?
        } else {
            println!("Extracting...");
            install::extract(&archive, &staging, &choice.build.archive_type)?;
            install::find_executable(&staging, &executable_path)
                .with_context(|| format!("could not find `{executable_path}` in the archive"))?
        };
        Ok(exe.strip_prefix(&staging)?.to_path_buf())
    })();
    let _ = fs::remove_file(&archive); // the download is no longer needed either way
    let relative_exe = match prepared {
        Ok(rel) => rel,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e.context("install failed; any previous install was left untouched"));
        }
    };
    install::swap_into_place(&staging, &dir)?;
    let executable = dir.join(relative_exe);

    let mut library = Library::load(paths)?;
    // Reinstalls and upgrades keep the game's launch options.
    let launch = library.find(id).map(|g| g.launch.clone()).unwrap_or_default();
    library.upsert(InstalledGame {
        game_id: id,
        title: title.clone(),
        build_id: choice.build.id,
        platform: choice.platform,
        version,
        install_dir: dir,
        executable,
        package_id: Some(choice.build.game_package_id),
        launch,
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
        let tweaked = if g.launch.is_empty() { "" } else { "  (custom launch options)" };
        println!("{:>8}  {}  [{}]{tweaked}", g.game_id, g.title, describe(g.platform));
    }
    Ok(())
}

/// Checks every installed game, printing one line each. Returns the games
/// that have an update.
fn check_all(paths: &Paths) -> Result<Vec<(InstalledGame, updates::Update)>> {
    let library = Library::load(paths)?;
    if library.games.is_empty() {
        println!("No games installed.");
        return Ok(Vec::new());
    }
    let config = Config::load(paths)?;
    let api = ApiClient::new(config.session_cookie.as_deref())?;
    let mut found = Vec::new();
    for game in library.games {
        match updates::check(&api, &game) {
            Ok(Some(update)) => {
                println!(
                    "{} ({}): {} -> {} (build {})",
                    game.title,
                    game.game_id,
                    version_label(&game.version),
                    version_label(&update.version),
                    update.build_id
                );
                found.push((game, update));
            }
            Ok(None) => println!("{} ({}): up to date", game.title, game.game_id),
            Err(e) => println!("{} ({}): could not check ({e:#})", game.title, game.game_id),
        }
    }
    Ok(found)
}

fn version_label(version: &str) -> String {
    match version.trim().trim_start_matches('v') {
        "" => "unknown version".to_owned(),
        v => format!("v{v}"),
    }
}

fn cmd_outdated(paths: &Paths) -> Result<()> {
    let found = check_all(paths)?;
    match found.len() {
        0 => {}
        1 => println!("1 update available. Install it with: joltux upgrade {}", found[0].0.game_id),
        n => println!("{n} updates available. Install them with: joltux upgrade"),
    }
    Ok(())
}

fn cmd_upgrade(paths: &Paths, game_ids: &[u64]) -> Result<()> {
    let pending = if game_ids.is_empty() {
        check_all(paths)?
    } else {
        let library = Library::load(paths)?;
        let config = Config::load(paths)?;
        let api = ApiClient::new(config.session_cookie.as_deref())?;
        let mut pending = Vec::new();
        for &id in game_ids {
            let game = library
                .find(id)
                .with_context(|| format!("game {id} is not installed (see `joltux list`)"))?;
            match updates::check(&api, game)? {
                Some(update) => pending.push((game.clone(), update)),
                None => println!("{} is already up to date.", game.title),
            }
        }
        pending
    };
    if pending.is_empty() {
        println!("Nothing to update.");
        return Ok(());
    }

    let mut failed = Vec::new();
    for (game, update) in &pending {
        println!();
        let result = cmd_install(
            paths,
            &game.game_id.to_string(),
            game.platform == Platform::Windows,
            Some(&update.package_id.to_string()),
            Some(update.build_id),
        );
        if let Err(e) = result {
            eprintln!("error: could not update {}: {e:#}", game.title);
            failed.push(game.title.as_str());
        }
    }
    if !failed.is_empty() {
        bail!("{} of {} updates failed: {}", failed.len(), pending.len(), failed.join(", "));
    }
    Ok(())
}

struct OptionChanges {
    env: Vec<String>,
    unset_env: Vec<String>,
    args: Option<String>,
    wrapper: Option<String>,
    runner: Option<String>,
    reset: bool,
}

impl OptionChanges {
    fn is_empty(&self) -> bool {
        self.env.is_empty()
            && self.unset_env.is_empty()
            && self.args.is_none()
            && self.wrapper.is_none()
            && self.runner.is_none()
            && !self.reset
    }
}

fn cmd_options(paths: &Paths, game_id: u64, changes: OptionChanges) -> Result<()> {
    let mut library = Library::load(paths)?;
    let game = library
        .find_mut(game_id)
        .with_context(|| format!("game {game_id} is not installed (see `joltux list`)"))?;

    if !changes.is_empty() {
        // Validate everything before changing anything.
        let set: Vec<(String, String)> =
            changes.env.iter().map(|e| launch::parse_env(e)).collect::<Result<_>>()?;
        let args = changes.args.as_deref().map(launch::split_words).transpose()?;
        let wrapper = changes.wrapper.as_deref().map(launch::split_words).transpose()?;
        let runner = match changes.runner.as_deref() {
            None => None,
            Some(_) if game.platform == Platform::Linux => {
                bail!("{} is a native Linux build; --runner only applies to Windows builds", game.title)
            }
            Some("default") => Some(None),
            Some("umu") => Some(Some(config::Runner::Umu)),
            Some("wine") => Some(Some(config::Runner::Wine)),
            Some(other) => bail!("runner must be `umu`, `wine` or `default`, not `{other}`"),
        };

        let launch = &mut game.launch;
        if changes.reset {
            *launch = Default::default();
        }
        for key in &changes.unset_env {
            if launch.env.remove(key).is_none() {
                eprintln!("note: {key} was not set");
            }
        }
        launch.env.extend(set);
        if let Some(args) = args {
            launch.args = args;
        }
        if let Some(wrapper) = wrapper {
            launch.wrapper = wrapper;
        }
        if let Some(runner) = runner {
            launch.runner = runner;
        }
        library.save(paths)?;
        println!("Saved.");
    }

    let game = library.find(game_id).expect("game is still in the library");
    print_launch_options(game, &Config::load(paths)?);
    Ok(())
}

fn print_launch_options(game: &InstalledGame, config: &Config) {
    let launch = &game.launch;
    println!("Launch options for {} ({}):", game.title, game.game_id);
    if game.platform == Platform::Windows {
        let runner = match launch.runner {
            Some(r) => format!("{r:?} (this game only)").to_lowercase(),
            None => format!("{:?} (global setting)", config.runner).to_lowercase(),
        };
        println!("  runner:  {runner}");
    }
    let or_none = |words: &[String]| {
        if words.is_empty() { "(none)".to_owned() } else { launch::join_words(words) }
    };
    println!("  wrapper: {}", or_none(&launch.wrapper));
    println!("  args:    {}", or_none(&launch.args));
    if launch.env.is_empty() {
        println!("  env:     (none)");
    } else {
        println!("  env:");
        for (key, value) in &launch.env {
            println!("    {key}={value}");
        }
    }
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
            let value = match value {
                Some(v) if v != "-" => v,
                _ if key == "session_cookie" => read_secret("frontend cookie: ")?,
                _ => bail!("missing value for `{key}`"),
            };
            config.set(&key, &value)?;
            config.save(paths)?;
            println!("Saved.");
        }
    }
    Ok(())
}

/// Reads a secret without echoing it when run in a terminal, or one line
/// from stdin when it's piped in.
fn read_secret(prompt: &str) -> Result<String> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        return rpassword::prompt_password(prompt)
            .map(|s| s.trim().to_string())
            .context("could not read from the terminal");
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
