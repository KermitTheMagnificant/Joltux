//! The GUI's store: browsing Game Jolt by category and sort order, searching,
//! and a preview page with a game's screenshots, videos and description.

use std::sync::mpsc::{self, Receiver, Sender};

use eframe::egui;

use crate::api::ApiClient;
use crate::models::{
    description_blocks, Compatibility, DescriptionBlock, GameDetails, GameMedia, GameSearchPayload,
    GameSearchResult,
};
use crate::theme::{self, icon, AccentButton};

const CARD_MIN_WIDTH: f32 = 220.0;
const CARD_GAP: f32 = 14.0;
/// Height of the text area under a card's thumbnail.
const CARD_TEXT_HEIGHT: f32 = 82.0;
/// How far a card rises when hovered.
const CARD_LIFT: f32 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Hot,
    New,
    Best,
    Featured,
}

impl Section {
    const ALL: [Section; 4] = [Section::Hot, Section::New, Section::Best, Section::Featured];

    fn api_name(self) -> &'static str {
        match self {
            Section::Hot => "hot",
            Section::New => "new",
            Section::Best => "best",
            Section::Featured => "featured",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Section::Hot => "Hot",
            Section::New => "New",
            Section::Best => "Best",
            Section::Featured => "Featured",
        }
    }
}

/// Category chips: (label, Game Jolt tag). Every tag here was checked against
/// the live listing endpoint; an unknown tag silently returns no games.
const CATEGORIES: &[(&str, Option<&str>)] = &[
    ("All", None),
    ("Action", Some("action")),
    ("Adventure", Some("adventure")),
    ("Arcade", Some("arcade")),
    ("Horror", Some("horror")),
    ("Platformer", Some("platformer")),
    ("Puzzle", Some("puzzle")),
    ("RPG", Some("rpg")),
    ("Shooter", Some("shooter")),
    ("Strategy", Some("strategy")),
    ("Survival", Some("survival")),
    ("Roguelike", Some("roguelike")),
    ("Retro", Some("retro")),
    ("Multiplayer", Some("multiplayer")),
    ("Point & click", Some("pointnclick")),
    ("Visual novel", Some("visualnovel")),
    ("Sports", Some("sports")),
    ("Fan games", Some("fangame")),
    ("FNaF", Some("fnaf")),
];

#[derive(Clone, PartialEq, Eq)]
enum Listing {
    Browse,
    Search(String),
}

enum StoreMessage {
    Listing {
        request: u64,
        result: Result<GameSearchPayload, String>,
    },
    Details {
        game_id: u64,
        result: Result<GameDetails, String>,
    },
}

enum DetailState {
    Loading,
    Ready(Box<GameDetails>),
    Failed(String),
}

struct Preview {
    game_id: u64,
    title: String,
    state: DetailState,
    selected_media: usize,
}

/// Something the store wants the app to do.
pub enum StoreAction {
    OpenGame,
    ReviewBuilds { game_id: u64, title: String },
    Install { game_id: u64, title: String },
}

pub struct Store {
    sender: Sender<StoreMessage>,
    receiver: Receiver<StoreMessage>,
    query: String,
    listing: Listing,
    section: Section,
    category: usize,
    games: Vec<GameSearchResult>,
    page: u32,
    per_page: u32,
    game_count: u64,
    /// Incremented per listing request so stale responses can be dropped.
    request: u64,
    loading: bool,
    loaded_once: bool,
    error: Option<String>,
    preview: Option<Preview>,
}

impl Store {
    pub fn new() -> Self {
        let (sender, receiver) = mpsc::channel();
        Self {
            sender,
            receiver,
            query: String::new(),
            listing: Listing::Browse,
            section: Section::Hot,
            category: 0,
            games: Vec::new(),
            page: 1,
            per_page: 36,
            game_count: 0,
            request: 0,
            loading: false,
            loaded_once: false,
            error: None,
            preview: None,
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loading || matches!(self.preview, Some(Preview { state: DetailState::Loading, .. }))
    }

    /// Applies finished background requests. Call once per frame.
    pub fn poll(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            match message {
                StoreMessage::Listing { request, result } => {
                    if request != self.request {
                        continue;
                    }
                    self.loading = false;
                    match result {
                        Ok(payload) => {
                            self.games = payload.games;
                            self.game_count = payload.games_count;
                            self.page = payload.page.max(1);
                            self.per_page = payload.per_page.max(1);
                            self.error = None;
                        }
                        Err(error) => {
                            self.games.clear();
                            self.game_count = 0;
                            self.error = Some(format!("Could not load games: {error}"));
                        }
                    }
                }
                StoreMessage::Details { game_id, result } => {
                    if let Some(preview) = self.preview.as_mut().filter(|p| p.game_id == game_id) {
                        preview.state = match result {
                            Ok(details) => DetailState::Ready(Box::new(details)),
                            Err(error) => DetailState::Failed(error),
                        };
                    }
                }
            }
        }
    }

    fn load(&mut self, page: u32, session: Option<String>) {
        self.request += 1;
        self.loading = true;
        self.loaded_once = true;
        self.error = None;
        self.page = page.max(1);
        let request = self.request;
        let page = self.page;
        let listing = self.listing.clone();
        let section = self.section;
        let tag = CATEGORIES[self.category].1;
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = ApiClient::new(session.as_deref())
                .and_then(|api| match &listing {
                    Listing::Browse => api.browse_games(section.api_name(), tag, page),
                    Listing::Search(query) => api.search_games(query, page),
                })
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(StoreMessage::Listing { request, result });
        });
    }

    pub fn open_game(&mut self, game_id: u64, title: String, session: Option<String>) {
        self.preview = Some(Preview {
            game_id,
            title,
            state: DetailState::Loading,
            selected_media: 0,
        });
        let sender = self.sender.clone();
        std::thread::spawn(move || {
            let result = ApiClient::new(session.as_deref())
                .and_then(|api| api.game_details(game_id))
                .map_err(|error| format!("{error:#}"));
            let _ = sender.send(StoreMessage::Details { game_id, result });
        });
    }

    fn heading_text(&self) -> String {
        match &self.listing {
            Listing::Search(query) => format!("Results for “{query}”"),
            Listing::Browse => {
                let (category, tag) = CATEGORIES[self.category];
                match tag {
                    None => format!("{} games", self.section.label()),
                    Some(_) => format!("{} {category} games", self.section.label()),
                }
            }
        }
    }

    /// The browse/search page.
    pub fn ui(&mut self, ui: &mut egui::Ui, session: Option<&str>) -> Option<StoreAction> {
        let session = session.map(str::to_owned);
        if !self.loaded_once {
            self.load(1, session.clone());
        }
        let mut reload: Option<u32> = None;

        // Search bar and sort order.
        ui.horizontal(|ui| {
            let response = ui.add(
                theme::field(&mut self.query)
                    .hint_text(format!("{}  Search Game Jolt games", icon::SEARCH))
                    .desired_width(320.0),
            );
            let submitted =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if (ui
                .add_enabled(!self.query.trim().is_empty(), AccentButton::new("Search"))
                .clicked()
                || submitted)
                && !self.query.trim().is_empty()
            {
                self.listing = Listing::Search(self.query.trim().to_owned());
                reload = Some(1);
            }
            if matches!(self.listing, Listing::Search(_)) && ui.button("Clear search").clicked() {
                self.query.clear();
                self.listing = Listing::Browse;
                reload = Some(1);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let browsing = self.listing == Listing::Browse;
                let before = self.section;
                ui.add_enabled_ui(browsing, |ui| {
                    let options = Section::ALL.map(|section| (section, section.label()));
                    theme::segmented(ui, "store_sort", &mut self.section, &options);
                })
                .response
                .on_disabled_hover_text("Sorting applies when browsing, not to search results");
                if self.section != before {
                    reload = Some(1);
                }
            });
        });

        // Category chips.
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
            for (index, (label, _)) in CATEGORIES.iter().enumerate() {
                let selected = self.listing == Listing::Browse && self.category == index;
                if theme::chip(ui, selected, label).clicked() && !selected {
                    self.category = index;
                    if matches!(self.listing, Listing::Search(_)) {
                        self.query.clear();
                        self.listing = Listing::Browse;
                    }
                    reload = Some(1);
                }
            }
        });
        ui.add_space(8.0);

        // Heading, result count and pagination.
        let pages = self
            .game_count
            .div_ceil(u64::from(self.per_page.max(1)))
            .clamp(1, u64::from(u32::MAX)) as u32;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(self.heading_text())
                    .size(18.0)
                    .color(egui::Color32::WHITE),
            );
            if self.game_count > 0 {
                ui.label(
                    egui::RichText::new(format!("{} games", thousands(self.game_count)))
                        .color(theme::TEXT_DIM),
                );
            }
            if self.loading {
                ui.add(egui::Spinner::new().size(14.0).color(theme::ACCENT_BRIGHT));
            }
            if !self.games.is_empty() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled(!self.loading && self.page < pages, egui::Button::new("Next ›"))
                        .clicked()
                    {
                        reload = Some(self.page + 1);
                    }
                    ui.label(
                        egui::RichText::new(format!("Page {} of {}", self.page, thousands(u64::from(pages))))
                            .color(theme::TEXT_DIM),
                    );
                    if ui
                        .add_enabled(!self.loading && self.page > 1, egui::Button::new("‹ Previous"))
                        .clicked()
                    {
                        reload = Some(self.page - 1);
                    }
                });
            }
        });
        ui.add_space(6.0);

        if let Some(error) = &self.error {
            ui.colored_label(theme::DANGER, error);
            if ui.button("Try again").clicked() {
                reload = Some(self.page);
            }
        } else if self.games.is_empty() && !self.loading {
            ui.add_space(12.0);
            ui.label(
                egui::RichText::new(match self.listing {
                    Listing::Search(_) => "No public games found for this search.",
                    Listing::Browse => "No games in this category.",
                })
                .color(theme::TEXT_DIM),
            );
        }

        let mut opened = None;
        egui::ScrollArea::vertical()
            .id_salt(("store_grid", self.request))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // Room for the top row of cards to rise on hover without clipping.
                ui.add_space(CARD_LIFT);
                if self.loading && self.games.is_empty() {
                    skeleton_grid(ui);
                    return;
                }
                // The old page dims while the next one loads.
                let dim = ui
                    .ctx()
                    .animate_bool_with_time(egui::Id::new("store_grid_dim"), self.loading, 0.2);
                ui.multiply_opacity(1.0 - 0.6 * dim);
                opened = game_grid(ui, &self.games).map(|game| (game.id, game.title.clone()));
            });

        if let Some(page) = reload {
            self.load(page, session.clone());
        }
        let (game_id, title) = opened?;
        self.open_game(game_id, title, session);
        Some(StoreAction::OpenGame)
    }

    /// The preview page for the game opened with `open_game`.
    pub fn preview_ui(&mut self, ui: &mut egui::Ui, session: Option<&str>, busy: bool) -> Option<StoreAction> {
        let Some(preview) = self.preview.as_mut() else {
            ui.label("No game selected.");
            return None;
        };
        let game_id = preview.game_id;
        let title = preview.title.clone();
        let mut action = None;
        let mut retry = false;

        match &preview.state {
            DetailState::Loading => {
                ui.heading(egui::RichText::new(&title).color(egui::Color32::WHITE));
                ui.horizontal(|ui| {
                    ui.add(egui::Spinner::new().size(16.0).color(theme::ACCENT_BRIGHT));
                    ui.label(egui::RichText::new("Loading game details...").color(theme::TEXT_DIM));
                });
            }
            DetailState::Failed(error) => {
                ui.heading(egui::RichText::new(&title).color(egui::Color32::WHITE));
                ui.colored_label(theme::DANGER, format!("Could not load this game: {error}"));
                ui.horizontal(|ui| {
                    retry = ui.button("Try again").clicked();
                    action = install_buttons(ui, game_id, &title, busy);
                });
            }
            DetailState::Ready(details) => {
                let details = details.clone();
                egui::ScrollArea::vertical()
                    .id_salt(("store_preview", game_id))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        action = details_body(ui, &details, &mut preview.selected_media, busy);
                    });
            }
        }

        if retry {
            self.open_game(game_id, title, session.map(str::to_owned));
        }
        action
    }
}

/// How many cards fit across `available` pixels, and how wide each one is.
fn grid_columns(available: f32) -> (usize, f32) {
    let columns = (((available + CARD_GAP) / (CARD_MIN_WIDTH + CARD_GAP)).floor() as usize).max(1);
    let card_width = (available - CARD_GAP * (columns as f32 - 1.0)) / columns as f32;
    (columns, card_width)
}

/// Lays games out as a responsive grid of cards; returns the clicked game.
fn game_grid<'a>(ui: &mut egui::Ui, games: &'a [GameSearchResult]) -> Option<&'a GameSearchResult> {
    let visible: Vec<&GameSearchResult> = games.iter().filter(|game| !game.has_adult_content).collect();
    let (columns, card_width) = grid_columns(ui.available_width());
    let thumb_height = card_width * 9.0 / 16.0;
    let card_size = egui::vec2(card_width, thumb_height + CARD_TEXT_HEIGHT);

    let mut clicked = None;
    for row in visible.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for game in row {
                if game_card(ui, game, card_size, thumb_height) {
                    clicked = Some(*game);
                }
            }
        });
        ui.add_space(CARD_GAP - ui.spacing().item_spacing.y);
    }
    clicked
}

/// Softly pulsing card outlines shown while the first page of games loads.
fn skeleton_grid(ui: &mut egui::Ui) {
    let (columns, card_width) = grid_columns(ui.available_width());
    let thumb_height = card_width * 9.0 / 16.0;
    let card_size = egui::vec2(card_width, thumb_height + CARD_TEXT_HEIGHT);
    let time = ui.input(|input| input.time) as f32;
    let radius = egui::CornerRadius::same(12);

    for row in 0..3 {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = CARD_GAP;
            for column in 0..columns {
                // Each card is slightly out of step, so the pulse sweeps across.
                let phase = time * 3.0 - (row + column) as f32 * 0.45;
                let pulse = 0.5 + 0.5 * phase.sin();
                let (rect, _) = ui.allocate_exact_size(card_size, egui::Sense::hover());
                let painter = ui.painter();
                painter.rect_filled(rect, radius, theme::SURFACE);
                let thumb = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), thumb_height));
                painter.rect_filled(
                    thumb,
                    egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 },
                    theme::mix(theme::SURFACE, theme::SURFACE_HI, pulse),
                );
                let bar = |top: f32, width: f32| {
                    egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 12.0, thumb.bottom() + top),
                        egui::vec2(rect.width() * width, 10.0),
                    )
                };
                let bar_color = theme::mix(theme::SURFACE_HI, theme::BORDER, pulse);
                painter.rect_filled(bar(16.0, 0.6), egui::CornerRadius::same(5), bar_color);
                painter.rect_filled(bar(36.0, 0.35), egui::CornerRadius::same(5), bar_color);
            }
        });
        ui.add_space(CARD_GAP - ui.spacing().item_spacing.y);
    }
}

fn game_card(ui: &mut egui::Ui, game: &GameSearchResult, size: egui::Vec2, thumb_height: f32) -> bool {
    let (slot, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if !ui.is_rect_visible(slot.expand(CARD_LIFT)) {
        return response.clicked();
    }

    // On hover the card rises, gains a shadow and its border turns teal.
    let hover = theme::ease_out(ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.16));
    let rect = slot.translate(egui::vec2(0.0, -CARD_LIFT * hover));
    let radius = egui::CornerRadius::same(12);
    let top_radius = egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 };
    if hover > 0.0 {
        let shadow = egui::epaint::Shadow {
            offset: [0, 6],
            blur: 18,
            spread: 0,
            color: egui::Color32::from_black_alpha((110.0 * hover) as u8),
        };
        ui.painter().add(shadow.as_shape(rect, radius));
    }
    ui.painter()
        .rect_filled(rect, radius, theme::mix(theme::SURFACE, theme::SURFACE_HI, hover));

    let thumb_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), thumb_height));
    ui.painter().rect_filled(thumb_rect, top_radius, theme::SIDEBAR);
    if let Some(url) = game.img_thumbnail.as_deref().filter(|url| is_gamejolt_media(url)) {
        egui::Image::from_uri(url).corner_radius(top_radius).paint_at(ui, thumb_rect);
    }

    let text_rect = egui::Rect::from_min_max(
        egui::pos2(rect.min.x + 12.0, thumb_rect.max.y + 10.0),
        rect.max - egui::vec2(12.0, 8.0),
    );
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(text_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.spacing_mut().item_spacing.y = 3.0;
    // Keeps the badge row as short as the badges instead of button height.
    child.spacing_mut().interact_size.y = 18.0;
    child.add(
        egui::Label::new(egui::RichText::new(&game.title).color(egui::Color32::WHITE))
            .truncate()
            .selectable(false),
    );
    let developer = game
        .developer
        .as_ref()
        .and_then(|developer| developer.name())
        .unwrap_or("Unknown creator");
    child.add(
        egui::Label::new(egui::RichText::new(developer).small().color(theme::TEXT_DIM))
            .truncate()
            .selectable(false),
    );
    child.add_space(2.0);
    child.horizontal(|ui| {
        platform_badges(ui, game.compatibility.as_ref());
        if let Some(followers) = game.follower_count.filter(|count| *count > 0) {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(format!("{} {}", icon::HEART, compact(followers)))
                        .small()
                        .color(theme::TEXT_DIM),
                );
            });
        }
    });

    ui.painter().rect_stroke(
        rect,
        radius,
        egui::Stroke::new(1.0_f32 + hover * 0.5, theme::mix(theme::BORDER, theme::ACCENT, hover)),
        egui::StrokeKind::Inside,
    );
    response.clicked()
}

fn platform_badges(ui: &mut egui::Ui, compatibility: Option<&Compatibility>) {
    let Some(compat) = compatibility else {
        return;
    };
    ui.spacing_mut().item_spacing.x = 4.0;
    if compat.linux() {
        theme::badge(ui, "Linux", theme::ACCENT_BRIGHT);
    }
    if compat.windows() {
        theme::badge(ui, "Windows", theme::BLUE);
    }
    if compat.mac() {
        theme::badge(ui, "Mac", theme::TEXT_DIM);
    }
    if compat.type_html {
        theme::badge(ui, "Web", theme::AMBER);
    }
}

fn install_buttons(ui: &mut egui::Ui, game_id: u64, title: &str, busy: bool) -> Option<StoreAction> {
    let mut action = None;
    if ui.add_enabled(!busy, AccentButton::new("Install")).clicked() {
        action = Some(StoreAction::Install { game_id, title: title.to_owned() });
    }
    if ui.add_enabled(!busy, egui::Button::new("Review builds")).clicked() {
        action = Some(StoreAction::ReviewBuilds { game_id, title: title.to_owned() });
    }
    action
}

fn details_body(
    ui: &mut egui::Ui,
    details: &GameDetails,
    selected_media: &mut usize,
    busy: bool,
) -> Option<StoreAction> {
    let game = &details.game;
    let width = ui.available_width();

    if let Some(header) = game
        .header_media_item
        .as_ref()
        .and_then(|item| item.mediaserver_url.as_deref())
        .filter(|url| is_gamejolt_media(url))
    {
        ui.add(
            egui::Image::from_uri(header)
                .max_width(width)
                .max_height(220.0)
                .corner_radius(egui::CornerRadius::same(12)),
        );
        ui.add_space(10.0);
    }

    ui.label(egui::RichText::new(&game.title).size(28.0).color(egui::Color32::WHITE));
    ui.horizontal_wrapped(|ui| {
        let developer = game.developer.as_ref().and_then(|d| d.name()).unwrap_or("Unknown creator");
        ui.label(format!("by {developer}"));
        if let Some(followers) = game.follower_count {
            ui.weak(format!("· {} followers", thousands(followers)));
        }
        if let Some(likes) = game.like_count.filter(|likes| *likes > 0) {
            ui.weak(format!("· {} likes", thousands(likes)));
        }
        if let Some(status) = development_status(game.canceled, game.development_status) {
            ui.weak(format!("· {status}"));
        }
    });
    ui.horizontal(|ui| platform_badges(ui, game.compatibility.as_ref()));
    if game.compatibility.as_ref().is_some_and(|c| !c.linux() && !c.windows()) {
        ui.label(
            egui::RichText::new("This game has no Linux or Windows builds, so Joltux may not be able to install it.")
                .small()
                .color(theme::AMBER),
        );
    }
    ui.add_space(8.0);

    let mut action = None;
    ui.horizontal(|ui| {
        action = install_buttons(ui, game.id, &game.title, busy);
        let path = game.slug.as_deref().unwrap_or("game");
        ui.hyperlink_to("Open on Game Jolt", format!("https://gamejolt.com/games/{path}/{}", game.id));
    });
    ui.add_space(12.0);

    let media: Vec<&GameMedia> = details
        .media
        .iter()
        .filter(|item| media_preview_url(item, 400).is_some())
        .collect();
    if !media.is_empty() {
        *selected_media = (*selected_media).min(media.len() - 1);
        media_viewer(ui, &media, selected_media, width);
        ui.add_space(14.0);
    }

    ui.label(egui::RichText::new("About this game").size(18.0).color(egui::Color32::WHITE));
    ui.add_space(4.0);
    let blocks = description_blocks(&game.description_content);
    if blocks.is_empty() {
        ui.label(if details.meta_description.is_empty() {
            "This game has no description."
        } else {
            &details.meta_description
        });
    }
    for block in blocks {
        match block {
            DescriptionBlock::Heading(text) => {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(text).size(16.0).color(egui::Color32::WHITE));
            }
            DescriptionBlock::Paragraph(text) => {
                ui.label(text);
            }
            DescriptionBlock::ListItem(text) => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("•");
                    ui.label(text);
                });
            }
        }
        ui.add_space(4.0);
    }
    action
}

/// A large view of the selected screenshot or video with a thumbnail strip.
fn media_viewer(ui: &mut egui::Ui, media: &[&GameMedia], selected: &mut usize, width: f32) {
    let item = media[*selected];
    let view_width = width.min(900.0);
    let view_size = egui::vec2(view_width, view_width * 9.0 / 16.0);
    let (rect, _) = ui.allocate_exact_size(view_size, egui::Sense::hover());
    ui.painter().rect_filled(rect, egui::CornerRadius::same(12), theme::SIDEBAR);
    if let Some(url) = media_preview_url(item, 900) {
        let image = egui::Image::from_uri(url).maintain_aspect_ratio(true).fit_to_exact_size(view_size);
        // Centre the image inside the dark frame.
        let size = image
            .load_and_calc_size(ui, view_size)
            .unwrap_or(view_size);
        image.paint_at(ui, egui::Rect::from_center_size(rect.center(), size));
    }
    if let Some(link) = item.video_link() {
        let label = if item.title.is_empty() { "Watch video" } else { item.title.as_str() };
        ui.hyperlink_to(format!("{} {label}", icon::PLAY), link);
    } else if !item.caption.is_empty() {
        ui.small(&item.caption);
    }

    if media.len() > 1 {
        ui.add_space(6.0);
        egui::ScrollArea::horizontal()
            .id_salt("media_strip")
            .max_width(view_width)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, item) in media.iter().enumerate() {
                        let size = egui::vec2(128.0, 72.0);
                        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
                        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                        ui.painter().rect_filled(rect, egui::CornerRadius::same(6), theme::SIDEBAR);
                        if let Some(url) = media_preview_url(item, 400) {
                            egui::Image::from_uri(url)
                                .corner_radius(egui::CornerRadius::same(6))
                                .paint_at(ui, rect);
                        }
                        if item.media_type == "video" {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                icon::PLAY,
                                egui::FontId::proportional(22.0),
                                egui::Color32::WHITE,
                            );
                        }
                        if index == *selected || response.hovered() {
                            ui.painter().rect_stroke(
                                rect,
                                egui::CornerRadius::same(6),
                                egui::Stroke::new(if index == *selected { 2.5_f32 } else { 1.5 }, theme::ACCENT_BRIGHT),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if response.clicked() {
                            *selected = index;
                        }
                    }
                });
            });
    }
}

/// A Game Jolt CDN image for a screenshot or video, resized to `width` where
/// the URL allows it.
fn media_preview_url(item: &GameMedia, width: u32) -> Option<String> {
    let url = item
        .media_item
        .as_ref()
        .and_then(|media| media.mediaserver_url.as_deref())
        .or(item.img_thumbnail.as_deref())
        .filter(|url| is_gamejolt_media(url))?;
    Some(resize_media_url(url, width))
}

/// `https://m.gjcdn.net/game-screenshot/400/x.webp` -> `.../<width>/x.webp`.
pub fn resize_media_url(url: &str, width: u32) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_owned();
    };
    if parsed.host_str() != Some("m.gjcdn.net") {
        return url.to_owned();
    }
    let segments: Vec<String> = match parsed.path_segments() {
        Some(segments) => segments.map(str::to_owned).collect(),
        None => return url.to_owned(),
    };
    if segments.len() != 3 || !segments[1].bytes().all(|b| b.is_ascii_digit()) {
        return url.to_owned();
    }
    parsed.set_path(&format!("/{}/{width}/{}", segments[0], segments[2]));
    parsed.to_string()
}

fn development_status(canceled: bool, status: Option<u64>) -> Option<&'static str> {
    if canceled {
        return Some("Canceled");
    }
    match status {
        Some(2) => Some("In development"),
        Some(3) => Some("Canceled"),
        Some(4) => Some("Devlog only"),
        _ => None,
    }
}

pub fn is_gamejolt_media(uri: &str) -> bool {
    reqwest::Url::parse(uri).is_ok_and(|url| {
        url.scheme() == "https" && matches!(url.host_str(), Some("m.gjcdn.net" | "i.gjcdn.net"))
    })
}

fn thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn compact(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => format!("{:.1}k", value as f64 / 1_000.0).replace(".0k", "k"),
        _ => format!("{:.1}M", value as f64 / 1_000_000.0).replace(".0M", "M"),
    }
}

#[cfg(test)]
mod tests {
    use super::{compact, is_gamejolt_media, resize_media_url, thousands};

    #[test]
    fn only_loads_https_gamejolt_cdn_media() {
        assert!(is_gamejolt_media("https://m.gjcdn.net/game-thumbnail/500/123.webp"));
        assert!(is_gamejolt_media("https://i.gjcdn.net/public-data/games/1/2/3/thumbnail.png"));
        assert!(!is_gamejolt_media("http://m.gjcdn.net/game-thumbnail/500/123.webp"));
        assert!(!is_gamejolt_media("https://m.gjcdn.net.example.test/thumbnail.webp"));
    }

    #[test]
    fn resizes_media_server_urls() {
        assert_eq!(
            resize_media_url("https://m.gjcdn.net/game-screenshot/400/5-abc-v4.webp", 900),
            "https://m.gjcdn.net/game-screenshot/900/5-abc-v4.webp"
        );
        let original = "https://i.gjcdn.net/public-data/games/1/shot.png";
        assert_eq!(resize_media_url(original, 900), original);
    }

    #[test]
    fn formats_counts() {
        assert_eq!(thousands(1_556), "1,556");
        assert_eq!(thousands(999), "999");
        assert_eq!(compact(1_688), "1.7k");
        assert_eq!(compact(2_000), "2k");
    }
}
