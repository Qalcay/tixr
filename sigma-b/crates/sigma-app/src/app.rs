//! The display thread's application state and top-level layout.
//!
//! The app owns no market state. It keeps the latest `Frame` from the
//! engine plus purely visual state (sort order, open windows, crawl
//! positions) and sends `Command`s back. If the engine thread stalls, the
//! UI keeps drawing the last frame at full frame rate.

use crate::theme::{Palette, Theme};
use crate::ui::crawl::CrawlRow;
use crate::ui::{detail, drivers, list, windows};
use eframe::egui::{self, Color32, RichText};
use sigma_core::clock::SessionMode;
use sigma_core::log::Level;
use sigma_core::market::{EXCHANGE_NAMES, EXCHANGES, Sector};
use sigma_core::protocol::{Command, Frame, OrderKind, Row, Side};
use sigma_core::runtime::EngineHandle;
use sigma_core::security::SecurityId;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

/// The two menu-selectable layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    /// List left, chart centre, drivers dock along the bottom.
    Desk,
    /// Drivers column left, chart centre, list right, one combined crawl.
    Terminal,
}

/// Panel width presets. Panels never size themselves from content; they
/// only change through this menu (or not at all), so a huge price can
/// never push the layout around. Window resizing only resizes the centre.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelWidth {
    Narrow,
    Normal,
    Wide,
}

impl PanelWidth {
    pub fn list(self) -> f32 {
        match self {
            PanelWidth::Narrow => 250.0,
            PanelWidth::Normal => 300.0,
            PanelWidth::Wide => 360.0,
        }
    }
    pub fn side(self) -> f32 {
        match self {
            PanelWidth::Narrow => 250.0,
            PanelWidth::Normal => 290.0,
            PanelWidth::Wide => 340.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Ticker,
    Price,
    Change,
    Cap,
    ValueGap,
    Vol,
}

pub struct Settings {
    pub layout: Layout,
    pub width: PanelWidth,
    pub theme: Theme,
    pub speed: f64,
    pub session: SessionMode,
    pub budget_ms: f64,
    pub crawl_speed: f32,
    pub log_level: Level,
    pub seed_text: String,
}

pub struct OrderForm {
    pub notional: f64,
    pub side: Side,
    pub kind: OrderKind,
    pub trigger: f64,
    pub amount: f64,
}

pub struct IpoForm {
    pub sector: Sector,
    pub capital: f64,
    pub stake: f64,
}

pub struct SigmaApp {
    pub engine: EngineHandle,
    pub frame: Option<Frame>,
    pub table: Arc<Vec<Row>>,
    pub table_version: u64,
    /// Indices into `table`, filtered and sorted.
    pub view: Vec<u32>,
    pub view_dirty: bool,
    /// id -> (price, change) for crawl cells and lookups.
    pub quotes: HashMap<u32, (f32, f32)>,
    pub search: String,
    pub perk_filter: String,
    pub exchange_filter: Option<u8>,
    pub sort: SortKey,
    pub sort_desc: bool,
    pub selected: Option<SecurityId>,
    pub chart_daily: bool,
    pub crawl: Vec<CrawlRow>,
    pub crawl_combined: CrawlRow,
    pub settings: Settings,
    pub applied_theme: Option<Theme>,
    pub show_brokers: bool,
    pub show_personal: bool,
    pub show_settings: bool,
    pub broker_focus: Option<u16>,
    pub broker_sort: u8,
    pub order: OrderForm,
    pub ipo: IpoForm,
    pub toasts: VecDeque<(String, f64)>,
    pub paused: bool,
    pub last_watch: f64,
    pub visible_rows: Vec<SecurityId>,
}

impl SigmaApp {
    pub fn new(engine: EngineHandle, seed: u64) -> Self {
        Self {
            engine,
            frame: None,
            table: Arc::new(Vec::new()),
            table_version: 0,
            view: Vec::new(),
            view_dirty: true,
            quotes: HashMap::new(),
            search: String::new(),
            perk_filter: String::new(),
            exchange_filter: None,
            sort: SortKey::Cap,
            sort_desc: true,
            selected: None,
            chart_daily: false,
            crawl: (0..EXCHANGES).map(|_| CrawlRow::default()).collect(),
            crawl_combined: CrawlRow::default(),
            settings: Settings {
                layout: Layout::Desk,
                width: PanelWidth::Normal,
                theme: Theme::Dark,
                speed: 1.0,
                session: SessionMode::Regular,
                budget_ms: 1.5,
                crawl_speed: 55.0,
                log_level: sigma_core::log::level(),
                seed_text: format!("{seed}"),
            },
            applied_theme: None,
            show_brokers: false,
            show_personal: false,
            show_settings: false,
            broker_focus: None,
            broker_sort: 0,
            order: OrderForm { notional: 10_000.0, side: Side::Buy, kind: OrderKind::Limit, trigger: 0.0, amount: 10_000.0 },
            ipo: IpoForm { sector: Sector::Tech, capital: 250_000.0, stake: 0.5 },
            toasts: VecDeque::new(),
            paused: false,
            last_watch: 0.0,
            visible_rows: Vec::new(),
        }
    }

    pub fn pal(&self) -> Palette {
        self.settings.theme.palette()
    }

    pub fn send(&self, cmd: Command) {
        self.engine.send(cmd);
    }

    pub fn select(&mut self, id: SecurityId) {
        self.selected = Some(id);
        self.send(Command::Select(Some(id)));
    }

    fn ingest(&mut self, now: f64) {
        let Some(mut f) = self.engine.latest() else { return };
        if let Some(t) = f.table.take() {
            self.table = t;
            self.table_version = f.table_version;
            self.view_dirty = true;
            self.quotes = self.table.iter().map(|r| (r.id.0, (r.price, r.change_pct))).collect();
        }
        for n in f.notices.drain(..) {
            self.toasts.push_back((n, now + 6.0));
        }
        while self.toasts.len() > 6 {
            self.toasts.pop_front();
        }
        // The engine clears the selection when a security exits.
        if self.selected.is_some() && f.selected.is_none() && f.clock.t > 0.0 {
            if let Some(id) = self.selected
                && !self.quotes.contains_key(&id.0)
            {
                self.selected = None;
            }
        }
        self.frame = Some(f);
    }

    /// Tell the engine which securities are on screen, twice a second.
    fn send_watch(&mut self, now: f64) {
        if now - self.last_watch < 0.5 {
            return;
        }
        self.last_watch = now;
        let mut ids: Vec<SecurityId> = std::mem::take(&mut self.visible_rows);
        for row in &self.crawl {
            ids.extend(row.visible_ids());
        }
        ids.extend(self.crawl_combined.visible_ids());
        ids.sort_unstable();
        ids.dedup();
        self.send(Command::Watch(ids));
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.label(RichText::new("TIXR-σ").strong().color(self.pal().accent));
            ui.menu_button("View", |ui| {
                ui.label("Layout");
                ui.radio_value(&mut self.settings.layout, Layout::Desk, "Desk (dock at bottom)");
                ui.radio_value(&mut self.settings.layout, Layout::Terminal, "Terminal (three columns)");
                ui.separator();
                ui.label("Panel width");
                ui.radio_value(&mut self.settings.width, PanelWidth::Narrow, "Narrow");
                ui.radio_value(&mut self.settings.width, PanelWidth::Normal, "Normal");
                ui.radio_value(&mut self.settings.width, PanelWidth::Wide, "Wide");
                ui.separator();
                ui.radio_value(&mut self.settings.theme, Theme::Dark, "Dark theme");
                ui.radio_value(&mut self.settings.theme, Theme::Light, "Light theme");
            });
            ui.menu_button("Windows", |ui| {
                if ui.button("Brokers & firms").clicked() {
                    self.show_brokers = true;
                    ui.close();
                }
                if ui.button("Personal finances").clicked() {
                    self.show_personal = true;
                    ui.close();
                }
                if ui.button("Settings").clicked() {
                    self.show_settings = true;
                    ui.close();
                }
            });
            ui.menu_button("Sim", |ui| {
                let label = if self.paused { "Resume" } else { "Pause" };
                if ui.button(label).clicked() {
                    self.paused = !self.paused;
                    self.send(Command::SetPaused(self.paused));
                    ui.close();
                }
                ui.separator();
                for s in [1.0, 4.0, 16.0, 64.0, 256.0] {
                    if ui.selectable_label(self.settings.speed == s, format!("{s}x")).clicked() {
                        self.settings.speed = s;
                        self.send(Command::SetSpeed(s));
                        ui.close();
                    }
                }
            });
            ui.separator();
            if let Some(f) = &self.frame {
                let c = &f.clock;
                let open = if c.open { RichText::new("OPEN").color(self.pal().up) } else { RichText::new("CLOSED").color(self.pal().down) };
                ui.label(format!("Day {} {} {:02}:{:02}", c.parts.day, c.parts.weekday_name(), c.parts.hour, c.parts.minute));
                ui.label(open.strong());
                if c.paused {
                    ui.label(RichText::new("PAUSED").color(self.pal().accent));
                }
                ui.label(RichText::new(format!("{}x", c.speed)).color(self.pal().weak));
                ui.separator();
                let s = &f.stats;
                ui.label(RichText::new(format!("{} listed", s.live)).color(self.pal().weak));
                if let Some(comp) = f.indices.first() {
                    ui.label(format!("Composite {:.1}", comp.level));
                    ui.label(RichText::new(crate::fmt::pct(comp.change_pct)).color(self.settings.theme.signed(comp.change_pct)));
                }
            }
        });
    }

    fn crawl_band(&mut self, ui: &mut egui::Ui, combined: bool) {
        let pal = self.pal();
        let dt = ui.ctx().input(|i| i.stable_dt);
        let Some(f) = self.frame.as_ref() else {
            ui.label("Starting engine...");
            return;
        };
        if combined {
            let all: Vec<_> = (0..EXCHANGES).rev().flat_map(|e| f.crawl.get(e).cloned().unwrap_or_default().into_iter().take(16)).collect();
            self.crawl_combined.show(ui, &pal, &all, &self.quotes, self.settings.crawl_speed, dt, 20.0);
        } else {
            for ex in (0..EXCHANGES).rev() {
                ui.horizontal(|ui| {
                    ui.add_sized([56.0, 18.0], egui::Label::new(RichText::new(EXCHANGE_NAMES[ex]).monospace().strong().color(pal.accent)));
                    let src = f.crawl.get(ex).map(|v| v.as_slice()).unwrap_or(&[]);
                    self.crawl[ex].show(ui, &pal, src, &self.quotes, self.settings.crawl_speed, dt, 18.0);
                });
            }
        }
    }

    fn toasts(&mut self, ctx: &egui::Context, now: f64) {
        self.toasts.retain(|(_, until)| *until > now);
        if self.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::unique("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .show(ctx, |ui| {
                for (t, _) in &self.toasts {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(RichText::new(t).color(Color32::WHITE));
                    });
                }
            });
    }
}

impl eframe::App for SigmaApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if self.applied_theme != Some(self.settings.theme) {
            self.settings.theme.apply(&ctx);
            self.applied_theme = Some(self.settings.theme);
        }
        self.ingest(now);
        if self.view_dirty {
            list::rebuild_view(self);
        }

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        let combined = self.settings.layout == Layout::Terminal;
        egui::Panel::top("crawl").resizable(false).show(ui, |ui| self.crawl_band(ui, combined));

        match self.settings.layout {
            Layout::Desk => {
                egui::Panel::left("list").resizable(false).exact_size(self.settings.width.list()).show(ui, |ui| list::show(self, ui));
                egui::Panel::bottom("dock").resizable(false).exact_size(236.0).show(ui, |ui| drivers::dock(self, ui));
                egui::CentralPanel::default().show(ui, |ui| detail::show(self, ui));
            }
            Layout::Terminal => {
                egui::Panel::left("drivers").resizable(false).exact_size(self.settings.width.side()).show(ui, |ui| drivers::column(self, ui));
                egui::Panel::right("list").resizable(false).exact_size(self.settings.width.list()).show(ui, |ui| list::show(self, ui));
                egui::CentralPanel::default().show(ui, |ui| detail::show(self, ui));
            }
        }

        windows::brokers(self, &ctx);
        windows::personal(self, &ctx);
        windows::settings(self, &ctx);
        self.toasts(&ctx, now);
        self.send_watch(now);
        // The crawl animates continuously.
        ctx.request_repaint();
    }
}
