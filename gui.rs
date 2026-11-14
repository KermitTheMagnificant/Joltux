use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::sync::mpsc::{self, Receiver, Sender};

use anyhow::{Context, Result};
use eframe::egui;

use crate::api::{ApiClient, LoginOutcome};
use crate::config::{Config, Runner};
use crate::library::Library;
use crate::models::InstalledGame;
use crate::paths::Paths;
use crate::runtime;
use crate::select::Platform;
use crate::store::{Store, StoreAction};
use crate::theme::{self, icon, AccentButton};

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
    let png = include_bytes!("assets/joltux-icon-256.png");
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
        })
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
            Ok(library) => self.games = library.games,
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
                self.run_cli(vec!["install".into(), game_id.to_string()]);
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

        ui.horizontal(|ui| {
            let count = self.games.len();
            ui.label(
                egui::RichText::new(format!("{count} game{} installed", if count == 1 { "" } else { "s" }))
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add_enabled(!self.busy, egui::Button::new("Refresh")).clicked() {
                    self.refresh();
                }
            });
        });
        ui.add_space(4.0);

        // Leave room under the list for the prefix option.
        let list_height = (ui.available_height() - 40.0).max(80.0);
        egui::ScrollArea::vertical()
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for game in self.games.clone() {
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

        // Hover is read from the previous frame, because the card's background
        // has to be chosen before its contents are laid out.
        let id = ui.id().with(("library_row", game.game_id));
        let was_hovered = ui.ctx().data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
        let hover = ui.ctx().animate_bool_with_time(id, was_hovered, theme::FAST);

        let response = theme::card()
            .fill(theme::mix(theme::SURFACE, theme::SURFACE_HI, hover))
            .stroke(egui::Stroke::new(1.0_f32, theme::mix(theme::BORDER, tint, hover * 0.8)))
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    // A coloured tile with the game's initial stands in for cover art.
                    let (tile, _) = ui.allocate_exact_size(egui::vec2(46.0, 46.0), egui::Sense::hover());
                    ui.painter().rect_filled(tile, egui::CornerRadius::same(10), tint.gamma_multiply(0.22));
                    let initial = game
                        .title
                        .chars()
                        .find(|c| c.is_alphanumeric())
                        .map(|c| c.to_uppercase().to_string())
                        .unwrap_or_else(|| "?".into());
                    ui.painter().text(
                        tile.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        egui::FontId::proportional(22.0),
                        tint,
                    );
                    ui.add_space(4.0);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(!self.busy, AccentButton::new(format!("{}  Play", icon::PLAY)))
                            .clicked()
                        {
                            self.run_cli(vec!["run".into(), game.game_id.to_string()]);
                        }
                        if ui.add_enabled(!self.busy, AccentButton::danger("Remove")).clicked() {
                            let mut args = vec!["uninstall".into(), game.game_id.to_string()];
                            if self.remove_prefix {
                                args.push("--prefix".into());
                            }
                            self.run_cli(args);
                        }
                        // Whatever is left belongs to the title, which truncates.
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.spacing_mut().item_spacing.y = 4.0;
                            ui.spacing_mut().interact_size.y = 18.0;
                            ui.add_space(3.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&game.title).size(16.0).color(egui::Color32::WHITE),
                                )
                                .truncate(),
                            );
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                theme::badge(ui, if linux { "Linux" } else { "Windows · Proton/Wine" }, tint);
                                let version = if game.version.is_empty() {
                                    "version unknown".to_owned()
                                } else {
                                    format!("v{}", game.version.trim_start_matches('v'))
                                };
                                ui.label(
                                    egui::RichText::new(format!("{version}  ·  ID {}", game.game_id))
                                        .small()
                                        .color(theme::TEXT_DIM),
                                );
                            });
                        });
                    });
                });
            })
            .response;
        let hovered = ui.rect_contains_pointer(response.rect);
        ui.ctx().data_mut(|data| data.insert_temp(id, hovered));
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
                    self.run_cli(args);
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
                egui::Image::new(egui::include_image!("assets/joltux-icon-256.png"))
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
        }
        self.store.poll();
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
