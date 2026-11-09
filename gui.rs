use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result};
use eframe::egui;

use crate::api::{ApiClient, LoginOutcome};
use crate::config::{Config, Runner};
use crate::library::Library;
use crate::models::{GameSearchPayload, GameSearchResult, InstalledGame};
use crate::paths::Paths;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Store,
    Library,
    Install,
    Account,
    Settings,
}

enum StoreMessage {
    Search(Result<GameSearchPayload, String>),
}

pub fn run() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([900.0, 650.0])
            .with_min_inner_size([650.0, 480.0]),
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
    games: Vec<InstalledGame>,
    config: Config,
    receiver: Receiver<String>,
    sender: Sender<String>,
    store_receiver: Receiver<StoreMessage>,
    store_sender: Sender<StoreMessage>,
    busy: bool,
    status: String,
    store_query: String,
    store_games: Vec<GameSearchResult>,
    store_page: u32,
    store_per_page: u32,
    store_game_count: u64,
    store_searching: bool,
    store_error: Option<String>,
    game_input: String,
    windows_only: bool,
    remove_prefix: bool,
    login_username: String,
    login_password: String,
    cookie_input: String,
    show_cookie: bool,
}

impl JoltuxApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Result<Self> {
        let paths = Paths::discover()?;
        let (sender, receiver) = mpsc::channel();
        let (store_sender, store_receiver) = mpsc::channel();
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::light();
        style.visuals.panel_fill = egui::Color32::from_rgb(246, 247, 244);
        style.visuals.window_fill = egui::Color32::from_rgb(252, 252, 249);
        style.visuals.selection.bg_fill = egui::Color32::from_rgb(16, 120, 104);
        style.visuals.widgets.active.bg_fill = egui::Color32::from_rgb(16, 120, 104);
        style.visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(218, 237, 230);
        cc.egui_ctx.set_style(style);

        let games = Library::load(&paths)?.games;
        let config = Config::load(&paths)?;
        Ok(Self {
            paths,
            page: Page::Store,
            games,
            config,
            receiver,
            sender,
            store_receiver,
            store_sender,
            busy: false,
            status: String::new(),
            store_query: String::new(),
            store_games: Vec::new(),
            store_page: 1,
            store_per_page: 36,
            store_game_count: 0,
            store_searching: false,
            store_error: None,
            game_input: String::new(),
            windows_only: false,
            remove_prefix: false,
            login_username: String::new(),
            login_password: String::new(),
            cookie_input: String::new(),
            show_cookie: false,
        })
    }

    fn start_task(&mut self, task: impl FnOnce() -> String + Send + 'static) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.status = "Working...".to_owned();
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let _ = sender.send(task());
        });
    }

    fn run_cli(&mut self, args: Vec<String>) {
        self.start_task(move || {
            let result = std::env::current_exe()
                .context("could not find the Joltux executable")
                .and_then(|exe| {
                    Command::new(exe)
                        .args(args)
                        .stdin(Stdio::inherit())
                        .output()
                        .context("could not start Joltux command")
                });
            match result {
                Ok(output) => {
                    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if !stderr.trim().is_empty() {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(stderr.trim());
                    }
                    if output.status.success() {
                        if text.trim().is_empty() {
                            "Done.".to_owned()
                        } else {
                            text
                        }
                    } else if text.trim().is_empty() {
                        format!("Command failed with {}.", output.status)
                    } else {
                        format!("Command failed: {text}")
                    }
                }
                Err(error) => format!("Error: {error:#}"),
            }
        });
    }

    fn login(&mut self) {
        let username = self.login_username.trim().to_owned();
        let password = std::mem::take(&mut self.login_password);
        self.start_task(move || login_task(username, password));
    }

    fn search_store(&mut self, page: u32) {
        let query = self.store_query.trim().to_owned();
        if query.is_empty() || self.store_searching {
            return;
        }

        self.store_searching = true;
        self.store_error = None;
        self.store_page = page.max(1);
        if self.store_page == 1 {
            self.store_games.clear();
            self.store_game_count = 0;
        }
        let page = self.store_page;
        let session_cookie = self.config.session_cookie.clone();
        let sender = self.store_sender.clone();
        std::thread::spawn(move || {
            let result = ApiClient::new(session_cookie.as_deref())
                .and_then(|api| api.search_games(&query, page))
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(StoreMessage::Search(result));
        });
    }

    fn refresh(&mut self) {
        match Library::load(&self.paths) {
            Ok(library) => self.games = library.games,
            Err(error) => self.status = format!("Could not reload library: {error:#}"),
        }
        match Config::load(&self.paths) {
            Ok(config) => self.config = config,
            Err(error) => self.status = format!("Could not reload settings: {error:#}"),
        }
    }

    fn store_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Discover games");
        ui.add_space(8.0);

        let mut search_requested = false;
        ui.horizontal(|ui| {
            let response = ui.add_enabled(
                !self.store_searching,
                egui::TextEdit::singleline(&mut self.store_query)
                    .hint_text("Search Game Jolt games")
                    .desired_width(360.0),
            );
            search_requested =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(
                    !self.store_searching && !self.store_query.trim().is_empty(),
                    egui::Button::new("Search"),
                )
                .clicked()
            {
                search_requested = true;
            }
            if self.store_searching {
                ui.spinner();
            }
        });

        if search_requested {
            self.search_store(1);
        }

        if let Some(error) = &self.store_error {
            ui.colored_label(egui::Color32::DARK_RED, error);
        } else if self.store_games.is_empty() && !self.store_searching {
            ui.add_space(12.0);
            ui.label(if self.store_query.trim().is_empty() {
                "Search for a game by title or keyword."
            } else {
                "No public games found for this search."
            });
        }

        if !self.store_games.is_empty() {
            ui.horizontal(|ui| {
                ui.label(format!("{} results", self.store_game_count));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let pages = self
                        .store_game_count
                        .div_ceil(u64::from(self.store_per_page.max(1)))
                        .max(1) as u32;
                    if ui
                        .add_enabled(
                            !self.store_searching && self.store_page < pages,
                            egui::Button::new("Next"),
                        )
                        .clicked()
                    {
                        search_requested = true;
                        self.store_page = self.store_page.saturating_add(1);
                    }
                    ui.label(format!("{} / {}", self.store_page, pages));
                    if ui
                        .add_enabled(
                            !self.store_searching && self.store_page > 1,
                            egui::Button::new("Previous"),
                        )
                        .clicked()
                    {
                        search_requested = true;
                        self.store_page = self.store_page.saturating_sub(1);
                    }
                });
            });
        }

        let mut review_game = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for game in self
                .store_games
                .iter()
                .filter(|game| !game.has_adult_content)
                .cloned()
            {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if let Some(thumbnail) = game
                                .img_thumbnail
                                .as_deref()
                                .filter(|url| is_gamejolt_thumbnail(url))
                            {
                                ui.add(
                                    egui::Image::from_uri(thumbnail)
                                        .fit_to_exact_size(egui::vec2(144.0, 81.0)),
                                );
                            }
                            ui.vertical(|ui| {
                                ui.strong(&game.title);
                                let developer = game
                                    .developer
                                    .as_ref()
                                    .and_then(|developer| {
                                        developer
                                            .display_name
                                            .as_ref()
                                            .or(developer.username.as_ref())
                                    })
                                    .map(String::as_str)
                                    .unwrap_or("Unknown creator");
                                ui.small(format!("{developer}  ·  Game {}", game.id));
                                if let Some(slug) = game.slug.as_deref() {
                                    ui.hyperlink_to(
                                        "Game Jolt page",
                                        format!("https://gamejolt.com/games/{slug}/{}", game.id),
                                    );
                                }
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Review builds").clicked() {
                                        review_game = Some((game.id, game.title.clone()));
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(4.0);
            }
        });

        if search_requested && !self.store_searching {
            self.search_store(self.store_page);
        }
        if let Some((game_id, title)) = review_game {
            self.game_input = game_id.to_string();
            self.windows_only = false;
            self.page = Page::Install;
            self.status = format!("Reviewing builds for {title}...");
            self.run_cli(vec!["info".into(), game_id.to_string()]);
        }
    }

    fn library_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading("Your library");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Refresh"))
                    .clicked()
                {
                    self.refresh();
                }
            });
        });
        ui.label(format!("{} installed", self.games.len()));
        ui.add_space(12.0);

        if self.games.is_empty() {
            ui.label("No games installed yet.");
            if ui.button("Install a game").clicked() {
                self.page = Page::Install;
            }
            return;
        }

        egui::ScrollArea::vertical().show(ui, |ui| {
            for game in self.games.clone() {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.strong(&game.title);
                                ui.small(format!(
                                    "ID {}  ·  {}  ·  {}",
                                    game.game_id,
                                    if game.platform == crate::select::Platform::Linux {
                                        "Linux"
                                    } else {
                                        "Windows via Proton/Wine"
                                    },
                                    if game.version.is_empty() {
                                        "version unknown"
                                    } else {
                                        &game.version
                                    }
                                ));
                            });
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui
                                        .add_enabled(!self.busy, egui::Button::new("Play"))
                                        .clicked()
                                    {
                                        self.run_cli(vec!["run".into(), game.game_id.to_string()]);
                                    }
                                    if ui
                                        .add_enabled(!self.busy, egui::Button::new("Remove"))
                                        .clicked()
                                    {
                                        let mut args =
                                            vec!["uninstall".into(), game.game_id.to_string()];
                                        if self.remove_prefix {
                                            args.push("--prefix".into());
                                        }
                                        self.run_cli(args);
                                    }
                                },
                            );
                        });
                    });
                ui.add_space(6.0);
            }
        });

        ui.add_space(8.0);
        ui.checkbox(
            &mut self.remove_prefix,
            "Also remove Wine/Proton prefixes when removing games",
        );
    }

    fn install_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Install a game");
        ui.label("Enter a Game Jolt ID or game URL.");
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label("Game");
            ui.add(egui::TextEdit::singleline(&mut self.game_input).desired_width(420.0));
        });
        ui.checkbox(&mut self.windows_only, "Prefer the Windows build");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.busy && !self.game_input.trim().is_empty(),
                    egui::Button::new("Inspect builds"),
                )
                .clicked()
            {
                self.run_cli(vec!["info".into(), self.game_input.trim().to_owned()]);
            }
            if ui
                .add_enabled(
                    !self.busy && !self.game_input.trim().is_empty(),
                    egui::Button::new("Install"),
                )
                .clicked()
            {
                let mut args = vec!["install".into(), self.game_input.trim().to_owned()];
                if self.windows_only {
                    args.push("--windows".into());
                }
                self.run_cli(args);
            }
        });
        ui.add_space(20.0);
        ui.label("The native Linux build is selected by default when one is available.");
    }

    fn account_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Game Jolt account");
        ui.add_space(8.0);
        ui.label(if self.config.session_cookie.is_some() {
            "A Game Jolt session cookie is configured."
        } else {
            "Sign in to connect your Game Jolt account."
        });
        ui.add_space(12.0);

        egui::Grid::new("account_login")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Username");
                ui.add(egui::TextEdit::singleline(&mut self.login_username).desired_width(280.0));
                ui.end_row();

                ui.label("Password");
                ui.add(
                    egui::TextEdit::singleline(&mut self.login_password)
                        .password(true)
                        .desired_width(280.0),
                );
                ui.end_row();
            });

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Log in"))
                .clicked()
            {
                self.login();
            }
            if ui
                .add_enabled(
                    !self.busy && self.config.session_cookie.is_some(),
                    egui::Button::new("Log out"),
                )
                .clicked()
            {
                self.run_cli(vec!["logout".into()]);
            }
        });
        ui.add_space(8.0);
        ui.small("If login requires a CAPTCHA, sign in through your browser and enter the frontend cookie in Settings.");
        ui.add_space(8.0);
        ui.label(self.config.redacted());
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.add_space(8.0);
        egui::Grid::new("runtime_settings")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Windows runner");
                egui::ComboBox::from_id_salt("runner")
                    .selected_text(match self.config.runner {
                        Runner::Umu => "umu / Proton",
                        Runner::Wine => "Wine",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.config.runner, Runner::Umu, "umu / Proton");
                        ui.selectable_value(&mut self.config.runner, Runner::Wine, "Wine");
                    });
                ui.end_row();

                ui.label("umu-run path");
                ui.text_edit_singleline(&mut self.config.umu_path);
                ui.end_row();

                ui.label("Wine path");
                ui.text_edit_singleline(&mut self.config.wine_path);
                ui.end_row();

                ui.label("Proton path");
                ui.text_edit_singleline(&mut self.config.proton_path);
                ui.end_row();

                ui.label("Game Jolt session cookie");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.cookie_input)
                            .password(!self.show_cookie)
                            .desired_width(300.0),
                    );
                    ui.checkbox(&mut self.show_cookie, "Show");
                });
                ui.end_row();
            });

        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.busy, egui::Button::new("Save settings"))
                .clicked()
            {
                self.config.session_cookie = if self.cookie_input.is_empty() {
                    self.config.session_cookie.take()
                } else {
                    Some(std::mem::take(&mut self.cookie_input))
                };
                self.status = match self.config.save(&self.paths) {
                    Ok(()) => {
                        "Settings saved. Session cookie is stored in the owner-only config file."
                            .into()
                    }
                    Err(error) => format!("Could not save settings: {error:#}"),
                };
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Set up Wine + umu"))
                .clicked()
            {
                self.run_cli(vec!["setup".into()]);
            }
            if ui
                .add_enabled(!self.busy, egui::Button::new("Check for updates"))
                .clicked()
            {
                self.run_cli(vec!["update".into()]);
            }
        });
    }
}

impl eframe::App for JoltuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(message) = self.receiver.try_recv() {
            self.status = message;
            self.busy = false;
            self.refresh();
        }
        while let Ok(message) = self.store_receiver.try_recv() {
            match message {
                StoreMessage::Search(Ok(payload)) => {
                    self.store_searching = false;
                    self.store_games = payload.games;
                    self.store_game_count = payload.games_count;
                    self.store_page = payload.page.max(1);
                    self.store_per_page = payload.per_page.max(1);
                    self.store_error = None;
                }
                StoreMessage::Search(Err(error)) => {
                    self.store_searching = false;
                    self.store_games.clear();
                    self.store_game_count = 0;
                    self.store_error = Some(format!("Game search failed: {error}"));
                }
            }
        }
        if self.busy || self.store_searching {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        egui::SidePanel::left("sidebar")
            .resizable(false)
            .exact_width(176.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.heading("Joltux");
                ui.add_space(20.0);
                for (page, label) in [
                    (Page::Store, "Store"),
                    (Page::Library, "Library"),
                    (Page::Install, "Add a game"),
                    (Page::Account, "Account"),
                    (Page::Settings, "Settings"),
                ] {
                    if ui.selectable_label(self.page == page, label).clicked() {
                        self.page = page;
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Store => self.store_page(ui),
            Page::Library => self.library_page(ui),
            Page::Install => self.install_page(ui),
            Page::Account => self.account_page(ui),
            Page::Settings => self.settings_page(ui),
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
            });
            egui::ScrollArea::vertical()
                .max_height(90.0)
                .show(ui, |ui| {
                    ui.label(&self.status);
                });
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

fn is_gamejolt_thumbnail(uri: &str) -> bool {
    reqwest::Url::parse(uri).is_ok_and(|url| {
        url.scheme() == "https" && matches!(url.host_str(), Some("m.gjcdn.net" | "i.gjcdn.net"))
    })
}

#[cfg(test)]
mod tests {
    use super::is_gamejolt_thumbnail;

    #[test]
    fn only_loads_https_gamejolt_cdn_thumbnails() {
        assert!(is_gamejolt_thumbnail(
            "https://m.gjcdn.net/game-thumbnail/500/123.webp"
        ));
        assert!(is_gamejolt_thumbnail(
            "https://i.gjcdn.net/public-data/games/1/2/3/thumbnail.png"
        ));
        assert!(!is_gamejolt_thumbnail(
            "http://m.gjcdn.net/game-thumbnail/500/123.webp"
        ));
        assert!(!is_gamejolt_thumbnail(
            "https://m.gjcdn.net.example.test/thumbnail.webp"
        ));
    }
}
