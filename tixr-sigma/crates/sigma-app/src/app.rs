//! Application state and the per-frame layout.
//!
//! The Display thread never touches engine state. Each frame it:
//! 1. takes the newest [`Snapshot`] off the channel (if any),
//! 2. draws panels from that owned snapshot,
//! 3. turns clicks into [`Command`]s sent back to the engine.
//!
//! # Layout rule: the window decides sizes, content never does
//! Side and bottom panels use `exact_size` and are not user-resizable, and
//! every number goes through a bounded-width formatter (`format.rs`). So a
//! $40,000 stock or a $3T fund cannot widen a panel; only resizing the window
//! changes the layout. Two layouts are offered from the View menu.

use std::collections::VecDeque;
use std::path::PathBuf;

use eframe::egui::{self, FontId, RichText};
use sigma_engine::EngineHandle;
use sigma_engine::console::Level;
use sigma_engine::history::ChartRange;
use sigma_engine::protocol::{Command, Notice, OrderSize, Row, Side, Snapshot};
use sigma_engine::save::{self, SlotInfo};
use sigma_engine::security::SecurityId;
use sigma_engine::time::REAL_SECS_PER_SIM_DAY;

use crate::format;
use crate::panels;
use crate::theme::Theme;
use crate::ticker::{CellText, Crawl};
use crate::windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutKind {
    /// List left, chart centre, drivers dock along the bottom, ticker on top.
    Classic,
    /// Drivers column left, chart centre, list right, ticker along the bottom.
    Terminal,
}

impl LayoutKind {
    pub fn label(self) -> &'static str {
        match self {
            LayoutKind::Classic => "Classic",
            LayoutKind::Terminal => "Terminal",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Ticker,
    Price,
    Change,
    Cap,
}

pub struct Settings {
    pub speed: f64,
    pub closed_boost: bool,
    pub ticker_step: u32,
    pub ticker_rows: usize,
    pub theme: Theme,
    pub layout: LayoutKind,
    pub log_level: Level,
    pub ui_scale: f32,
    pub show_forecast: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            speed: 1.0,
            closed_boost: false,
            ticker_step: 1,
            ticker_rows: 4,
            theme: Theme::Dark,
            layout: LayoutKind::Classic,
            log_level: Level::Info,
            ui_scale: 1.0,
            show_forecast: true,
        }
    }
}

#[derive(Default)]
pub struct OpenWindows {
    pub brokers: bool,
    pub personal: bool,
    pub financials: bool,
    pub settings: bool,
}

pub struct OrderTicket {
    pub amount: f64,
    pub by_notional: bool,
}

struct Toast {
    text: String,
    error: bool,
    until: f64,
}

pub struct App {
    pub engine: EngineHandle,
    pub snap: Option<Box<Snapshot>>,
    pub settings: Settings,
    applied: Option<(Theme, f32)>,
    pub paused: bool,
    pub selected: Option<SecurityId>,
    pub search: String,
    pub exchange_filter: Option<u8>,
    pub sort: SortKey,
    pub sort_desc: bool,
    /// Indices into `snap.rows`, filtered and sorted; rebuilt per snapshot.
    pub view: Vec<usize>,
    view_dirty: bool,
    pub chart_range: ChartRange,
    pub ticket: OrderTicket,
    pub windows: OpenWindows,
    pub brokers_ui: windows::brokers::BrokersUi,
    pub personal_ui: windows::personal::PersonalUi,
    pub settings_ui: windows::settings::SettingsUi,
    pub financials_ui: windows::financials::FinancialsUi,
    /// Directory holding the save slots (one sub-directory each).
    save_root: PathBuf,
    save_slot: String,
    /// Slots on disk, re-read each time the Game menu is opened.
    slots: Vec<SlotInfo>,
    crawls: [Crawl; 4],
    crawl_cursor: [usize; 4],
    /// Per exchange: row indices of its 48 largest listings (ticker content).
    crawl_lists: [Vec<usize>; 4],
    toasts: VecDeque<Toast>,
}

impl App {
    pub fn new(engine: EngineHandle, save_root: PathBuf) -> Self {
        Self {
            engine,
            snap: None,
            settings: Settings::default(),
            applied: None,
            paused: false,
            selected: None,
            search: String::new(),
            exchange_filter: None,
            sort: SortKey::Cap,
            sort_desc: true,
            view: Vec::new(),
            view_dirty: true,
            chart_range: ChartRange::Month,
            ticket: OrderTicket { amount: 10_000.0, by_notional: true },
            windows: OpenWindows::default(),
            brokers_ui: Default::default(),
            personal_ui: Default::default(),
            settings_ui: Default::default(),
            financials_ui: Default::default(),
            save_root,
            save_slot: "slot1".to_string(),
            slots: Vec::new(),
            crawls: Default::default(),
            crawl_cursor: [0; 4],
            crawl_lists: Default::default(),
            toasts: VecDeque::new(),
        }
    }

    pub fn send(&self, cmd: Command) {
        self.engine.send(cmd);
    }

    pub fn select(&mut self, id: Option<SecurityId>) {
        if self.selected != id {
            self.selected = id;
            self.send(Command::Select(id));
        }
    }

    pub fn order(&self, side: Side) {
        if let Some(id) = self.selected {
            let size = if self.ticket.by_notional {
                OrderSize::Notional(self.ticket.amount)
            } else {
                OrderSize::Shares(self.ticket.amount)
            };
            self.send(Command::Order { id, side, size });
        }
    }

    pub fn mark_view_dirty(&mut self) {
        self.view_dirty = true;
    }

    fn take_snapshot(&mut self, now: f64) {
        let Some(mut snap) = self.engine.latest() else { return };
        // A restart (new seed) invalidates everything scrolling on screen.
        if self.snap.as_ref().is_some_and(|old| old.seed != snap.seed) {
            for c in &mut self.crawls {
                c.clear();
            }
            self.selected = None;
        }
        for n in snap.notices.drain(..) {
            let (text, error) = match n {
                Notice::Info(t) => (t, false),
                Notice::Error(t) => (t, true),
            };
            self.toasts.push_back(Toast { text, error, until: now + 5.0 });
        }
        while self.toasts.len() > 5 {
            self.toasts.pop_front();
        }
        // The engine drops a selection when the security delists.
        if self.selected.is_some() && snap.detail.is_none() && !snap.rows.iter().any(|r| Some(r.id) == self.selected) {
            self.selected = None;
        }
        for (ex, list) in self.crawl_lists.iter_mut().enumerate() {
            let mut idx: Vec<usize> = (0..snap.rows.len()).filter(|&i| snap.rows[i].exchange as usize == ex).collect();
            idx.sort_by(|&a, &b| snap.rows[b].market_cap.total_cmp(&snap.rows[a].market_cap));
            idx.truncate(48);
            *list = idx;
        }
        // First snapshot: open on the largest listing so the chart is live.
        if self.snap.is_none()
            && self.selected.is_none()
            && let Some(top) = snap.rows.iter().max_by(|a, b| a.market_cap.total_cmp(&b.market_cap))
        {
            self.selected = Some(top.id);
            self.engine.send(Command::Select(Some(top.id)));
        }
        self.snap = Some(snap);
        self.view_dirty = true;
    }

    fn rebuild_view(&mut self) {
        let Some(snap) = &self.snap else { return };
        let q = self.search.trim().to_ascii_uppercase();
        let rows = &snap.rows;
        let mut view: Vec<usize> = (0..rows.len())
            .filter(|&i| {
                let r = &rows[i];
                self.exchange_filter.is_none_or(|e| e == r.exchange)
                    && (q.is_empty() || r.ticker.as_str().contains(&q) || r.persona.letters().contains(&q))
            })
            .collect();
        let key = |r: &Row| -> f64 {
            match self.sort {
                SortKey::Ticker => 0.0,
                SortKey::Price => r.price,
                SortKey::Change => r.change_pct as f64,
                SortKey::Cap => r.market_cap,
            }
        };
        if self.sort == SortKey::Ticker {
            view.sort_by(|&a, &b| rows[a].ticker.cmp(&rows[b].ticker));
        } else {
            view.sort_by(|&a, &b| key(&rows[a]).total_cmp(&key(&rows[b])));
        }
        if self.sort_desc {
            view.reverse();
        }
        self.view = view;
        self.view_dirty = false;
    }

    fn apply_style(&mut self, ctx: &egui::Context) {
        let want = (self.settings.theme, self.settings.ui_scale);
        if self.applied != Some(want) {
            want.0.apply(ctx);
            ctx.set_zoom_factor(want.1);
            self.applied = Some(want);
        }
    }

    // ------------------------------------------------------------ top bars

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            let game = ui.menu_button("Game", |ui| {
                if ui.button("New market (random seed)").clicked() {
                    self.send(Command::Restart { seed: save::fresh_seed() });
                    self.selected = None;
                    ui.close();
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label("Slot");
                    ui.add(egui::TextEdit::singleline(&mut self.save_slot).desired_width(110.0).char_limit(32));
                    let valid = save::slot_dir(&self.save_root, &self.save_slot).is_ok();
                    if ui.add_enabled(valid, egui::Button::new("Save")).clicked() {
                        self.send(Command::Save { slot: self.save_slot.clone() });
                        ui.close();
                    }
                });
                ui.label(RichText::new("Load").weak());
                if self.slots.is_empty() {
                    ui.label(RichText::new("no saved games yet").small());
                }
                let mut load = None;
                for slot in &self.slots {
                    let size_mb = (slot.state_bytes + slot.history_bytes) as f64 / 1e6;
                    let age = slot.modified.and_then(|t| t.elapsed().ok()).map(format::age).unwrap_or_default();
                    if ui.button(format!("{}  ({size_mb:.1} MB, {age})", slot.name)).clicked() {
                        load = Some(slot.name.clone());
                    }
                }
                if let Some(slot) = load {
                    self.save_slot = slot.clone();
                    self.send(Command::Load { slot });
                    self.selected = None;
                    ui.close();
                }
                ui.label(RichText::new(format!("in {}", self.save_root.display())).small().weak());
                ui.separator();
                if ui.button("Quit").clicked() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
            // Re-read the slot list when the menu is opened, not every frame.
            if game.response.clicked() {
                self.slots = save::list_slots(&self.save_root);
            }
            ui.menu_button("View", |ui| {
                ui.label(RichText::new("Layout").weak());
                for l in [LayoutKind::Classic, LayoutKind::Terminal] {
                    if ui.radio_value(&mut self.settings.layout, l, l.label()).clicked() {
                        ui.close();
                    }
                }
                ui.separator();
                ui.label(RichText::new("Theme").weak());
                for t in [Theme::Dark, Theme::Light] {
                    ui.radio_value(&mut self.settings.theme, t, t.label());
                }
                ui.separator();
                ui.checkbox(&mut self.settings.show_forecast, "Forecast cone");
            });
            ui.menu_button("Windows", |ui| {
                ui.toggle_value(&mut self.windows.brokers, "Brokers & Firms");
                ui.toggle_value(&mut self.windows.personal, "Personal finance");
                ui.toggle_value(&mut self.windows.financials, "Financials");
                ui.toggle_value(&mut self.windows.settings, "Settings");
            });
            ui.separator();
            let label = if self.paused { "Resume" } else { "Pause" };
            if ui.button(label).clicked() {
                self.paused = !self.paused;
                self.send(Command::SetPaused(self.paused));
            }
            // Speed buttons exist only in development builds; the engine
            // ignores speed commands in a time-locked build regardless.
            let time_controls = self.snap.as_ref().is_some_and(|s| s.time_controls);
            if time_controls {
                for (name, s) in [("1x", 1.0), ("16x", 16.0), ("256x", 256.0)] {
                    if ui.selectable_label(self.settings.speed == s, name).clicked() {
                        self.settings.speed = s;
                        self.send(Command::SetSpeed(s));
                    }
                }
            }
            if let Some(s) = &self.snap {
                ui.separator();
                let pace = if !time_controls {
                    "time-locked".to_string()
                } else {
                    let boost = if s.closed_boost_active { " (closed: 8x)" } else { "" };
                    format!("DEV {:.1} real min/day{boost}", REAL_SECS_PER_SIM_DAY / 60.0 / s.speed)
                };
                let marked = if s.dev_time_used { "  |  dev-time save" } else { "" };
                ui.monospace(format!(
                    "Day {} {} {}  |  {}  |  {}{}",
                    s.day,
                    s.weekday,
                    s.clock,
                    pace,
                    s.regime.label(),
                    marked
                ));
            }
        });
    }

    fn ticker_strip(&mut self, ui: &mut egui::Ui) {
        let Some(snap) = &self.snap else {
            ui.label("Starting engine...");
            return;
        };
        let pal = self.settings.theme.palette();
        let font = FontId::monospace(12.5);
        let step = if self.paused { 0 } else { self.settings.ticker_step };
        for ex in 0..self.settings.ticker_rows.min(4) {
            let view = &snap.exchanges[ex];
            ui.horizontal(|ui| {
                let status = if view.is_open { pal.up } else { pal.text_weak };
                ui.add_sized(
                    [178.0, 18.0],
                    egui::Label::new(
                        RichText::new(format!(
                            "{} {:>8} {}",
                            view.name,
                            format::price(view.index),
                            format::pct(view.change_pct)
                        ))
                        .monospace()
                        .color(status),
                    )
                    .truncate(),
                );
                let list = &self.crawl_lists[ex];
                let cursor = &mut self.crawl_cursor[ex];
                let rows = &snap.rows;
                self.crawls[ex].ui(ui, 18.0, step, &font, || {
                    if list.is_empty() {
                        return None;
                    }
                    *cursor = (*cursor + 1) % list.len();
                    let r = rows.get(list[*cursor])?;
                    let up = r.change_pct >= 0.0;
                    let arrow = if up { "▲" } else { "▼" };
                    let col = if up { pal.up } else { pal.down };
                    let cell: CellText = vec![
                        (format!("{} ", r.ticker), pal.text),
                        (format!("{} ", format::price(r.price)), pal.text_weak),
                        (format!("{arrow}{:.2}%", r.change_pct.abs()), col),
                    ];
                    Some(cell)
                });
            });
        }
    }

    fn toasts(&mut self, ctx: &egui::Context, now: f64) {
        self.toasts.retain(|t| t.until > now);
        if self.toasts.is_empty() {
            return;
        }
        let pal = self.settings.theme.palette();
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for t in &self.toasts {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(360.0);
                        ui.colored_label(if t.error { pal.down } else { pal.text }, &t.text);
                    });
                }
            });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = ctx.input(|i| i.time);
        self.take_snapshot(now);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.apply_style(&ctx);
        if self.view_dirty {
            self.rebuild_view();
        }
        let ticker_h = 6.0 + 22.0 * self.settings.ticker_rows as f32;

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        match self.settings.layout {
            LayoutKind::Classic => {
                egui::Panel::top("ticker").exact_size(ticker_h).resizable(false).show(ui, |ui| self.ticker_strip(ui));
                egui::Panel::left("list")
                    .exact_size(300.0)
                    .resizable(false)
                    .show(ui, |ui| panels::security_list(self, ui));
                egui::Panel::bottom("dock").exact_size(236.0).resizable(false).show(ui, |ui| panels::dock(self, ui));
                egui::CentralPanel::default().show(ui, |ui| panels::detail(self, ui, 0.0));
            }
            LayoutKind::Terminal => {
                egui::Panel::bottom("ticker")
                    .exact_size(ticker_h)
                    .resizable(false)
                    .show(ui, |ui| self.ticker_strip(ui));
                egui::Panel::left("drivers").exact_size(270.0).resizable(false).show(ui, |ui| {
                    egui::ScrollArea::vertical().id_salt("drv").show(ui, |ui| panels::drivers_column(self, ui))
                });
                egui::Panel::right("list")
                    .exact_size(300.0)
                    .resizable(false)
                    .show(ui, |ui| panels::security_list(self, ui));
                egui::CentralPanel::default().show(ui, |ui| {
                    panels::detail(self, ui, 170.0);
                    ui.separator();
                    panels::feed(self, ui, 140.0);
                });
            }
        }

        windows::brokers::show(self, &ctx);
        windows::personal::show(self, &ctx);
        windows::financials::show(self, &ctx);
        windows::settings::show(self, &ctx);
        self.toasts(&ctx, now);
        // The crawl animates every frame; vsync caps this at the display rate.
        ctx.request_repaint();
    }
}
