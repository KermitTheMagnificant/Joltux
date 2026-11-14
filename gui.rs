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
use crate::store::{Store, StoreAction};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Store,
    Game,
    Library,
    Install,
    Account,
    Settings,
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
    page_history: Vec<Page>,
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
            page_history: Vec::new(),
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
        }
    }

    fn go_back(&mut self) {
        if let Some(page) = self.page_history.pop() {
            self.page = page;
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
                self.navigate_to(Page::Install);
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
        if self.busy || self.store.is_loading() {
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
                    let current = self.page == page || (page == Page::Store && self.page == Page::Game);
                    if ui.selectable_label(current, label).clicked() {
                        self.navigate_to(page);
                    }
                }
            });

        egui::TopBottomPanel::top("top_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.page_history.is_empty(), egui::Button::new("Back"))
                    .clicked()
                {
                    self.go_back();
                }
                ui.separator();
                ui.heading(match self.page {
                    Page::Store => "Store",
                    Page::Game => "Game preview",
                    Page::Library => "Library",
                    Page::Install => "Add a game",
                    Page::Account => "Account",
                    Page::Settings => "Settings",
                });
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.page {
            Page::Store | Page::Game => self.store_page(ui),
            Page::Library => self.library_page(ui),
            Page::Install => self.install_page(ui),
            Page::Account => self.account_page(ui),
            Page::Settings => self.settings_page(ui),
        });

        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            let last_line = self.task_output.lines().rev().find(|line| !line.trim().is_empty());
            ui.horizontal(|ui| {
                if self.busy {
                    ui.spinner();
                }
                if self.busy {
                    ui.label(&self.status);
                    if let Some(fraction) = last_line.and_then(progress_fraction) {
                        ui.add(
                            egui::ProgressBar::new(fraction)
                                .desired_width(ui.available_width().min(360.0))
                                .text(last_line.unwrap_or_default().trim()),
                        );
                    }
                }
            });
            egui::ScrollArea::vertical()
                .max_height(90.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    if self.busy {
                        if !self.task_output.trim().is_empty() {
                            ui.small(self.task_output.trim());
                        }
                    } else {
                        ui.label(&self.status);
                    }
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

/// Runs a Joltux subcommand in a child process, sending its combined
/// stdout/stderr to the GUI as it arrives. Returns (succeeded, final output).
fn run_streaming(args: Vec<String>, sender: &Sender<TaskMessage>) -> Result<(bool, String)> {
    let exe = std::env::current_exe().context("could not find the Joltux executable")?;
    let mut child = Command::new(exe)
        .args(args)
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
    use super::{collapse_carriage_returns, progress_fraction};

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
