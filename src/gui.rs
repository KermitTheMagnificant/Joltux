use std::collections::HashMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result};
use eframe::egui;

use crate::api::{ApiClient, LoginOutcome};
use crate::config::{Config, Runner};
use crate::desktop::{self, RunningGame};
use crate::launch;
use crate::library::{self, Library};
use crate::models::{GameSummary, InstalledGame, LaunchSettings};
use crate::paths::Paths;
use crate::runtime;
use crate::select::Platform;
use crate::store::{self, Store, StoreAction};
use crate::theme::{self, icon, AccentButton};
use crate::updates::{self, Update};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Store,
    Game,
    Library,
    Install,
    Account,
    Settings,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Store => "Store",
            Page::Game => "Game preview",
            Page::Library => "Library",
            Page::Install => "Add a game",
            Page::Account => "Account",
            Page::Settings => "Settings",
        }
    }
}

/// Sidebar entries, top to bottom.
const NAV: [(Page, &str, &str); 5] = [
    (Page::Store, icon::STORE, "Store"),
    (Page::Library, icon::LIBRARY, "Library"),
    (Page::Install, icon::ADD, "Add a game"),
    (Page::Account, icon::ACCOUNT, "Account"),
    (Page::Settings, icon::SETTINGS, "Settings"),
];
const NAV_ROW: f32 = 40.0;
const NAV_GAP: f32 = 4.0;
/// Seconds a page takes to fade and slide in.
const PAGE_FADE: f64 = 0.22;
/// Forms read better when they do not stretch across a wide window.
const FORM_WIDTH: f32 = 640.0;

/// The app icon, embedded so the window has it even when not installed.
fn window_icon() -> egui::IconData {
    let png = include_bytes!("../assets/joltux-icon-256.png");
    match image::load_from_memory_with_format(png, image::ImageFormat::Png) {
        Ok(image) => {
            let image = image.into_rgba8();
            let (width, height) = image.dimensions();
            egui::IconData { rgba: image.into_raw(), width, height }
        }
        Err(_) => egui::IconData::default(),
    }
}

/// Messages from a background task to the GUI.
enum TaskMessage {
    /// Everything the task has printed so far.
    Output(String),
    /// The task finished; this becomes the status line.
    Done(String),
}

/// What a running CLI task does to which games, so the library can block
/// conflicting actions and a notification can say what finished.
struct GameTask {
    kind: TaskKind,
    games: Vec<u64>,
    /// Title to use if the game isn't in the library (a failed install).
    title: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskKind {
    Install,
    Update,
    Remove,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LibrarySort {
    Recent,
    Name,
}

/// Something done to one game from its library row or its menu.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowAction {
    Play,
    Stop,
    Update,
    Reinstall,
    Remove,
    ToggleOptions,
    OpenFolder,
    OpenPrefix,
    OpenLog,
    ViewOnGameJolt,
    CopyId,
}

/// What a game's row and menu need to know about it.
struct RowState {
    running: bool,
    stopping: bool,
    /// A task (install, update, remove) is running for this game.
    in_task: bool,
    /// Any task is running.
    busy: bool,
    has_update: bool,
    prefix_exists: bool,
    log_exists: bool,
}

/// Result of checking the whole library for updates.
struct UpdateReport {
    found: HashMap<u64, Update>,
    /// Games that couldn't be checked (offline, removed from Game Jolt, ...).
    failed: usize,
}

/// The launch options of one game, as text while being edited.
struct LaunchEditor {
    game_id: u64,
    runner: Option<Runner>,
    wrapper: String,
    args: String,
    /// One KEY=VALUE per line.
    env: String,
}

impl LaunchEditor {
    fn new(game: &InstalledGame) -> Self {
        let launch = &game.launch;
        Self {
            game_id: game.game_id,
            runner: launch.runner,
            wrapper: launch::join_words(&launch.wrapper),
            args: launch::join_words(&launch.args),
            env: launch
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    fn settings(&self) -> Result<LaunchSettings> {
        let env = self
            .env
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(launch::parse_env)
            .collect::<Result<_>>()?;
        Ok(LaunchSettings {
            env,
            args: launch::split_words(&self.args).context("game arguments")?,
            wrapper: launch::split_words(&self.wrapper).context("wrapper command")?,
            runner: self.runner,
        })
    }
}

pub fn run() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            // Matches the installed joltux.desktop, so Wayland desktops pick up its icon.
            .with_app_id("joltux")
            .with_icon(window_icon())
            .with_inner_size([1040.0, 720.0])
            .with_min_inner_size([760.0, 520.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Joltux",
        options,
        Box::new(|cc| Ok(Box::new(JoltuxApp::new(cc)?))),
    )
    .map_err(|error| anyhow::anyhow!(error.to_string()))
}

struct JoltuxApp {
    paths: Paths,
    page: Page,
    page_history: Vec<Page>,
    /// When the current page was opened (egui time), for its fade-in. `None`
    /// means "just navigated"; the next frame fills it in.
    page_since: Option<f64>,
    games: Vec<InstalledGame>,
    config: Config,
    receiver: Receiver<TaskMessage>,
    sender: Sender<TaskMessage>,
    /// Live output of the running command, shown under the status.
    task_output: String,
    busy: bool,
    status: String,
    store: Store,
    game_input: String,
    windows_only: bool,
    remove_prefix: bool,
    login_username: String,
    login_password: String,
    cookie_input: String,
    show_cookie: bool,
    /// Newer builds of installed games, by game ID.
    updates: HashMap<u64, Update>,
    /// Set while an update check runs in the background.
    update_check: Option<Receiver<UpdateReport>>,
    /// The library is checked once automatically, the first time it's shown.
    updates_checked: bool,
    /// Outcome of the last update check, shown above the library.
    update_note: String,
    launch_editor: Option<LaunchEditor>,
    /// Games started from the app, by game ID.
    running: HashMap<u64, RunningGame>,
    /// The games the running CLI task affects.
    task: Option<GameTask>,
    library_sort: LibrarySort,
    /// Fetches slugs and cover art for games installed before Joltux saved them.
    cover_backfill: Option<Receiver<Vec<(u64, GameSummary)>>>,
}

impl JoltuxApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let paths = Paths::discover()?;
        let (sender, receiver) = mpsc::channel();
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);

        let games = Library::load(&paths)?.games;
        let config = Config::load(&paths)?;
        Ok(Self {
            paths,
            page: Page::Store,
            page_history: Vec::new(),
            page_since: None,
            games,
            config,
            receiver,
            sender,
            task_output: String::new(),
            busy: false,
            status: String::new(),
            store: Store::new(),
            game_input: String::new(),
            windows_only: false,
            remove_prefix: false,
            login_username: String::new(),
            login_password: String::new(),
            cookie_input: String::new(),
            show_cookie: false,
            updates: HashMap::new(),
            update_check: None,
            updates_checked: false,
            update_note: String::new(),
            launch_editor: None,
            running: HashMap::new(),
            task: None,
            library_sort: LibrarySort::Recent,
            cover_backfill: None,
        }
        .with_cover_backfill())
    }

    /// Starts fetching slugs and covers for games that lack them.
    fn with_cover_backfill(mut self) -> Self {
        let missing: Vec<u64> = self
            .games
            .iter()
            .filter(|g| g.thumbnail_url.is_none() || g.slug.is_none())
            .map(|g| g.game_id)
            .collect();
        if missing.is_empty() {
            return self;
        }
        let session = self.config.session_cookie.clone();
        let (sender, receiver) = mpsc::channel();
        self.cover_backfill = Some(receiver);
        std::thread::spawn(move || {
            let Ok(api) = ApiClient::new(session.as_deref()) else { return };
            let found = missing
                .into_iter()
                .filter_map(|id| api.game_summary(id).ok().map(|summary| (id, summary)))
                .collect();
            let _ = sender.send(found);
        });
        self
    }

    /// Saves backfilled covers into the library. Waits while a task runs, so
    /// this never races a CLI command writing the library.
    fn poll_cover_backfill(&mut self) {
        if self.busy {
            return;
        }
        let Some(receiver) = &self.cover_backfill else { return };
        let found = match receiver.try_recv() {
            Ok(found) => found,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => Vec::new(),
        };
        self.cover_backfill = None;
        if found.is_empty() {
            return;
        }
        let saved = Library::load(&self.paths).and_then(|mut library| {
            for (id, summary) in found {
                if let Some(game) = library.find_mut(id) {
                    game.slug = game.slug.take().or(summary.slug);
                    game.thumbnail_url = game.thumbnail_url.take().or(summary.img_thumbnail);
                }
            }
            library.save(&self.paths)
        });
        if saved.is_ok() {
            self.refresh();
        }
    }

    /// Runs a CLI command that installs, updates or removes `games`.
    fn run_game_task(&mut self, args: Vec<String>, kind: TaskKind, games: Vec<u64>, title: Option<String>) {
        self.run_cli(args);
        if self.busy && self.task.is_none() {
            self.task = Some(GameTask { kind, games, title });
        }
    }

    /// Called when a CLI task finishes, after the library was reloaded.
    /// Notifies the desktop about installs and updates when the window isn't
    /// focused, since those can take a while.
    fn finish_task(&mut self, window_focused: bool) {
        let Some(task) = self.task.take() else { return };
        if window_focused || task.kind == TaskKind::Remove {
            return;
        }
        let failed = is_failure(&self.status);
        let titles: Vec<String> = task
            .games
            .iter()
            .map(|id| match self.games.iter().find(|g| g.game_id == *id) {
                Some(game) => game.title.clone(),
                None => task.title.clone().unwrap_or_else(|| format!("game {id}")),
            })
            .collect();
        let name = match titles.as_slice() {
            [one] => one.clone(),
            [] => "your games".to_owned(),
            many => format!("{} games", many.len()),
        };
        let (summary, body) = match (task.kind, failed) {
            (TaskKind::Install, false) => (format!("{name} is installed"), "Ready to play.".to_owned()),
            (TaskKind::Install, true) => (format!("Couldn't install {name}"), first_line(&self.status)),
            (_, false) => (format!("Updated {name}"), titles.join(", ")),
            (_, true) => (format!("Couldn't update {name}"), first_line(&self.status)),
        };
        desktop::notify(&summary, &body);
    }

    fn play(&mut self, game: &InstalledGame) {
        if self.running.contains_key(&game.game_id) {
            return;
        }
        match RunningGame::start(&self.paths, game.game_id) {
            Ok(running) => {
                self.running.insert(game.game_id, running);
                self.status = format!("Playing {}.", game.title);
            }
            Err(error) => self.status = format!("Could not start {}: {error:#}", game.title),
        }
    }

    /// Notices games that have exited.
    fn poll_running(&mut self) {
        let mut exited = Vec::new();
        for (id, game) in &mut self.running {
            if let Some(status) = game.finished() {
                exited.push((*id, status, game.started.elapsed().as_secs(), game.stopping));
            }
        }
        if exited.is_empty() {
            return;
        }
        for (id, ..) in &exited {
            self.running.remove(id);
        }
        // Picks up the playtime `joltux run` saved.
        self.refresh();
        for (id, status, secs, stopped) in exited {
            let title = self
                .games
                .iter()
                .find(|g| g.game_id == id)
                .map_or_else(|| format!("game {id}"), |g| g.title.clone());
            self.status = if status.success() || stopped {
                format!("Finished playing {title} ({}).", library::format_playtime(secs))
            } else {
                let log = desktop::log_tail(&self.paths.log_file(id), 4);
                format!("Could not run {title}:\n{log}")
            };
        }
    }

    fn row_state(&self, game: &InstalledGame) -> RowState {
        let running = self.running.get(&game.game_id);
        RowState {
            running: running.is_some(),
            stopping: running.is_some_and(|r| r.stopping),
            in_task: self.task.as_ref().is_some_and(|t| t.games.contains(&game.game_id)),
            busy: self.busy,
            has_update: self.updates.contains_key(&game.game_id),
            prefix_exists: game.platform == Platform::Windows && self.paths.prefix_dir(game.game_id).is_dir(),
            log_exists: self.paths.log_file(game.game_id).is_file(),
        }
    }

    fn apply_row_action(&mut self, ctx: &egui::Context, game: &InstalledGame, action: RowAction) {
        let id = game.game_id;
        let opened = match action {
            RowAction::Play => {
                self.play(game);
                Ok(())
            }
            RowAction::Stop => {
                if let Some(running) = self.running.get_mut(&id) {
                    running.stop();
                }
                Ok(())
            }
            RowAction::Update => {
                self.run_game_task(vec!["upgrade".into(), id.to_string()], TaskKind::Update, vec![id], None);
                if self.busy {
                    self.status = format!("Updating {}...", game.title);
                }
                Ok(())
            }
            RowAction::Reinstall => {
                let mut args = vec!["install".into(), id.to_string(), "--build".into(), game.build_id.to_string()];
                if game.platform == Platform::Windows {
                    args.push("--windows".into());
                }
                self.run_game_task(args, TaskKind::Install, vec![id], Some(game.title.clone()));
                if self.busy {
                    self.status = format!("Reinstalling {}...", game.title);
                }
                Ok(())
            }
            RowAction::Remove => {
                let mut args = vec!["uninstall".into(), id.to_string()];
                if self.remove_prefix {
                    args.push("--prefix".into());
                }
                self.run_game_task(args, TaskKind::Remove, vec![id], None);
                Ok(())
            }
            RowAction::ToggleOptions => {
                let editing = self.launch_editor.as_ref().is_some_and(|e| e.game_id == id);
                self.launch_editor = (!editing).then(|| LaunchEditor::new(game));
                Ok(())
            }
            RowAction::OpenFolder => desktop::open(&game.install_dir),
            RowAction::OpenPrefix => desktop::open(self.paths.prefix_dir(id)),
            RowAction::OpenLog => desktop::open(self.paths.log_file(id)),
            RowAction::ViewOnGameJolt => desktop::open(game_page_url(game)),
            RowAction::CopyId => {
                ctx.copy_text(id.to_string());
                self.status = format!("Copied the ID of {} ({id}).", game.title);
                Ok(())
            }
        };
        if let Err(error) = opened {
            self.status = format!("Could not open it: {error:#}");
        }
    }

    /// Checks every installed game for a newer build on a background thread.
    fn check_updates(&mut self) {
        if self.update_check.is_some() || self.games.is_empty() {
            return;
        }
        self.updates_checked = true;
        let games = self.games.clone();
        let session = self.config.session_cookie.clone();
        let (sender, receiver) = mpsc::channel();
        self.update_check = Some(receiver);
        std::thread::spawn(move || {
            let mut report = UpdateReport { found: HashMap::new(), failed: games.len() };
            if let Ok(api) = ApiClient::new(session.as_deref()) {
                report.failed = 0;
                for game in &games {
                    match updates::check(&api, game) {
                        Ok(Some(update)) => {
                            report.found.insert(game.game_id, update);
                        }
                        Ok(None) => {}
                        Err(_) => report.failed += 1,
                    }
                }
            }
            let _ = sender.send(report);
        });
    }

    fn poll_update_check(&mut self) {
        let Some(receiver) = &self.update_check else { return };
        let report = match receiver.try_recv() {
            Ok(report) => report,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => UpdateReport { found: HashMap::new(), failed: self.games.len() },
        };
        self.update_check = None;
        self.updates = report.found;
        self.forget_applied_updates();
        let found = self.updates.len();
        let mut note = match found {
            0 => "Everything is up to date".to_owned(),
            1 => "1 update available".to_owned(),
            n => format!("{n} updates available"),
        };
        if report.failed > 0 {
            note = format!(
                "{note} · couldn't check {} game{}",
                report.failed,
                if report.failed == 1 { "" } else { "s" }
            );
        }
        self.update_note = note;
    }

    /// Drops updates for games that were since updated or removed.
    fn forget_applied_updates(&mut self) {
        let games = &self.games;
        self.updates.retain(|id, update| {
            games.iter().any(|g| g.game_id == *id && g.build_id != update.build_id)
        });
    }

    fn save_launch_options(&mut self) {
        let Some(editor) = &self.launch_editor else { return };
        let game_id = editor.game_id;
        let result = editor.settings().and_then(|settings| {
            let mut library = Library::load(&self.paths)?;
            let game = library.find_mut(game_id).context("the game is no longer installed")?;
            game.launch = settings;
            let title = game.title.clone();
            library.save(&self.paths)?;
            Ok(title)
        });
        match result {
            Ok(title) => {
                self.status = format!("Saved launch options for {title}.");
                self.launch_editor = None;
                self.refresh();
            }
            Err(error) => self.status = format!("Could not save launch options: {error:#}"),
        }
    }

    fn navigate_to(&mut self, page: Page) {
        if self.page != page {
            self.page_history.push(self.page);
            self.page = page;
            self.page_since = None;
        }
    }

    fn go_back(&mut self) {
        if let Some(page) = self.page_history.pop() {
            self.page = page;
            self.page_since = None;
        }
    }

    fn start_task(&mut self, task: impl FnOnce(&Sender<TaskMessage>) -> String + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Working...".to_owned();
        self.task_output.clear();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = task(&sender);
            let _ = sender.send(TaskMessage::Done(result));
        });
    }

    fn run_cli(&mut self, args: Vec<String>) {
        self.start_task(move |sender| match run_streaming(args, sender) {
            Ok((true, text)) if text.trim().is_empty() => "Done.".to_owned(),
            Ok((true, text)) => text,
            Ok((false, text)) if text.trim().is_empty() => "Command failed.".to_owned(),
            Ok((false, text)) => format!("Command failed: {text}"),
            Err(error) => format!("Error: {error:#}"),
        });
    }

    fn login(&mut self) {
        let username = self.login_username.trim().to_owned();
        let password = std::mem::take(&mut self.login_password);
        self.start_task(move |_| login_task(username, password));
    }

    fn refresh(&mut self) {
        match Library::load(&self.paths) {
            Ok(library) => {
                self.games = library.games;
                self.forget_applied_updates();
            }
            Err(error) => self.status = format!("Could not reload library: {error:#}"),
        }
        match Config::load(&self.paths) {
            Ok(config) => self.config = config,
            Err(error) => self.status = format!("Could not reload settings: {error:#}"),
        }
    }

    fn store_page(&mut self, ui: &mut egui::Ui) {
        let session = self.config.session_cookie.clone();
        let action = if self.page == Page::Game {
            self.store.preview_ui(ui, session.as_deref(), self.busy)
        } else {
            self.store.ui(ui, session.as_deref())
        };
        match action {
            None => {}
            Some(StoreAction::OpenGame) => self.navigate_to(Page::Game),
            Some(StoreAction::ReviewBuilds { game_id, title }) => {
                self.game_input = game_id.to_string();
                self.windows_only = false;
                self.navigate_to(Page::Install);
                self.status = format!("Reviewing builds for {title}...");
                self.run_cli(vec!["info".into(), game_id.to_string()]);
            }
            Some(StoreAction::Install { game_id, title }) => {
                self.game_input = game_id.to_string();
                self.run_game_task(
                    vec!["install".into(), game_id.to_string()],
                    TaskKind::Install,
                    vec![game_id],
                    Some(title.clone()),
                );
                if self.busy {
                    self.status = format!("Installing {title}... this can take a while for large games.");
                }
            }
        }
    }

    fn library_page(&mut self, ui: &mut egui::Ui) {
        if self.games.is_empty() {
            ui.add_space(ui.available_height() * 0.22);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(icon::LIBRARY).size(56.0).color(theme::BORDER));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Nothing installed yet").size(20.0).color(egui::Color32::WHITE));
                ui.label(egui::RichText::new("Games you install show up here, ready to play.").color(theme::TEXT_DIM));
                ui.add_space(10.0);
                if ui.add(AccentButton::new("Browse the store")).clicked() {
                    self.navigate_to(Page::Store);
                }
                if ui.button("Add by ID or URL").clicked() {
                    self.navigate_to(Page::Install);
                }
            });
            return;
        }

        if !self.updates_checked {
            self.check_updates();
        }
        ui.horizontal(|ui| {
            let count = self.games.len();
            ui.label(
                egui::RichText::new(format!("{count} game{} installed", if count == 1 { "" } else { "s" }))
                    .color(theme::TEXT_DIM),
            );
            if self.update_check.is_some() {
                ui.add(egui::Spinner::new().size(12.0).color(theme::TEXT_DIM));
                ui.label(egui::RichText::new("Checking for updates...").small().color(theme::TEXT_DIM));
            } else if !self.update_note.is_empty() {
                let color = if self.updates.is_empty() { theme::TEXT_DIM } else { theme::AMBER };
                ui.label(egui::RichText::new(format!("·  {}", self.update_note)).small().color(color));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!self.busy, egui::Button::new("Refresh")).clicked() {
                    self.refresh();
                }
                let idle = !self.busy && self.update_check.is_none();
                if ui.add_enabled(idle, egui::Button::new("Check for updates")).clicked() {
                    self.check_updates();
                }
                // Games that are running are left for later.
                let ready: Vec<u64> =
                    self.updates.keys().copied().filter(|id| !self.running.contains_key(id)).collect();
                if !self.updates.is_empty() {
                    let label = format!("Update all ({})", self.updates.len());
                    let button = ui
                        .add_enabled(idle && !ready.is_empty(), AccentButton::new(label))
                        .on_disabled_hover_text("Close running games to update them");
                    if button.clicked() {
                        let mut args = vec!["upgrade".to_owned()];
                        args.extend(ready.iter().map(u64::to_string));
                        self.run_game_task(args, TaskKind::Update, ready, None);
                        if self.busy {
                            self.status = "Updating games...".into();
                        }
                    }
                }
                theme::segmented(
                    ui,
                    "library_sort",
                    &mut self.library_sort,
                    &[(LibrarySort::Name, "A–Z"), (LibrarySort::Recent, "Recent")],
                );
            });
        });
        ui.add_space(4.0);

        // Leave room under the list for the prefix option.
        let list_height = (ui.available_height() - 40.0).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let mut games = self.games.clone();
                if self.library_sort == LibrarySort::Recent {
                    // Running games count as played now. The library is
                    // already sorted by title, and the sort is stable.
                    let now = library::now_secs();
                    let running = &self.running;
                    games.sort_by_key(|g| {
                        std::cmp::Reverse(if running.contains_key(&g.game_id) { Some(now) } else { g.last_played })
                    });
                }
                for game in games {
                    self.library_row(ui, &game);
                    ui.add_space(2.0);
                }
            });

        ui.add_space(4.0);
        theme::toggle(
            ui,
            &mut self.remove_prefix,
            "Also remove Wine/Proton prefixes when removing games",
        );
    }

    fn library_row(&mut self, ui: &mut egui::Ui, game: &InstalledGame) {
        let linux = game.platform == Platform::Linux;
        let tint = if linux { theme::ACCENT } else { theme::BLUE };
        let state = self.row_state(game);
        let mut action = None;

        // Hover is read from the previous frame, because the card's background
        // has to be chosen before its contents are laid out.
        let id = ui.id().with(("library_row", game.game_id));
        let was_hovered = ui.ctx().data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
        let hover = ui.ctx().animate_bool_with_time(id, was_hovered, theme::FAST);
        let border = if state.running { theme::ACCENT_BRIGHT } else { tint };

        // The whole card senses clicks for the right-click menu and
        // double-click to play; its buttons sit on top and still get theirs.
        let card = ui.scope_builder(
            egui::UiBuilder::new().id_salt(("library_card", game.game_id)).sense(egui::Sense::click()),
            |ui| {
                theme::card()
                    .fill(theme::mix(theme::SURFACE, theme::SURFACE_HI, hover))
                    .stroke(egui::Stroke::new(1.0_f32, theme::mix(theme::BORDER, border, hover * 0.8)))
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            cover_tile(ui, game, tint);
                            ui.add_space(4.0);
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if let Some(chosen) = self.row_buttons(ui, game, &state) {
                                    action = Some(chosen);
                                }
                                // Whatever is left belongs to the title, which truncates.
                                ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                    self.row_details(ui, game, &state, tint);
                                });
                            });
                        });
                        if self.launch_editor.as_ref().is_some_and(|e| e.game_id == game.game_id) {
                            ui.add_space(8.0);
                            ui.separator();
                            self.launch_editor_ui(ui, game);
                        }
                    });
            },
        );
        let response = card.response;
        response.context_menu(|ui| {
            if let Some(chosen) = game_menu(ui, game, &state) {
                action = Some(chosen);
                ui.close_menu();
            }
        });
        if response.double_clicked() && !state.running && !state.in_task {
            action = Some(RowAction::Play);
        }
        let hovered = ui.rect_contains_pointer(response.rect);
        ui.ctx().data_mut(|data| data.insert_temp(id, hovered));

        if let Some(action) = action {
            self.apply_row_action(ui.ctx(), game, action);
        }
    }

    /// Play/Stop, the menu button and Update, laid out right to left.
    fn row_buttons(&self, ui: &mut egui::Ui, game: &InstalledGame, state: &RowState) -> Option<RowAction> {
        let mut action = None;
        if state.running {
            let label = if state.stopping { "Force stop" } else { "Stop" };
            let hover = if state.stopping {
                "The game didn't quit; kill it"
            } else {
                "Ask the game to quit"
            };
            if ui.add(AccentButton::danger(format!("{}  {label}", icon::STOP))).on_hover_text(hover).clicked() {
                action = Some(RowAction::Stop);
            }
        } else if ui
            .add_enabled(!state.in_task, AccentButton::new(format!("{}  Play", icon::PLAY)))
            .clicked()
        {
            action = Some(RowAction::Play);
        }
        let menu = ui.menu_button(icon::MORE, |ui| {
            let chosen = game_menu(ui, game, state);
            if chosen.is_some() {
                ui.close_menu();
            }
            chosen
        });
        if let Some(chosen) = menu.inner.flatten() {
            action = Some(chosen);
        }
        menu.response.on_hover_text("More actions (or right-click the game)");
        if let Some(update) = self.updates.get(&game.game_id) {
            let button = egui::Button::new(egui::RichText::new("Update").color(theme::AMBER));
            let hover = format!("Install {} (build {})", version_text(&update.version), update.build_id);
            if ui
                .add_enabled(!state.busy && !state.running, button)
                .on_hover_text(hover)
                .on_disabled_hover_text("Close the game to update it")
                .clicked()
            {
                action = Some(RowAction::Update);
            }
        }
        action
    }

    /// Title, badges and play statistics.
    fn row_details(&self, ui: &mut egui::Ui, game: &InstalledGame, state: &RowState, tint: egui::Color32) {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.spacing_mut().interact_size.y = 18.0;
        ui.add_space(3.0);
        ui.add(
            egui::Label::new(egui::RichText::new(&game.title).size(16.0).color(egui::Color32::WHITE))
                .truncate()
                .selectable(false),
        );
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if let Some(running) = self.running.get(&game.game_id) {
                let played = library::format_playtime(running.started.elapsed().as_secs());
                theme::badge(ui, &format!("{}  Playing · {played}", icon::PLAY), theme::ACCENT_BRIGHT);
            }
            theme::badge(ui, if game.platform == Platform::Linux { "Linux" } else { "Windows · Proton/Wine" }, tint);
            if let Some(update) = self.updates.get(&game.game_id) {
                theme::badge(ui, &format!("{} available", version_text(&update.version)), theme::AMBER);
            }
            if !game.launch.is_empty() {
                theme::badge(ui, "Custom launch", theme::TEXT_DIM);
            }
            let mut details = vec![version_text(&game.version)];
            if game.playtime_secs > 0 {
                details.push(format!("{} played", library::format_playtime(game.playtime_secs)));
            }
            if !state.running {
                details.push(match game.last_played {
                    Some(when) => format!("last played {}", library::format_ago(when, library::now_secs())),
                    None => "not played yet".to_owned(),
                });
            }
            ui.add(
                egui::Label::new(egui::RichText::new(details.join("  ·  ")).small().color(theme::TEXT_DIM))
                    .truncate()
                    .selectable(false),
            );
        });
    }

    /// The launch options form shown under a library row.
    fn launch_editor_ui(&mut self, ui: &mut egui::Ui, game: &InstalledGame) {
        let (mut save, mut cancel) = (false, false);
        let busy = self.busy;
        let global = self.config.runner;
        let Some(editor) = &mut self.launch_editor else { return };
        ui.add_space(4.0);
        if game.platform == Platform::Windows {
            theme::caption(ui, "Run this game with");
            let global_label = match global {
                Runner::Umu => "Global (umu)",
                Runner::Wine => "Global (Wine)",
            };
            theme::segmented(
                ui,
                "launch_runner",
                &mut editor.runner,
                &[(None, global_label), (Some(Runner::Umu), "umu / Proton"), (Some(Runner::Wine), "Wine")],
            );
            ui.add_space(2.0);
        }
        theme::caption(ui, "Wrapper command");
        ui.add(
            theme::field(&mut editor.wrapper)
                .hint_text("e.g. gamemoderun, mangohud, gamescope -f --")
                .desired_width(f32::INFINITY),
        );
        theme::caption(ui, "Game arguments");
        ui.add(
            theme::field(&mut editor.args)
                .hint_text("e.g. -windowed")
                .desired_width(f32::INFINITY),
        );
        theme::caption(ui, "Environment variables, one KEY=VALUE per line");
        ui.add(
            egui::TextEdit::multiline(&mut editor.env)
                .margin(egui::Margin::symmetric(10, 7))
                .hint_text(if game.platform == Platform::Windows {
                    "DXVK_HUD=fps\nPROTON_USE_WINED3D=1"
                } else {
                    "SDL_VIDEODRIVER=x11"
                })
                .font(egui::TextStyle::Monospace)
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            save = ui.add_enabled(!busy, AccentButton::new("Save")).clicked();
            cancel = ui.button("Cancel").clicked();
            if ui.button("Clear all").clicked() {
                editor.runner = None;
                editor.wrapper.clear();
                editor.args.clear();
                editor.env.clear();
            }
            ui.label(egui::RichText::new("Used the next time you press Play.").small().color(theme::TEXT_DIM));
        });
        if save {
            self.save_launch_options();
        } else if cancel {
            self.launch_editor = None;
        }
    }

    fn install_page(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, "Install from Game Jolt");
            ui.label(
                egui::RichText::new("The native Linux build is used when the game has one.")
                    .color(theme::TEXT_DIM),
            );
            ui.add_space(6.0);
            theme::caption(ui, "Game ID or URL");
            ui.add(
                theme::field(&mut self.game_input)
                    .hint_text("123456 or https://gamejolt.com/games/name/123456")
                    .desired_width(f32::INFINITY),
            );
            theme::toggle(ui, &mut self.windows_only, "Prefer the Windows build");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let ready = !self.busy && !self.game_input.trim().is_empty();
                if ui.add_enabled(ready, AccentButton::new("Install")).clicked() {
                    let mut args = vec!["install".into(), self.game_input.trim().to_owned()];
                    if self.windows_only {
                        args.push("--windows".into());
                    }
                    let games = crate::parse_game_id(&self.game_input).into_iter().collect();
                    self.run_game_task(args, TaskKind::Install, games, None);
                }
                if ui.add_enabled(ready, egui::Button::new("Inspect builds")).clicked() {
                    self.run_cli(vec!["info".into(), self.game_input.trim().to_owned()]);
                }
            });
        });
    }

    fn account_page(&mut self, ui: &mut egui::Ui) {
        let signed_in = self.config.session_cookie.is_some();
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, "Game Jolt account");
            ui.horizontal(|ui| {
                theme::dot(ui, if signed_in { theme::ACCENT_BRIGHT } else { theme::AMBER });
                ui.label(if signed_in {
                    "A Game Jolt session is saved on this computer."
                } else {
                    "Sign in to connect your Game Jolt account."
                });
            });
            ui.add_space(6.0);

            theme::caption(ui, "Username");
            ui.add(theme::field(&mut self.login_username).desired_width(320.0));
            theme::caption(ui, "Password");
            ui.add(theme::field(&mut self.login_password).password(true).desired_width(320.0));
            ui.add_space(4.0);

            ui.horizontal(|ui| {
                if ui.add_enabled(!self.busy, AccentButton::new("Log in")).clicked() {
                    self.login();
                }
                if ui
                    .add_enabled(!self.busy && signed_in, egui::Button::new("Log out"))
                    .clicked()
                {
                    self.run_cli(vec!["logout".into()]);
                }
            });
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "If login asks for a CAPTCHA, sign in through your browser and enter the \
                     frontend cookie in Settings.",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        });

        ui.add_space(6.0);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, "Current configuration");
            ui.label(egui::RichText::new(self.config.redacted()).monospace().color(theme::TEXT_DIM));
        });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, "Windows games");
            ui.add_space(2.0);
            theme::caption(ui, "Run Windows builds with");
            theme::segmented(
                ui,
                "runner",
                &mut self.config.runner,
                &[(Runner::Umu, "umu / Proton"), (Runner::Wine, "Wine")],
            );
            ui.add_space(2.0);
            theme::caption(ui, "umu-run path");
            ui.add(theme::field(&mut self.config.umu_path).desired_width(f32::INFINITY));
            theme::caption(ui, "Wine path");
            ui.add(theme::field(&mut self.config.wine_path).desired_width(f32::INFINITY));
            theme::caption(ui, "Proton path (a folder, or a name such as GE-Proton)");
            ui.add(theme::field(&mut self.config.proton_path).desired_width(f32::INFINITY));
        });

        ui.add_space(6.0);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, "Game Jolt session cookie");
            ui.label(
                egui::RichText::new(if self.config.session_cookie.is_some() {
                    "A cookie is saved. Enter a new one to replace it."
                } else {
                    "Only needed if logging in on the Account page asks for a CAPTCHA."
                })
                .color(theme::TEXT_DIM),
            );
            // Right to left, so the text box takes whatever the checkbox leaves.
            let row = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
            ui.allocate_ui_with_layout(row, egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::toggle(ui, &mut self.show_cookie, "Show");
                ui.add(
                    theme::field(&mut self.cookie_input)
                        .password(!self.show_cookie)
                        .hint_text("frontend cookie value")
                        .desired_width(f32::INFINITY),
                );
            });
        });

        ui.add_space(6.0);
        if ui.add_enabled(!self.busy, AccentButton::new("Save settings")).clicked() {
            self.config.session_cookie = if self.cookie_input.is_empty() {
                self.config.session_cookie.take()
            } else {
                Some(std::mem::take(&mut self.cookie_input))
            };
            self.status = match self.config.save(&self.paths) {
                Ok(()) => {
                    "Settings saved. Session cookie is stored in the owner-only config file.".into()
                }
                Err(error) => format!("Could not save settings: {error:#}"),
            };
        }

        ui.add_space(6.0);
        theme::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section_title(ui, "Maintenance");
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Set up Wine + umu"))
                    .clicked()
                {
                    self.run_cli(vec!["setup".into()]);
                }
                ui.label(
                    egui::RichText::new("Installs what Windows games need. Asks for your password.")
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Check for updates"))
                    .clicked()
                {
                    self.run_cli(vec!["update".into()]);
                }
                ui.label(
                    egui::RichText::new(concat!("Joltux ", env!("CARGO_PKG_VERSION")))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
        });
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.add(
                egui::Image::new(egui::include_image!("../assets/joltux-icon-256.png"))
                    .fit_to_exact_size(egui::vec2(36.0, 36.0)),
            );
            let mut wordmark = egui::text::LayoutJob::default();
            for (text, color) in [("Jolt", egui::Color32::WHITE), ("ux", theme::ACCENT_BRIGHT)] {
                wordmark.append(
                    text,
                    0.0,
                    egui::TextFormat { font_id: egui::FontId::proportional(26.0), color, ..Default::default() },
                );
            }
            ui.add(egui::Label::new(wordmark).selectable(false));
        });
        ui.add_space(18.0);

        let current = if self.page == Page::Game { Page::Store } else { self.page };
        let top = ui.cursor().min;
        let width = ui.available_width();
        let row_rect = |index: usize| {
            egui::Rect::from_min_size(
                egui::pos2(top.x, top.y + index as f32 * (NAV_ROW + NAV_GAP)),
                egui::vec2(width, NAV_ROW),
            )
        };

        // The highlight slides to the current page's row.
        if let Some(index) = NAV.iter().position(|(page, ..)| *page == current) {
            let y = ui
                .ctx()
                .animate_value_with_time(egui::Id::new("nav_highlight"), row_rect(index).top(), 0.2);
            let pill = egui::Rect::from_min_size(egui::pos2(top.x, y), egui::vec2(width, NAV_ROW));
            ui.painter()
                .rect_filled(pill, egui::CornerRadius::same(10), theme::ACCENT.gamma_multiply(0.2));
            let bar = egui::Rect::from_min_size(
                egui::pos2(pill.left(), pill.top() + 10.0),
                egui::vec2(3.0, NAV_ROW - 20.0),
            );
            ui.painter().rect_filled(bar, egui::CornerRadius::same(2), theme::ACCENT_BRIGHT);
        }

        let mut clicked = None;
        for (index, (page, glyph, label)) in NAV.into_iter().enumerate() {
            let rect = row_rect(index);
            let response = ui
                .interact(rect, egui::Id::new(("nav", index)), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            let selected = page == current;
            let hover = ui
                .ctx()
                .animate_bool_with_time(response.id, response.hovered() && !selected, theme::FAST);
            if hover > 0.0 {
                ui.painter().rect_filled(
                    rect,
                    egui::CornerRadius::same(10),
                    theme::SURFACE_HI.gamma_multiply(hover),
                );
            }
            let color = if selected {
                egui::Color32::WHITE
            } else {
                theme::mix(theme::TEXT_DIM, theme::TEXT, hover)
            };
            let icon_color = if selected { theme::ACCENT_BRIGHT } else { color };
            ui.painter().text(
                egui::pos2(rect.left() + 26.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                glyph,
                egui::FontId::proportional(16.0),
                icon_color,
            );
            ui.painter().text(
                egui::pos2(rect.left() + 46.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(15.0),
                color,
            );
            if response.clicked() {
                clicked = Some(page);
            }
        }
        ui.allocate_space(egui::vec2(width, NAV.len() as f32 * (NAV_ROW + NAV_GAP)));
        if let Some(page) = clicked {
            self.navigate_to(page);
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(concat!("v", env!("CARGO_PKG_VERSION")))
                    .small()
                    .color(theme::BORDER),
            );
            ui.horizontal(|ui| {
                let signed_in = self.config.session_cookie.is_some();
                theme::dot(ui, if signed_in { theme::ACCENT_BRIGHT } else { theme::TEXT_DIM });
                ui.label(
                    egui::RichText::new(if signed_in { "Signed in" } else { "Not signed in" })
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
        });
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.set_min_height(40.0);
            // Slides the title over when there is somewhere to go back to.
            let show_back = ui.ctx().animate_bool_with_time(
                egui::Id::new("back_button"),
                !self.page_history.is_empty(),
                theme::FAST,
            );
            if show_back > 0.0 {
                ui.scope(|ui| {
                    ui.set_opacity(show_back);
                    let button = egui::Button::new(icon::BACK)
                        .corner_radius(egui::CornerRadius::same(255))
                        .min_size(egui::vec2(34.0, 34.0));
                    if ui.add(button).on_hover_text("Back").clicked() {
                        self.go_back();
                    }
                });
                ui.add_space(4.0 * show_back);
            }
            ui.label(
                egui::RichText::new(self.page.title())
                    .size(24.0)
                    .color(egui::Color32::WHITE),
            );
        });
    }

    /// The panel along the bottom: what is running, its progress and output,
    /// or the result of the last command.
    fn status_panel(&mut self, ui: &mut egui::Ui) {
        let last_line = self.task_output.lines().rev().find(|line| !line.trim().is_empty());
        let failed = !self.busy && is_failure(&self.status);
        let (headline, detail) = if self.busy {
            (self.status.as_str(), self.task_output.trim())
        } else {
            match self.status.split_once('\n') {
                // Multi-line results (build lists, logs) go in the scrolling area.
                Some(_) => (if failed { "Something went wrong" } else { "Finished" }, self.status.trim()),
                None => (self.status.as_str(), ""),
            }
        };

        let mut dismiss = false;
        ui.horizontal(|ui| {
            if self.busy {
                ui.add(egui::Spinner::new().size(16.0).color(theme::ACCENT_BRIGHT));
            } else {
                theme::dot(ui, if failed { theme::DANGER } else { theme::ACCENT_BRIGHT });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !self.busy {
                    dismiss = ui
                        .add(egui::Button::new(egui::RichText::new(icon::CLOSE).small()).frame(false))
                        .on_hover_text("Dismiss")
                        .clicked();
                }
                if let Some(fraction) = self.busy.then(|| last_line.and_then(progress_fraction)).flatten() {
                    ui.add(
                        egui::ProgressBar::new(fraction)
                            .desired_width(260.0)
                            .desired_height(8.0)
                            .fill(theme::ACCENT),
                    );
                    ui.label(
                        egui::RichText::new(last_line.unwrap_or_default().trim())
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    let color = if failed { theme::DANGER } else { theme::TEXT };
                    ui.add(egui::Label::new(egui::RichText::new(headline).color(color)).truncate());
                });
            });
        });

        if !detail.is_empty() {
            egui::Frame::new()
                .fill(theme::SIDEBAR)
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .auto_shrink([false, true])
                        .stick_to_bottom(self.busy)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new(detail).monospace().color(theme::TEXT_DIM));
                        });
                });
        }
        if dismiss {
            self.status.clear();
        }
    }
}

/// The game's cover art, or a coloured tile with its initial while the
/// image loads, when there is none, or when it can't be fetched.
fn cover_tile(ui: &mut egui::Ui, game: &InstalledGame, tint: egui::Color32) {
    let (tile, _) = ui.allocate_exact_size(egui::vec2(82.0, 46.0), egui::Sense::hover());
    let radius = egui::CornerRadius::same(8);
    let cover = game
        .thumbnail_url
        .as_deref()
        .filter(|url| store::is_gamejolt_media(url))
        .map(|url| store::resize_media_url(url, 200))
        .and_then(|url| match egui::Image::new(url).load_for_size(ui.ctx(), tile.size()) {
            Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture),
            _ => None,
        });
    if let Some(texture) = cover {
        egui::Image::from_texture(texture).corner_radius(radius).paint_at(ui, tile);
        return;
    }
    ui.painter().rect_filled(tile, radius, tint.gamma_multiply(0.22));
    let initial = game
        .title
        .chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "?".into());
    ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(22.0), tint);
}

/// The actions menu for a game, used by its menu button and right-click.
fn game_menu(ui: &mut egui::Ui, game: &InstalledGame, state: &RowState) -> Option<RowAction> {
    ui.set_min_width(190.0);
    let mut action = None;
    let mut item = |ui: &mut egui::Ui, enabled: bool, label: &str, chosen: RowAction| {
        if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
            action = Some(chosen);
        }
    };
    if state.running {
        item(ui, true, if state.stopping { "Force stop" } else { "Stop" }, RowAction::Stop);
    } else {
        item(ui, !state.in_task, "Play", RowAction::Play);
    }
    item(ui, true, "Launch options...", RowAction::ToggleOptions);
    ui.separator();
    item(ui, true, "Open game folder", RowAction::OpenFolder);
    if game.platform == Platform::Windows {
        item(ui, state.prefix_exists, "Open Wine prefix", RowAction::OpenPrefix);
    }
    item(ui, state.log_exists, "Open last launch log", RowAction::OpenLog);
    item(ui, true, "View on Game Jolt", RowAction::ViewOnGameJolt);
    item(ui, true, "Copy game ID", RowAction::CopyId);
    ui.separator();
    let idle = !state.busy && !state.running;
    if state.has_update {
        item(ui, idle, "Update", RowAction::Update);
    }
    item(ui, idle, "Reinstall", RowAction::Reinstall);
    if ui
        .add_enabled(idle, egui::Button::new(egui::RichText::new("Remove").color(theme::DANGER)))
        .clicked()
    {
        action = Some(RowAction::Remove);
    }
    action
}

/// The game's page; Game Jolt redirects to the right slug if ours is stale.
fn game_page_url(game: &InstalledGame) -> String {
    let slug = game
        .slug
        .as_deref()
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .unwrap_or("game");
    format!("https://gamejolt.com/games/{slug}/{}", game.game_id)
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_owned()
}

fn version_text(version: &str) -> String {
    match version.trim().trim_start_matches('v') {
        "" => "version unknown".to_owned(),
        v => format!("v{v}"),
    }
}

/// Whether a finished command's message reports a failure.
fn is_failure(status: &str) -> bool {
    ["Command failed", "Error:", "Login failed", "Could not", "Enter both"]
        .iter()
        .any(|prefix| status.starts_with(prefix))
}

impl eframe::App for JoltuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(message) = self.receiver.try_recv() {
            let TaskMessage::Done(message) = message else {
                if let TaskMessage::Output(output) = message {
                    self.task_output = output;
                }
                continue;
            };
            self.status = message;
            self.task_output.clear();
            self.busy = false;
            self.refresh();
            let viewport = ctx.input(|input| input.viewport().clone());
            let focused = viewport.focused != Some(false) && viewport.minimized != Some(true);
            self.finish_task(focused);
        }
        self.store.poll();
        self.poll_update_check();
        self.poll_cover_backfill();
        self.poll_running();
        if self.update_check.is_some() || self.cover_backfill.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        if !self.running.is_empty() {
            // Notices exits and keeps the play timers ticking.
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        if self.store.is_loading() {
            // Loading placeholders pulse, so keep drawing frames.
            ctx.request_repaint();
        } else if self.busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::SidePanel::left("sidebar")
            .resizable(false)
            .exact_width(204.0)
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::SIDEBAR).inner_margin(egui::Margin::same(14)))
            .show(ctx, |ui| self.sidebar(ui));

        egui::TopBottomPanel::top("top_bar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin {
                left: 24,
                right: 24,
                top: 16,
                bottom: 6,
            }))
            .show(ctx, |ui| self.header(ui));

        // Slides up when there is something to say and away when dismissed.
        egui::TopBottomPanel::bottom("status_bar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin {
                left: 24,
                right: 24,
                top: 0,
                bottom: 16,
            }))
            .show_animated(ctx, self.busy || !self.status.is_empty(), |ui| {
                theme::card()
                    .inner_margin(egui::Margin::symmetric(16, 12))
                    .show(ui, |ui| self.status_panel(ui));
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(egui::Margin {
                left: 24,
                right: 24,
                top: 8,
                bottom: 16,
            }))
            .show(ctx, |ui| {
                // Each page fades in and rises a few pixels into place.
                let now = ctx.input(|input| input.time);
                let since = *self.page_since.get_or_insert(now);
                let progress = theme::ease_out(((now - since) / PAGE_FADE) as f32);
                if progress < 1.0 {
                    ctx.request_repaint();
                    ui.set_opacity(progress);
                    ui.add_space((1.0 - progress) * 10.0);
                }
                match self.page {
                    Page::Store | Page::Game => self.store_page(ui),
                    Page::Library => self.library_page(ui),
                    // Forms scroll on short windows and stay a readable width on wide ones.
                    Page::Install | Page::Account | Page::Settings => {
                        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                            ui.set_max_width(FORM_WIDTH);
                            match self.page {
                                Page::Install => self.install_page(ui),
                                Page::Account => self.account_page(ui),
                                _ => self.settings_page(ui),
                            }
                        });
                    }
                }
            });
    }
}

fn login_task(username: String, password: String) -> String {
    if username.is_empty() || password.is_empty() {
        return "Enter both a username and password.".into();
    }
    let result = (|| -> Result<String> {
        let api = ApiClient::new(None)?;
        let cookie = match api.login(&username, &password)? {
            LoginOutcome::Authenticated(cookie) => cookie,
            LoginOutcome::CaptchaRequired => {
                anyhow::bail!("Game Jolt requires a CAPTCHA. Sign in through your browser, then enter the frontend cookie in Settings.")
            }
            LoginOutcome::ApprovalRequired(token) => api.poll_login_approval(&token)?,
        };
        let paths = Paths::discover()?;
        let mut config = Config::load(&paths)?;
        config.session_cookie = Some(cookie);
        config.save(&paths)?;
        Ok("Logged in to Game Jolt.".into())
    })();
    result.unwrap_or_else(|error| format!("Login failed: {error:#}"))
}

/// Runs a Joltux subcommand in a child process, sending its combined
/// stdout/stderr to the GUI as it arrives. Returns (succeeded, final output).
fn run_streaming(args: Vec<String>, sender: &Sender<TaskMessage>) -> Result<(bool, String)> {
    let exe = std::env::current_exe().context("could not find the Joltux executable")?;
    let mut child = Command::new(exe)
        .args(args)
        .env(runtime::GRAPHICAL_AUTH_ENV, "1")
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not start Joltux command")?;

    let output = Arc::new(Mutex::new(Vec::<u8>::new()));
    let pipes: Vec<Box<dyn Read + Send>> = vec![
        Box::new(child.stdout.take().context("missing stdout pipe")?),
        Box::new(child.stderr.take().context("missing stderr pipe")?),
    ];
    let readers: Vec<_> = pipes
        .into_iter()
        .map(|mut pipe| {
            let output = Arc::clone(&output);
            let sender = sender.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = pipe.read(&mut buf) {
                    if n == 0 {
                        break;
                    }
                    let text = {
                        let mut output = output.lock().unwrap_or_else(|e| e.into_inner());
                        output.extend_from_slice(&buf[..n]);
                        collapse_carriage_returns(&String::from_utf8_lossy(&output))
                    };
                    let _ = sender.send(TaskMessage::Output(text));
                }
            })
        })
        .collect();
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().context("could not wait for Joltux command")?;
    let output = output.lock().unwrap_or_else(|e| e.into_inner());
    let text = collapse_carriage_returns(&String::from_utf8_lossy(&output));
    Ok((status.success(), text.trim().to_owned()))
}

/// Reads the percentage from a download progress line like
/// `  1.0 / 3.0 MiB (33%)`.
fn progress_fraction(line: &str) -> Option<f32> {
    let line = line.trim_end();
    let inner = line.strip_suffix("%)")?;
    let percent: f32 = inner[inner.rfind('(')? + 1..].parse().ok()?;
    Some((percent / 100.0).clamp(0.0, 1.0))
}

/// Applies `\r` the way a terminal does: text after a carriage return
/// overwrites the line, so in-place progress updates collapse to the last one.
fn collapse_carriage_returns(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            let line = line.strip_suffix('\r').unwrap_or(line);
            line.rsplit('\r').next().unwrap_or(line).trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{collapse_carriage_returns, progress_fraction, window_icon};

    #[test]
    fn embedded_icon_decodes() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (256, 256));
    }

    #[test]
    fn reads_download_percentage() {
        assert_eq!(progress_fraction("  1.0 / 4.0 MiB (25%)   "), Some(0.25));
        assert_eq!(progress_fraction("  12.0 MiB"), None);
        assert_eq!(progress_fraction("Extracting..."), None);
    }

    #[test]
    fn keeps_only_the_last_progress_update() {
        let output = "Installing Game\n\r  1.0 / 3.0 MiB (33%)   \r  3.0 / 3.0 MiB (100%)   \nExtracting...\r\nDone.";
        assert_eq!(
            collapse_carriage_returns(output),
            "Installing Game\n  3.0 / 3.0 MiB (100%)\nExtracting...\nDone."
        );
    }
}
