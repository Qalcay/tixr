//! Brokers & Firms pop-up: every simulated participant, sortable, with a
//! drill-down into one broker's book.

use eframe::egui::{self, RichText};
use sigma_engine::broker::{BrokerId, BrokerKind};
use sigma_engine::protocol::{BrokerRow, Command};

use crate::app::App;
use crate::format;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum BrokerSort {
    Name,
    #[default]
    Equity,
    Return,
    Trades,
    Age,
}

#[derive(Default)]
pub struct BrokersUi {
    pub sort: BrokerSort,
    pub firms_only: bool,
    pub focused: Option<BrokerId>,
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
    egui::Window::new("Brokers & Firms").open(&mut open).default_size([760.0, 520.0]).resizable(true).show(ctx, |ui| {
        let Some(snap) = &app.snap else { return };
        let ui_state = &mut app.brokers_ui;
        let firms = snap.brokers.iter().filter(|b| b.kind == BrokerKind::Firm).count();
        let total_eq: f64 = snap.brokers.iter().map(|b| b.equity).sum();
        ui.horizontal(|ui| {
            ui.label(format!(
                "{} active ({} firms) · {} gone broke · combined equity {}",
                snap.brokers.len(),
                firms,
                snap.population.brokers_broke,
                format::money(total_eq)
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
        let mut rows: Vec<&BrokerRow> =
            snap.brokers.iter().filter(|b| !ui_state.firms_only || b.kind == BrokerKind::Firm).collect();
        match ui_state.sort {
            BrokerSort::Name => rows.sort_by(|a, b| a.name.cmp(&b.name)),
            BrokerSort::Equity => rows.sort_by(|a, b| b.equity.total_cmp(&a.equity)),
            BrokerSort::Return => rows.sort_by(|a, b| total_return(b).total_cmp(&total_return(a))),
            BrokerSort::Trades => rows.sort_by_key(|r| std::cmp::Reverse(r.trades)),
            BrokerSort::Age => rows.sort_by_key(|a| a.born_day),
        }
        let detail_h = if snap.broker_detail.is_some() { 190.0 } else { 0.0 };
        egui::ScrollArea::vertical()
            .id_salt("brokers")
            .max_height(ui.available_height() - detail_h)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("brokers_grid").striped(true).num_columns(9).spacing([10.0, 2.0]).show(ui, |ui| {
                    for h in ["Name", "Kind", "Strategy", "Equity", "Return", "Cash", "Pos", "Trades", "Since"] {
                        ui.label(RichText::new(h).strong());
                    }
                    ui.end_row();
                    for r in rows {
                        let sel = ui_state.focused == Some(r.id);
                        if ui.selectable_label(sel, &r.name).clicked() {
                            let next = if sel { None } else { Some(r.id) };
                            ui_state.focused = next;
                            focus_change = Some(next);
                        }
                        ui.label(if r.kind == BrokerKind::Firm { "Firm" } else { "Broker" });
                        ui.label(r.strategy.label());
                        ui.monospace(format::money(r.equity));
                        let ret = total_return(r);
                        ui.colored_label(
                            if ret >= 0.0 { pal.up } else { pal.down },
                            RichText::new(format::pct(ret)).monospace(),
                        );
                        ui.monospace(format::money(r.cash));
                        ui.monospace(format!("{}", r.positions));
                        ui.monospace(format!("{}", r.trades));
                        ui.monospace(if r.born_day <= 0 {
                            "pre-open".to_string()
                        } else {
                            format!("day {}", r.born_day)
                        });
                        ui.end_row();
                    }
                });
            });
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
            egui::ScrollArea::vertical().id_salt("bk_hold").max_height(150.0).show(ui, |ui| {
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
