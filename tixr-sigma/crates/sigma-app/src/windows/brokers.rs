//! Brokers & Firms pop-up: every simulated participant, sortable, with a
//! drill-down into one broker's book.

use std::sync::Arc;

use eframe::egui::{self, RichText, vec2};
use sigma_engine::broker::{BrokerId, BrokerKind, MAX_FIRMS, MAX_PLAIN_BROKERS};
use sigma_engine::history::ChartRange;
use sigma_engine::protocol::{BrokerRow, Command};

use crate::app::App;
use crate::chart;
use crate::format;

/// Column widths in points; rows are laid out by hand because only the
/// visible ones are built (there can be 16,384 participants).
const COLS: [(&str, f32); 9] = [
    ("Name", 190.0),
    ("Kind", 50.0),
    ("Strategy", 76.0),
    ("Equity", 84.0),
    ("Return", 70.0),
    ("Cash", 84.0),
    ("Pos", 36.0),
    ("Trades", 60.0),
    ("Since", 70.0),
];
const ROW_H: f32 = 18.0;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BrokerSort {
    Name,
    #[default]
    Equity,
    Return,
    Trades,
    Age,
}

pub struct BrokersUi {
    pub sort: BrokerSort,
    pub firms_only: bool,
    pub focused: Option<BrokerId>,
    pub range: ChartRange,
    /// Sorted row order, rebuilt only when the table or the sort changes
    /// (the engine shares one table across ~20 snapshots).
    order: Vec<u32>,
    order_key: Option<(usize, BrokerSort, bool)>,
}

impl Default for BrokersUi {
    fn default() -> Self {
        Self {
            sort: BrokerSort::default(),
            firms_only: false,
            focused: None,
            range: ChartRange::Year,
            order: Vec::new(),
            order_key: None,
        }
    }
}

impl BrokersUi {
    fn refresh_order(&mut self, rows: &Arc<Vec<BrokerRow>>) {
        let key = (Arc::as_ptr(rows) as usize, self.sort, self.firms_only);
        if self.order_key == Some(key) {
            return;
        }
        self.order_key = Some(key);
        let mut order: Vec<u32> =
            (0..rows.len() as u32).filter(|&i| !self.firms_only || rows[i as usize].kind == BrokerKind::Firm).collect();
        let row = |i: &u32| &rows[*i as usize];
        match self.sort {
            BrokerSort::Name => order.sort_by(|a, b| row(a).name.cmp(&row(b).name)),
            BrokerSort::Equity => order.sort_by(|a, b| row(b).equity.total_cmp(&row(a).equity)),
            BrokerSort::Return => order.sort_by(|a, b| total_return(row(b)).total_cmp(&total_return(row(a)))),
            BrokerSort::Trades => order.sort_by_key(|i| std::cmp::Reverse(row(i).trades)),
            BrokerSort::Age => order.sort_by_key(|i| row(i).born_day),
        }
        self.order = order;
    }
}

fn cell(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    ui.allocate_ui_with_layout(vec2(width, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_width(width);
        ui.set_max_width(width);
        add(ui);
    });
}

fn total_return(r: &BrokerRow) -> f64 {
    (r.equity / r.initial_capital - 1.0) * 100.0
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.windows.brokers {
        return;
    }
    let mut open = true;
    let pal = app.settings.theme.palette();
    let mut focus_change: Option<Option<BrokerId>> = None;
    let mut select_security = None;
    egui::Window::new("Brokers & Firms").open(&mut open).default_size([800.0, 640.0]).resizable(true).show(ctx, |ui| {
        let Some(snap) = &app.snap else { return };
        let ui_state = &mut app.brokers_ui;
        let p = &snap.population;
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} brokers (cap {}) · {} firms (cap {}) · {} gone broke · combined equity {}",
                p.brokers_alive,
                MAX_PLAIN_BROKERS,
                p.firms_alive,
                MAX_FIRMS,
                p.brokers_broke,
                format::money(snap.brokers.iter().map(|b| b.equity).sum())
            ));
            ui.checkbox(&mut ui_state.firms_only, "firms only");
        });
        ui.horizontal(|ui| {
            ui.label("Sort:");
            for (s, l) in [
                (BrokerSort::Equity, "equity"),
                (BrokerSort::Return, "return"),
                (BrokerSort::Trades, "trades"),
                (BrokerSort::Age, "age"),
                (BrokerSort::Name, "name"),
            ] {
                ui.selectable_value(&mut ui_state.sort, s, l);
            }
        });
        ui.separator();
        ui_state.refresh_order(&snap.brokers);
        ui.horizontal(|ui| {
            for (title, width) in COLS {
                cell(ui, width, |ui| {
                    ui.label(RichText::new(title).strong());
                });
            }
        });
        let detail_h = if snap.broker_detail.is_some() { 330.0 } else { 0.0 };
        let focused = ui_state.focused;
        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("brokers")
            .max_height((ui.available_height() - detail_h).max(80.0))
            .auto_shrink([false, false])
            .show_rows(ui, ROW_H, ui_state.order.len(), |ui, range| {
                for &i in &ui_state.order[range] {
                    let Some(r) = snap.brokers.get(i as usize) else { continue };
                    let sel = focused == Some(r.id);
                    ui.horizontal(|ui| {
                        cell(ui, COLS[0].1, |ui| {
                            if ui.selectable_label(sel, &r.name).clicked() {
                                clicked = Some(if sel { None } else { Some(r.id) });
                            }
                        });
                        cell(ui, COLS[1].1, |ui| {
                            ui.label(if r.kind == BrokerKind::Firm { "Firm" } else { "Broker" });
                        });
                        cell(ui, COLS[2].1, |ui| {
                            ui.label(r.strategy.label());
                        });
                        cell(ui, COLS[3].1, |ui| {
                            ui.monospace(format::money(r.equity));
                        });
                        let ret = total_return(r);
                        cell(ui, COLS[4].1, |ui| {
                            ui.colored_label(
                                if ret >= 0.0 { pal.up } else { pal.down },
                                RichText::new(format::pct(ret)).monospace(),
                            );
                        });
                        cell(ui, COLS[5].1, |ui| {
                            ui.monospace(format::money(r.cash));
                        });
                        cell(ui, COLS[6].1, |ui| {
                            ui.monospace(format!("{}", r.positions));
                        });
                        cell(ui, COLS[7].1, |ui| {
                            ui.monospace(format!("{}", r.trades));
                        });
                        cell(ui, COLS[8].1, |ui| {
                            ui.monospace(if r.born_day <= 0 {
                                "pre-open".to_string()
                            } else {
                                format!("day {}", r.born_day)
                            });
                        });
                    });
                }
            });
        if let Some(next) = clicked {
            ui_state.focused = next;
            focus_change = Some(next);
        }
        if let Some(d) = &snap.broker_detail {
            ui.separator();
            ui.horizontal(|ui| {
                ui.strong(&d.row.name);
                ui.label(format!(
                    "{} · {} · started with {} · realised {} · fees {} · interest {}{}",
                    if d.row.kind == BrokerKind::Firm { "Firm" } else { "Broker" },
                    d.row.strategy.label(),
                    format::money(d.row.initial_capital),
                    format::money(d.row.realized_pnl),
                    format::money(d.fees_paid),
                    format::money(d.interest_earned),
                    if d.row.listings_sponsored > 0 {
                        format!(" · {} listings underwritten", d.row.listings_sponsored)
                    } else {
                        String::new()
                    }
                ));
            });
            ui.horizontal(|ui| {
                ui.label("Equity:");
                for r in [ChartRange::Month, ChartRange::Year, ChartRange::ThreeYear] {
                    ui.selectable_value(&mut ui_state.range, r, r.label());
                }
            });
            chart::history_chart(
                ui,
                vec2(ui.available_width(), 120.0),
                &d.history,
                snap.time,
                ui_state.range,
                format::money,
                &pal,
            );
            egui::ScrollArea::vertical().id_salt("bk_hold").max_height(130.0).show(ui, |ui| {
                egui::Grid::new("bk_hold_grid").striped(true).num_columns(6).spacing([10.0, 1.0]).show(ui, |ui| {
                    for h in ["Ticker", "Shares", "Avg cost", "Price", "Value", "Unrealised"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for h in d.holdings.iter().take(60) {
                        if ui.link(h.ticker.as_str()).clicked() {
                            select_security = Some(h.id);
                        }
                        ui.monospace(format::shares(h.shares));
                        ui.monospace(format::price(h.avg_cost));
                        ui.monospace(format::price(h.price));
                        ui.monospace(format::money(h.value));
                        ui.colored_label(
                            if h.unrealized >= 0.0 { pal.up } else { pal.down },
                            RichText::new(format::money(h.unrealized)).monospace(),
                        );
                        ui.end_row();
                    }
                });
            });
        }
    });
    if let Some(f) = focus_change {
        app.send(Command::FocusBroker(f));
    }
    if let Some(id) = select_security {
        app.select(Some(id));
    }
    app.windows.brokers = open;
}
