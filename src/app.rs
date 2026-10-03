//! The window: input row, the session's Results list and the persistent History tab.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, FontFamily, FontId, Key, Layout, Margin, Modifiers, RichText,
    Sense, TextStyle, Theme, Vec2,
};

use crate::history::{History, HistoryEntry};
use crate::profile::{FetchError, OnlineState, Profile};
use crate::steamid::{self, Format, SteamId, Target};
use crate::worker::{Job, Pool, Resolved};

const WORKER_THREADS: usize = 4;
const COPIED_FEEDBACK_SECS: f64 = 1.5;
const AVATAR_SIZE: f32 = 64.0;
const HISTORY_AVATAR_SIZE: f32 = 40.0;
const HISTORY_ROW_HEIGHT: f32 = 48.0;
const AKA_INLINE: usize = 3;

const STEAM_BLUE_DARK: Color32 = Color32::from_rgb(0x66, 0xc0, 0xf4);
const STEAM_BLUE_LIGHT: Color32 = Color32::from_rgb(0x1a, 0x6f, 0xb0);
const GREEN: Color32 = Color32::from_rgb(0x7c, 0xa8, 0x2a);
const RED: Color32 = Color32::from_rgb(0xd9, 0x4a, 0x4a);
const ORANGE: Color32 = Color32::from_rgb(0xe0, 0x84, 0x2a);

/// Fallback fonts for names in scripts egui's bundled fonts lack. The first readable file of
/// each group is loaded.
const FALLBACK_FONTS: &[&[&str]] = &[
    // CJK
    &[
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJKsc-Regular.otf",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\YuGothM.ttc",
    ],
    // Hangul on Windows (Noto CJK already covers it on Linux)
    &["C:\\Windows\\Fonts\\malgun.ttf"],
    // Symbols and other scripts
    &[
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "C:\\Windows\\Fonts\\seguisym.ttf",
    ],
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Results,
    History,
}

enum CardState {
    Loading,
    Done(Box<Resolved>),
    Failed(FetchError),
}

struct Lookup {
    format: Format,
    target: Target,
    state: CardState,
}

enum CardKind {
    Invalid(String),
    Lookup(Lookup),
}

struct Card {
    uid: u64,
    input: String,
    kind: CardKind,
}

impl Card {
    fn lookup(&self) -> Option<&Lookup> {
        match &self.kind {
            CardKind::Lookup(lookup) => Some(lookup),
            CardKind::Invalid(_) => None,
        }
    }

    fn resolved(&self) -> Option<&Resolved> {
        match &self.lookup()?.state {
            CardState::Done(resolved) => Some(resolved),
            _ => None,
        }
    }

    fn steam_id(&self) -> Option<SteamId> {
        let lookup = self.lookup()?;
        match (&lookup.state, &lookup.target) {
            (CardState::Done(resolved), _) => Some(resolved.profile.id),
            (_, Target::Id(id)) => Some(*id),
            _ => None,
        }
    }

    /// Whether this card already shows the profile `target` refers to.
    fn shows(&self, target: &Target) -> bool {
        let Some(lookup) = self.lookup() else {
            return false;
        };
        match target {
            Target::Id(id) => self.steam_id() == Some(*id),
            Target::Vanity(name) => {
                matches!(&lookup.target, Target::Vanity(v) if v.eq_ignore_ascii_case(name))
                    || self
                        .resolved()
                        .and_then(|r| r.profile.custom_url.as_deref())
                        .is_some_and(|c| c.eq_ignore_ascii_case(name))
            }
        }
    }

    fn link(&self) -> Option<String> {
        let lookup = self.lookup()?;
        Some(match self.steam_id() {
            Some(id) => id.profile_url(),
            None => lookup.target.offline_url(),
        })
    }
}

enum Action {
    Copy { text: String, key: String },
    Remove(u64),
    ClearSession,
    Retry(u64),
    LookUp(String),
    DeleteHistory(u64),
    ClearHistory,
}

/// A previous name, from Steam's list or noticed by the app between lookups.
struct PreviousName {
    name: String,
    when: String,
}

pub struct App {
    pool: Pool,
    history: History,
    input: String,
    cards: Vec<Card>,
    next_uid: u64,
    tab: Tab,
    history_entries: Vec<HistoryEntry>,
    history_stale: bool,
    history_filter: String,
    confirm_clear: bool,
    copied: Option<(String, f64)>,
    focus_input: bool,
    notice: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, initial_query: &str) -> Self {
        let ctx = &cc.egui_ctx;

        let history = History::new(
            History::default_dir().unwrap_or_else(|| std::env::temp_dir().join("steamidfinder")),
        );
        Self::with_history(ctx, history, WORKER_THREADS, initial_query)
    }

    fn with_history(
        ctx: &egui::Context,
        history: History,
        threads: usize,
        initial_query: &str,
    ) -> Self {
        egui_extras::install_image_loaders(ctx);
        install_fallback_fonts(ctx);
        apply_style(ctx);
        let mut app = Self {
            pool: Pool::new(ctx.clone(), history.clone(), threads),
            history,
            input: String::new(),
            cards: Vec::new(),
            next_uid: 0,
            tab: Tab::Results,
            history_entries: Vec::new(),
            history_stale: true,
            history_filter: String::new(),
            confirm_clear: false,
            copied: None,
            focus_input: true,
            notice: None,
        };
        app.submit(ctx, initial_query, false);
        app
    }

    /// Parses `text` into lookups and puts them at the top of the Results list, in input order.
    fn submit(&mut self, ctx: &egui::Context, text: &str, open_first: bool) {
        let mut batch: Vec<Card> = Vec::new();
        for token in steamid::split_inputs(text) {
            self.next_uid += 1;
            let uid = self.next_uid;
            let kind = match steamid::parse(token) {
                Ok(parsed) => {
                    if batch.iter().any(|c| c.shows(&parsed.target)) {
                        continue;
                    }
                    self.cards.retain(|c| !c.shows(&parsed.target));
                    self.pool.submit(Job {
                        card: uid,
                        target: parsed.target.clone(),
                    });
                    CardKind::Lookup(Lookup {
                        format: parsed.format,
                        target: parsed.target,
                        state: CardState::Loading,
                    })
                }
                Err(message) => CardKind::Invalid(message),
            };
            batch.push(Card {
                uid,
                input: token.to_string(),
                kind,
            });
        }
        if batch.is_empty() {
            return;
        }
        if open_first && let Some(url) = batch.iter().find_map(Card::link) {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        self.tab = Tab::Results;
        self.cards.splice(0..0, batch);
    }

    fn receive_outcomes(&mut self) {
        while let Some(outcome) = self.pool.try_recv() {
            self.history_stale = true;
            let uid = outcome.card;
            if let Ok(resolved) = &outcome.result {
                // A custom URL may resolve to a profile that's already listed.
                let id = resolved.profile.id;
                self.cards
                    .retain(|c| c.uid == uid || c.steam_id() != Some(id));
            }
            let Some(card) = self.cards.iter_mut().find(|c| c.uid == uid) else {
                continue;
            };
            if let CardKind::Lookup(lookup) = &mut card.kind {
                lookup.state = match outcome.result {
                    Ok(resolved) => CardState::Done(Box::new(resolved)),
                    Err(err) => CardState::Failed(err),
                };
            }
        }
    }

    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>, now: f64) {
        for action in actions {
            match action {
                Action::Copy { text, key } => {
                    ctx.copy_text(text);
                    self.copied = Some((key, now));
                    ctx.request_repaint_after(Duration::from_secs_f64(COPIED_FEEDBACK_SECS + 0.05));
                }
                Action::Remove(uid) => self.cards.retain(|c| c.uid != uid),
                Action::ClearSession => self.cards.clear(),
                Action::Retry(uid) => {
                    let card = self.cards.iter_mut().find(|c| c.uid == uid);
                    if let Some(CardKind::Lookup(lookup)) = card.map(|c| &mut c.kind) {
                        lookup.state = CardState::Loading;
                        self.pool.submit(Job {
                            card: uid,
                            target: lookup.target.clone(),
                        });
                    }
                }
                Action::LookUp(query) => {
                    self.submit(ctx, &query, false);
                    self.focus_input = true;
                }
                Action::DeleteHistory(id64) => {
                    self.report(self.history.delete(id64));
                    self.history_stale = true;
                }
                Action::ClearHistory => {
                    self.report(self.history.clear());
                    self.history_stale = true;
                    self.confirm_clear = false;
                }
            }
        }
    }

    fn report(&mut self, result: std::io::Result<()>) {
        if let Err(err) = result {
            self.notice = Some(format!("Couldn't update the history file: {err}"));
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let button_width = 92.0;
            let edit_width = ui.available_width() - button_width - ui.spacing().item_spacing.x;
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.input)
                    .hint_text("Steam IDs, profile URLs or custom names (paste several at once)")
                    .desired_width(edit_width)
                    .margin(Margin::symmetric(8, 6)),
            );
            if self.focus_input {
                response.request_focus();
                self.focus_input = false;
            }
            let ctrl_enter = response.has_focus()
                && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            let clicked = ui
                .add_sized(
                    [button_width, response.rect.height()],
                    egui::Button::new("Look up"),
                )
                .clicked();
            if enter || clicked || ctrl_enter {
                let text = std::mem::take(&mut self.input);
                self.submit(ctx, &text, ctrl_enter);
                self.focus_input = true;
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let results = format!("Results ({})", self.cards.len());
            ui.selectable_value(&mut self.tab, Tab::Results, results);
            ui.selectable_value(&mut self.tab, Tab::History, "History");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new("Enter look up · Ctrl+Enter look up & open · Esc quit")
                        .small()
                        .weak(),
                );
            });
        });
        if let Some(notice) = self.notice.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(RED, notice);
                if ui.small_button("🗙").clicked() {
                    self.notice = None;
                }
            });
        }
        ui.add_space(4.0);
    }

    fn results_ui(&self, ui: &mut egui::Ui, now: f64, actions: &mut Vec<Action>) {
        if self.cards.is_empty() {
            empty_state(ui);
            return;
        }
        ui.horizontal(|ui| {
            let links: Vec<String> = self.cards.iter().filter_map(Card::link).collect();
            if !links.is_empty()
                && copy_button(ui, "Copy all links", "all-links", &self.copied, now)
            {
                actions.push(Action::Copy {
                    text: links.join("\n"),
                    key: "all-links".into(),
                });
            }
            if ui
                .button("Clear")
                .on_hover_text("Clears this list. History is kept.")
                .clicked()
            {
                actions.push(Action::ClearSession);
            }
            let loading = self
                .cards
                .iter()
                .filter(|c| {
                    matches!(
                        c.lookup(),
                        Some(Lookup {
                            state: CardState::Loading,
                            ..
                        })
                    )
                })
                .count();
            if loading > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("{loading} loading")).weak());
                    ui.spinner();
                });
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for card in &self.cards {
                    ui.push_id(card.uid, |ui| card_ui(ui, card, &self.copied, now, actions));
                    ui.add_space(8.0);
                }
            });
    }

    fn history_ui(&mut self, ui: &mut egui::Ui, now: f64, actions: &mut Vec<Action>) {
        if self.history_stale {
            self.history_entries = self.history.load();
            self.history_stale = false;
        }
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let count = self.history_entries.len();
                if self.confirm_clear {
                    if ui.button("Cancel").clicked() {
                        self.confirm_clear = false;
                    }
                    if ui.button(RichText::new("Yes, clear").color(RED)).clicked() {
                        actions.push(Action::ClearHistory);
                    }
                    ui.label(format!("Clear all {count} entries?"));
                } else if count > 0 && ui.button("Clear all").clicked() {
                    self.confirm_clear = true;
                }
                ui.add(
                    egui::TextEdit::singleline(&mut self.history_filter)
                        .hint_text("Filter by name, previous name or any ID")
                        .desired_width(ui.available_width()),
                );
            });
        });
        ui.add_space(4.0);

        let filter = self.history_filter.trim().to_lowercase();
        let entries: Vec<&HistoryEntry> = self
            .history_entries
            .iter()
            .filter(|e| filter.is_empty() || history_matches(e, &filter))
            .collect();
        if entries.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                let message = if self.history_entries.is_empty() {
                    "No lookups yet. Every profile you look up is saved here."
                } else {
                    "Nothing in your history matches that filter."
                };
                ui.label(RichText::new(message).weak());
            });
            return;
        }
        let avatar_dir = self.history.avatar_dir();
        let now_unix = chrono::Utc::now().timestamp();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show_rows(ui, HISTORY_ROW_HEIGHT, entries.len(), |ui, rows| {
                for entry in &entries[rows] {
                    ui.push_id(entry.id64, |ui| {
                        history_row(ui, entry, &avatar_dir, &self.copied, now, now_unix, actions);
                    });
                }
            });
    }
}

impl eframe::App for App {
    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.receive_outcomes();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let now = ctx.input(|i| i.time);
        let mut actions = Vec::new();
        egui::Panel::top("top").show(ui, |ui| self.top_bar(ui, &ctx));
        egui::CentralPanel::default_margins().show(ui, |ui| match self.tab {
            Tab::Results => self.results_ui(ui, now, &mut actions),
            Tab::History => self.history_ui(ui, now, &mut actions),
        });
        self.apply(&ctx, actions, now);
    }
}

fn card_ui(
    ui: &mut egui::Ui,
    card: &Card,
    copied: &Option<(String, f64)>,
    now: f64,
    actions: &mut Vec<Action>,
) {
    egui::Frame::group(ui.style())
        .corner_radius(8.0)
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            match &card.kind {
                CardKind::Invalid(message) => {
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .small_button("🗙")
                                .on_hover_text("Remove from this list")
                                .clicked()
                            {
                                actions.push(Action::Remove(card.uid));
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                ui.colored_label(ORANGE, "⚠");
                                ui.add(egui::Label::new(message.as_str()).wrap());
                            });
                        });
                    });
                }
                CardKind::Lookup(lookup) => lookup_card_ui(ui, card, lookup, copied, now, actions),
            }
        });
}

fn lookup_card_ui(
    ui: &mut egui::Ui,
    card: &Card,
    lookup: &Lookup,
    copied: &Option<(String, f64)>,
    now: f64,
    actions: &mut Vec<Action>,
) {
    let resolved = card.resolved();
    let link = card.link().unwrap_or_default();
    let title = match &lookup.state {
        CardState::Done(r) => display_name(&r.profile.name).to_string(),
        CardState::Loading => "Loading…".to_string(),
        CardState::Failed(FetchError::NotFound(_)) => "Profile not found".to_string(),
        CardState::Failed(_) => "Couldn't load profile".to_string(),
    };
    let previous = previous_names(resolved);

    ui.horizontal_top(|ui| {
        let initial = resolved.and_then(|r| r.profile.name.chars().next());
        avatar_ui(
            ui,
            resolved.and_then(|r| r.avatar.as_deref()),
            initial,
            AVATAR_SIZE,
        );
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button("🗙")
                        .on_hover_text("Remove from this list")
                        .clicked()
                    {
                        actions.push(Action::Remove(card.uid));
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        ui.add(
                            egui::Label::new(RichText::new(title).heading().strong()).truncate(),
                        );
                    });
                });
            });
            if let Some(r) = resolved {
                ui.horizontal_wrapped(|ui| badges(ui, &r.profile));
            }
            if !previous.is_empty() {
                let mut aka = previous
                    .iter()
                    .take(AKA_INLINE)
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                if previous.len() > AKA_INLINE {
                    aka.push_str(&format!(" (+{})", previous.len() - AKA_INLINE));
                }
                ui.add(egui::Label::new(RichText::new(format!("aka {aka}")).weak()).wrap());
            }
            ui.horizontal_wrapped(|ui| {
                ui.add(egui::Hyperlink::from_label_and_url(&link, &link).open_in_new_tab(true));
                let key = format!("link-{}", card.uid);
                if copy_button(ui, "Copy", &key, copied, now) {
                    actions.push(Action::Copy {
                        text: link.clone(),
                        key,
                    });
                }
            });
            match &lookup.state {
                CardState::Loading => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new("Fetching profile from Steam…").weak());
                    });
                }
                CardState::Failed(err) => {
                    ui.horizontal_wrapped(|ui| {
                        let color = if matches!(err, FetchError::NotFound(_)) {
                            ORANGE
                        } else {
                            RED
                        };
                        ui.colored_label(color, err.to_string());
                        if ui.small_button("Retry").clicked() {
                            actions.push(Action::Retry(card.uid));
                        }
                    });
                }
                CardState::Done(_) => {}
            }
            egui::CollapsingHeader::new("Details")
                .id_salt("details")
                .show(ui, |ui| {
                    details_ui(ui, card, lookup, &previous, copied, now, actions)
                });
        });
    });
}

fn details_ui(
    ui: &mut egui::Ui,
    card: &Card,
    lookup: &Lookup,
    previous: &[PreviousName],
    copied: &Option<(String, f64)>,
    now: f64,
    actions: &mut Vec<Action>,
) {
    let resolved = card.resolved();
    // (label, shown, copied). The profile URL is already the card's main link.
    let mut rows: Vec<(&str, String, String)> = Vec::new();
    if let Some(id) = card.steam_id() {
        for (label, value) in [
            ("SteamID64", id.id64().to_string()),
            ("SteamID2", id.steam2()),
            ("SteamID3", id.steam3()),
            ("Account ID", id.account_id().to_string()),
        ] {
            rows.push((label, value.clone(), value));
        }
    }
    if let Some(custom) = resolved.and_then(|r| r.profile.custom_url.as_deref()) {
        rows.push((
            "Custom URL",
            format!("/id/{custom}"),
            format!("https://steamcommunity.com/id/{custom}"),
        ));
    }
    egui::Grid::new("ids")
        .num_columns(3)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            for (label, shown, value) in rows {
                ui.label(RichText::new(label).weak());
                ui.label(RichText::new(shown).monospace());
                let key = format!("{label}-{}", card.uid);
                if copy_button(ui, "Copy", &key, copied, now) {
                    actions.push(Action::Copy { text: value, key });
                }
                ui.end_row();
            }
            ui.label(RichText::new("Looked up from").weak());
            ui.label(format!("{} ({})", card.input, lookup.format.label()));
            ui.end_row();
        });
    if card.steam_id().is_none() {
        let note = match lookup.state {
            CardState::Loading => "Resolving the custom URL…",
            _ => "The custom URL couldn't be resolved, so the SteamID is unknown.",
        };
        ui.label(RichText::new(note).weak());
    }

    let Some(resolved) = resolved else { return };
    ui.add_space(6.0);
    ui.label(RichText::new("Name history").strong());
    if previous.is_empty() {
        let note = if resolved.profile.privacy == "public" {
            "No previous names."
        } else {
            "No previous names visible (Steam hides them on non-public profiles)."
        };
        ui.label(RichText::new(note).weak());
    }
    for name in previous {
        ui.horizontal_wrapped(|ui| {
            ui.label(&name.name);
            ui.label(RichText::new(&name.when).small().weak());
        });
    }
    if let Some(entry) = &resolved.entry {
        ui.add_space(6.0);
        ui.label(
            RichText::new(format!(
                "In your history since {} · looked up {} time{}",
                local_time(entry.first_seen),
                entry.lookups,
                if entry.lookups == 1 { "" } else { "s" }
            ))
            .small()
            .weak(),
        );
    }
}

fn history_row(
    ui: &mut egui::Ui,
    entry: &HistoryEntry,
    avatar_dir: &Path,
    copied: &Option<(String, f64)>,
    now: f64,
    now_unix: i64,
    actions: &mut Vec<Action>,
) {
    let link = SteamId::from_id64(entry.id64)
        .map(SteamId::profile_url)
        .unwrap_or_default();
    ui.allocate_ui(Vec2::new(ui.available_width(), HISTORY_ROW_HEIGHT), |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .small_button("🗑")
                .on_hover_text("Remove from history")
                .clicked()
            {
                actions.push(Action::DeleteHistory(entry.id64));
            }
            let key = format!("history-{}", entry.id64);
            if copy_button(ui, "Copy link", &key, copied, now) {
                actions.push(Action::Copy {
                    text: link.clone(),
                    key,
                });
            }
            if ui.small_button("Look up").clicked() {
                actions.push(Action::LookUp(entry.id64.to_string()));
            }
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let avatar = entry.avatar_file.as_ref().map(|f| avatar_dir.join(f));
                avatar_ui(
                    ui,
                    avatar.as_deref(),
                    entry.name.chars().next(),
                    HISTORY_AVATAR_SIZE,
                );
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        let name = ui
                            .add(
                                egui::Label::new(RichText::new(display_name(&entry.name)).strong())
                                    .sense(Sense::click()),
                            )
                            .on_hover_text("Look up again");
                        if name.clicked() {
                            actions.push(Action::LookUp(entry.id64.to_string()));
                        }
                        let was = renamed_from(entry);
                        if !was.is_empty() {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(format!("was: {}", was.join(", ")))
                                        .small()
                                        .weak(),
                                )
                                .truncate(),
                            );
                        }
                    });
                    let lookups = if entry.lookups == 1 {
                        "1 lookup".to_string()
                    } else {
                        format!("{} lookups", entry.lookups)
                    };
                    ui.add(
                        egui::Hyperlink::from_label_and_url(
                            RichText::new(format!(
                                "{} · {} · {lookups}",
                                entry.id64,
                                ago(entry.last_seen, now_unix)
                            ))
                            .small(),
                            &link,
                        )
                        .open_in_new_tab(true),
                    )
                    .on_hover_text(format!(
                        "Open the Steam profile\nFirst looked up {}\nLast looked up {}",
                        local_time(entry.first_seen),
                        local_time(entry.last_seen)
                    ));
                });
            });
        });
    });
}

fn badges(ui: &mut egui::Ui, profile: &Profile) {
    let weak = ui.visuals().weak_text_color();
    let accent = ui.visuals().hyperlink_color;
    let (online, color) = match &profile.online {
        OnlineState::Online => ("● Online".to_string(), accent),
        OnlineState::InGame(Some(game)) => (format!("● In game: {game}"), GREEN),
        OnlineState::InGame(None) => ("● In game".to_string(), GREEN),
        OnlineState::Offline => ("● Offline".to_string(), weak),
        OnlineState::Other(state) => (format!("● {state}"), weak),
    };
    badge(ui, online, color);
    if profile.vac_banned {
        badge(ui, "VAC banned".into(), RED);
    }
    if let Some(ban) = &profile.trade_ban {
        badge(ui, format!("Trade ban: {ban}"), ORANGE);
    }
    let privacy = match profile.privacy.as_str() {
        "public" => "Public profile".to_string(),
        "friendsonly" => "Friends-only profile".to_string(),
        "private" => "Private profile".to_string(),
        other => format!("Privacy: {other}"),
    };
    badge(ui, privacy, weak);
}

fn badge(ui: &mut egui::Ui, text: String, color: Color32) {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .stroke((1.0, color.gamma_multiply(0.55)))
        .corner_radius(10.0)
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| ui.label(RichText::new(text).small().color(color)));
}

fn avatar_ui(ui: &mut egui::Ui, path: Option<&Path>, initial: Option<char>, size: f32) {
    let size = Vec2::splat(size);
    if let Some(path) = path {
        ui.add(
            egui::Image::new(file_uri(path))
                .fit_to_exact_size(size)
                .corner_radius(6.0),
        );
        return;
    }
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let visuals = ui.visuals();
    ui.painter()
        .rect_filled(rect, 6.0, visuals.widgets.inactive.bg_fill);
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        initial.map_or('?', |c| c.to_uppercase().next().unwrap_or(c)),
        FontId::proportional(size.x * 0.45),
        visuals.weak_text_color(),
    );
}

/// A button that briefly reads "Copied" after it was used. Returns whether it was clicked.
fn copy_button(
    ui: &mut egui::Ui,
    label: &str,
    key: &str,
    copied: &Option<(String, f64)>,
    now: f64,
) -> bool {
    let fresh = copied
        .as_ref()
        .is_some_and(|(k, at)| k == key && now - at < COPIED_FEEDBACK_SECS);
    ui.small_button(if fresh { "✔ Copied" } else { label })
        .clicked()
}

fn empty_state(ui: &mut egui::Ui) {
    ui.add_space(32.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("Look up a Steam profile").heading());
        ui.add_space(8.0);
        ui.label(RichText::new("Paste one or more IDs above and press Enter. Accepted:").weak());
        ui.add_space(4.0);
        for example in [
            "76561197960287930  (SteamID64)",
            "STEAM_0:0:11101  (SteamID2)",
            "[U:1:22202]  (SteamID3)",
            "22202  (account ID)",
            "steamcommunity.com/profiles/… or /id/…",
            "gabelogannewell  (custom URL name)",
        ] {
            ui.label(RichText::new(example).monospace().weak());
        }
    });
}

/// Steam's previous names plus renames the app noticed itself, without the current name.
fn previous_names(resolved: Option<&Resolved>) -> Vec<PreviousName> {
    let Some(resolved) = resolved else {
        return Vec::new();
    };
    let current = &resolved.profile.name;
    let mut names: Vec<PreviousName> = Vec::new();
    for alias in &resolved.aliases {
        if &alias.name != current && !names.iter().any(|n| n.name == alias.name) {
            names.push(PreviousName {
                name: alias.name.clone(),
                when: alias.when.clone(),
            });
        }
    }
    if let Some(entry) = &resolved.entry {
        for rename in entry.renames.iter().rev() {
            if &rename.from != current && !names.iter().any(|n| n.name == rename.from) {
                names.push(PreviousName {
                    name: rename.from.clone(),
                    when: format!("seen by you, until {}", local_time(rename.at)),
                });
            }
        }
    }
    names
}

/// Names the app saw this profile use before, newest first.
fn renamed_from(entry: &HistoryEntry) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for rename in entry.renames.iter().rev() {
        if rename.from != entry.name && !names.contains(&rename.from.as_str()) {
            names.push(&rename.from);
        }
    }
    names
}

fn history_matches(entry: &HistoryEntry, filter: &str) -> bool {
    let id_match = SteamId::from_id64(entry.id64).is_some_and(|id| {
        let parsed = steamid::parse(filter).ok().map(|p| p.target);
        parsed == Some(Target::Id(id))
            || [
                id.id64().to_string(),
                id.steam2(),
                id.steam3(),
                id.account_id().to_string(),
            ]
            .iter()
            .any(|s| s.to_lowercase().contains(filter))
    });
    id_match
        || entry.name.to_lowercase().contains(filter)
        || entry
            .renames
            .iter()
            .any(|r| r.from.to_lowercase().contains(filter))
        || entry
            .custom_url
            .as_deref()
            .is_some_and(|c| c.to_lowercase().contains(filter))
}

fn display_name(name: &str) -> &str {
    if name.trim().is_empty() {
        "(no name)"
    } else {
        name
    }
}

fn file_uri(path: &Path) -> String {
    if cfg!(windows) {
        format!("file:///{}", path.display().to_string().replace('\\', "/"))
    } else {
        format!("file://{}", path.display())
    }
}

fn local_time(unix: i64) -> String {
    chrono::DateTime::from_timestamp(unix, 0)
        .map(|t| {
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

fn ago(unix: i64, now: i64) -> String {
    let secs = (now - unix).max(0);
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        86_400..2_592_000 => {
            let days = secs / 86_400;
            if days == 1 {
                "yesterday".into()
            } else {
                format!("{days} days ago")
            }
        }
        _ => local_time(unix)
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_string(),
    }
}

fn install_fallback_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    // The bundled monospace font covers symbols (●, box drawing, …) the proportional ones lack.
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .push("Hack".to_owned());
    for (i, group) in FALLBACK_FONTS.iter().enumerate() {
        let Some(bytes) = group.iter().find_map(|path| std::fs::read(path).ok()) else {
            continue;
        };
        let name = format!("fallback-{i}");
        fonts
            .font_data
            .insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts.families.entry(family).or_default().push(name.clone());
        }
    }
    ctx.set_fonts(fonts);
}

fn apply_style(ctx: &egui::Context) {
    ctx.set_theme(egui::ThemePreference::System);
    for (theme, accent) in [
        (Theme::Dark, STEAM_BLUE_DARK),
        (Theme::Light, STEAM_BLUE_LIGHT),
    ] {
        ctx.style_mut_of(theme, |style| {
            style.visuals.hyperlink_color = accent;
            style.visuals.selection.bg_fill = accent.gamma_multiply(0.35);
            style.visuals.selection.stroke.color = accent;
            style.spacing.item_spacing = egui::vec2(8.0, 6.0);
            style.spacing.button_padding = egui::vec2(8.0, 3.0);
            for (text_style, size) in [
                (TextStyle::Heading, 19.0),
                (TextStyle::Body, 14.0),
                (TextStyle::Button, 14.0),
                (TextStyle::Monospace, 13.0),
                (TextStyle::Small, 11.5),
            ] {
                if let Some(font) = style.text_styles.get_mut(&text_style) {
                    font.size = size;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Record, SeenRename};
    use crate::profile::Alias;
    use crate::test_util::TempDir;
    use egui::accesskit::Role;
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;

    const ROBIN: u64 = 76_561_197_960_435_530;

    /// An app with no lookup workers (cards stay "loading" unless a test resolves them) and a
    /// throwaway history directory.
    fn harness(tmp: &TempDir) -> Harness<'static, App> {
        let history = History::new(tmp.0.clone());
        Harness::builder()
            .with_size([680.0, 760.0])
            .build_eframe(move |cc| App::with_history(&cc.egui_ctx, history, 0, ""))
    }

    fn robin() -> Resolved {
        Resolved {
            profile: Profile {
                id: SteamId::from_id64(ROBIN).unwrap(),
                name: "Robin".into(),
                avatar_url: None,
                online: OnlineState::InGame(Some("Half-Life 3".into())),
                privacy: "public".into(),
                vac_banned: true,
                trade_ban: None,
                custom_url: Some("robinwalker".into()),
            },
            aliases: [
                "Robin",
                "Sekiro",
                "You too can be the proud owner",
                "vipz",
                "tastee",
                "Aeo",
            ]
            .iter()
            .map(|name| Alias {
                name: (*name).into(),
                when: "7 May, 2019 @ 9:13pm".into(),
            })
            .collect(),
            avatar: Some(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icon-256.png").into()),
            entry: Some(HistoryEntry {
                id64: ROBIN,
                name: "Robin".into(),
                avatar_file: None,
                custom_url: Some("robinwalker".into()),
                first_seen: 1_700_000_000,
                last_seen: 1_700_000_500,
                lookups: 2,
                renames: vec![SeenRename {
                    from: "RobinOld".into(),
                    to: "Robin".into(),
                    at: 1_700_000_500,
                }],
            }),
        }
    }

    /// Submits `query` through the input box the way a user would.
    fn type_query(harness: &mut Harness<'_, App>, query: &str) {
        let input = harness.get_by_role(Role::TextInput);
        input.focus();
        input.type_text(query);
        harness.run_steps(2);
        harness.key_press(Key::Enter);
        harness.run_steps(2);
    }

    fn resolve_first(harness: &mut Harness<'_, App>, resolved: Resolved) {
        let card = &mut harness.state_mut().cards[0];
        if let CardKind::Lookup(lookup) = &mut card.kind {
            lookup.state = CardState::Done(Box::new(resolved));
        }
        harness.run_steps(4);
    }

    #[test]
    fn batch_input_dedupes_and_flags_invalid_tokens() {
        let tmp = TempDir::new("ui-batch");
        let mut harness = harness(&tmp);
        type_query(&mut harness, "76561197960435530, bad! [U:1:169802]");
        let cards = &harness.state().cards;
        assert_eq!(
            cards.len(),
            2,
            "the SteamID3 is the same account as the ID64"
        );
        assert!(matches!(cards[0].kind, CardKind::Lookup(_)));
        assert!(matches!(cards[1].kind, CardKind::Invalid(_)));
        harness.get_by_label_contains("isn't a Steam ID");
        harness.get_by_label("https://steamcommunity.com/profiles/76561197960435530");
        harness.get_by_label("Results (2)");
    }

    #[test]
    fn resolved_card_shows_profile_and_details() {
        let tmp = TempDir::new("ui-card");
        let mut harness = harness(&tmp);
        type_query(&mut harness, "robinwalker");
        harness.get_by_label("https://steamcommunity.com/id/robinwalker");
        resolve_first(&mut harness, robin());

        harness.get_by_label("Robin");
        harness.get_by_label("aka Sekiro, You too can be the proud owner, vipz (+3)");
        harness.get_by_label("● In game: Half-Life 3");
        harness.get_by_label("VAC banned");
        harness.get_by_label("https://steamcommunity.com/profiles/76561197960435530");

        harness.get_by_label("Details").click();
        harness.run_steps(4);
        harness.get_by_label("STEAM_0:0:84901");
        harness.get_by_label("[U:1:169802]");
        harness.get_by_label("RobinOld");
        harness.get_by_label_contains("seen by you");
        harness.get_by_label("robinwalker (Custom URL name)");
    }

    #[test]
    fn copy_button_copies_the_link() {
        let tmp = TempDir::new("ui-copy");
        let mut harness = harness(&tmp);
        type_query(&mut harness, "76561197960435530");
        harness.get_by_label("Copy").click();
        harness.step();
        let copied = harness.output().platform_output.commands.iter().any(|c| {
            matches!(c, egui::OutputCommand::CopyText(t) if t == "https://steamcommunity.com/profiles/76561197960435530")
        });
        assert!(copied);
        harness.run_steps(2);
        harness.get_by_label("✔ Copied");
    }

    #[test]
    fn history_tab_lists_filters_and_relooks_up() {
        let tmp = TempDir::new("ui-history");
        let history = History::new(tmp.0.clone());
        for (id64, name, at) in [
            (ROBIN, "Robin", 10),
            (76_561_197_960_287_930, "Rabscuttle", 20),
        ] {
            history
                .record(
                    Record {
                        id64,
                        name,
                        avatar_file: None,
                        custom_url: None,
                    },
                    at,
                )
                .unwrap();
        }
        let mut harness = harness(&tmp);
        harness.get_by_label("History").click();
        harness.run_steps(3);
        harness.get_by_label("Robin");
        harness.get_by_label("Rabscuttle");

        let filter = harness.get_all_by_role(Role::TextInput).last().unwrap();
        filter.focus();
        filter.type_text("rabs");
        harness.run_steps(3);
        assert!(harness.query_by_label("Robin").is_none());
        // The first "Look up" is the main input's button; the last is the remaining row's.
        harness.get_all_by_label("Look up").last().unwrap().click();
        harness.run_steps(3);
        assert_eq!(harness.state().tab as u8, Tab::Results as u8);
        assert_eq!(
            harness.state().cards[0].steam_id().map(SteamId::id64),
            Some(76_561_197_960_287_930)
        );
    }

    /// Renders the main screens to PNGs for eyeballing. Needs a GPU, so it's opt-in:
    /// `SCREENSHOT_DIR=/some/dir cargo test render_screenshots -- --ignored`
    #[test]
    #[ignore]
    fn render_screenshots() {
        let dir =
            std::path::PathBuf::from(std::env::var("SCREENSHOT_DIR").expect("set SCREENSHOT_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        let tmp = TempDir::new("ui-shots");
        let history = History::new(tmp.0.clone());
        history
            .record(
                Record {
                    id64: ROBIN,
                    name: "Robin",
                    avatar_file: None,
                    custom_url: None,
                },
                1_700_000_000,
            )
            .unwrap();
        history
            .record(
                Record {
                    id64: ROBIN,
                    name: "Robin Walker",
                    avatar_file: None,
                    custom_url: None,
                },
                chrono::Utc::now().timestamp() - 7200,
            )
            .unwrap();
        for theme in [Theme::Dark, Theme::Light] {
            let history = History::new(tmp.0.clone());
            let mut harness = Harness::builder()
                .with_size([680.0, 1080.0])
                .build_eframe(move |cc| App::with_history(&cc.egui_ctx, history, 0, ""));
            harness.ctx.set_theme(theme);
            type_query(
                &mut harness,
                "robinwalker 76561197960287930 notreal! 名前のテスト",
            );
            resolve_first(&mut harness, robin());
            harness.get_all_by_label("Details").next().unwrap().click();
            for _ in 0..20 {
                harness.step();
                std::thread::sleep(Duration::from_millis(20));
            }
            let name = format!("{theme:?}").to_lowercase();
            harness
                .render()
                .unwrap()
                .save(dir.join(format!("results-{name}.png")))
                .unwrap();
            harness.get_by_label("History").click();
            harness.run_steps(4);
            harness
                .render()
                .unwrap()
                .save(dir.join(format!("history-{name}.png")))
                .unwrap();
        }
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(1000, 1030), "just now");
        assert_eq!(ago(0, 125), "2 min ago");
        assert_eq!(ago(0, 7200), "2 h ago");
        assert_eq!(ago(0, 86_400 + 5), "yesterday");
        assert_eq!(ago(0, 3 * 86_400), "3 days ago");
    }

    #[test]
    fn history_filter_matches_ids_and_names() {
        let entry = HistoryEntry {
            id64: 76_561_197_960_287_930,
            name: "Rabscuttle".into(),
            avatar_file: None,
            custom_url: Some("gabelogannewell".into()),
            first_seen: 0,
            last_seen: 0,
            lookups: 1,
            renames: vec![crate::history::SeenRename {
                from: "Gabe".into(),
                to: "Rabscuttle".into(),
                at: 0,
            }],
        };
        for filter in [
            "rabs",
            "gabe",
            "gabelogan",
            "22202",
            "steam_0:0:11101",
            "[u:1:22202]",
            "7656119796028",
        ] {
            assert!(history_matches(&entry, filter), "{filter}");
        }
        assert!(!history_matches(&entry, "nobody"));
    }
}
