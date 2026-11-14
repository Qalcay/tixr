//! Pop-up windows: Brokers & firms, Personal finances, Settings.

use crate::app::{Layout, PanelWidth, SigmaApp};
use crate::fmt;
use crate::theme::Theme;
use eframe::egui::{self, RichText};
use sigma_core::clock::SessionMode;
use sigma_core::engine::sectors;
use sigma_core::log::Level;
use sigma_core::protocol::{BrokerRow, Command};

pub fn brokers(app: &mut SigmaApp, ctx: &egui::Context) {
    if !app.show_brokers {
        return;
    }
    let Some(f) = app.frame.clone() else { return };
    let pal = app.pal();
    let mut open = app.show_brokers;
    egui::Window::new("Brokers & firms").open(&mut open).default_size([760.0, 520.0]).show(ctx, |ui| {
        let alive = f.brokers.iter().filter(|b| b.alive).count();
        let listed = f.brokers.iter().filter(|b| b.listed.is_some()).count();
        ui.label(format!("{alive} active of {} seats · {listed} are listed firms · {} have gone broke so far", f.brokers.len(), f.stats.brokers_broke));
        ui.horizontal(|ui| {
            ui.label("Sort by");
            for (i, l) in ["Equity", "Return", "Strategy", "Age"].iter().enumerate() {
                ui.selectable_value(&mut app.broker_sort, i as u8, *l);
            }
        });
        let mut rows: Vec<&BrokerRow> = f.brokers.iter().collect();
        match app.broker_sort {
            0 => rows.sort_by(|a, b| b.equity.total_cmp(&a.equity)),
            1 => rows.sort_by(|a, b| b.roe.total_cmp(&a.roe)),
            2 => rows.sort_by_key(|b| b.strategy.name()),
            _ => rows.sort_by(|a, b| b.age_days.total_cmp(&a.age_days)),
        }
        egui::ScrollArea::vertical().id_salt("brokers").max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("brokergrid").num_columns(9).striped(true).show(ui, |ui| {
                for h in ["Broker", "Strategy", "Firm", "Equity", "vs start", "ROE", "Cash", "Pos.", "Trades"] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                for b in rows {
                    let name = if b.alive { RichText::new(&b.name) } else { RichText::new(format!("{} (broke)", b.name)).color(pal.down) };
                    if ui.selectable_label(app.broker_focus == Some(b.index), name).clicked() {
                        app.broker_focus = Some(b.index);
                    }
                    ui.label(b.strategy.name());
                    ui.label(b.listed.map_or("-".to_string(), |t| t.as_str().to_string()));
                    ui.label(RichText::new(fmt::money(b.equity)).monospace());
                    let vs = (b.equity / b.capital_in - 1.0) * 100.0;
                    ui.label(RichText::new(fmt::pct(vs)).monospace().color(app.settings.theme.signed(vs)));
                    ui.label(RichText::new(fmt::rate(b.roe)).monospace());
                    let cash_share = if b.equity > 0.0 { b.cash / b.equity } else { 0.0 };
                    ui.label(RichText::new(fmt::rate(cash_share)).monospace());
                    ui.label(format!("{}", b.positions));
                    ui.label(format!("{}", b.trades));
                    ui.end_row();
                }
            });
        });
        ui.separator();
        if let Some(b) = app.broker_focus.and_then(|i| f.brokers.iter().find(|b| b.index == i)) {
            ui.label(RichText::new(&b.name).strong());
            ui.label(format!(
                "{} · started with {} · {:.0} days old · ROE {}",
                b.strategy.name(),
                fmt::money(b.capital_in),
                b.age_days,
                fmt::rate(b.roe)
            ));
            if let Some(t) = b.listed {
                ui.label(format!("Listed as {} (valued on its book and return on equity)", t.as_str()));
            }
            ui.horizontal_wrapped(|ui| {
                ui.label("Top holdings:");
                for (t, v) in &b.top_holdings {
                    ui.label(RichText::new(format!("{} {}", t.as_str(), fmt::money(*v))).monospace());
                }
            });
        }
        ui.separator();
        let m = &f.money;
        ui.label(RichText::new("Money in the system").strong());
        egui::Grid::new("money").num_columns(4).show(ui, |ui| {
            ui.label("Cash held (all)");
            ui.label(RichText::new(fmt::money(m.cash_total)).monospace());
            ui.label("Market cap (all)");
            ui.label(RichText::new(fmt::money(m.market_cap_total)).monospace());
            ui.end_row();
            ui.label("Interest paid");
            ui.label(RichText::new(fmt::money(m.interest_paid)).monospace());
            ui.label("Dividends paid");
            ui.label(RichText::new(fmt::money(m.dividends_paid)).monospace());
            ui.end_row();
            ui.label("Fees collected");
            ui.label(RichText::new(fmt::money(m.fees_collected)).monospace());
            ui.label("Takeover cash");
            ui.label(RichText::new(fmt::money(m.takeover_cash)).monospace());
            ui.end_row();
            ui.label("Broker capital in");
            ui.label(RichText::new(fmt::money(m.broker_capital_in)).monospace());
            ui.label("Lost to bankruptcies");
            ui.label(RichText::new(fmt::money(m.broker_capital_out)).monospace());
            ui.end_row();
        });
    });
    app.show_brokers = open;
}

pub fn personal(app: &mut SigmaApp, ctx: &egui::Context) {
    if !app.show_personal {
        return;
    }
    let Some(f) = app.frame.clone() else { return };
    let pal = app.pal();
    let p = &f.player;
    let mut open = app.show_personal;
    let theme = app.settings.theme;
    egui::Window::new("Personal finances").open(&mut open).default_size([640.0, 560.0]).show(ctx, |ui| {
        let pnl = p.equity - p.start_cash;
        egui::Grid::new("pf").num_columns(4).show(ui, |ui| {
            ui.label("Net worth");
            ui.label(RichText::new(fmt::money(p.equity)).monospace().strong());
            ui.label("Since start");
            ui.label(RichText::new(format!("{} ({})", fmt::money(pnl), fmt::pct(pnl / p.start_cash * 100.0))).monospace().color(theme.signed(pnl)));
            ui.end_row();
            ui.label("Cash");
            ui.label(RichText::new(fmt::money(p.cash)).monospace());
            ui.label("Invested");
            ui.label(RichText::new(fmt::money(p.invested)).monospace());
            ui.end_row();
            ui.label("Unrealised");
            ui.label(RichText::new(fmt::money(p.unrealized)).monospace().color(theme.signed(p.unrealized)));
            ui.label("Realised");
            ui.label(RichText::new(fmt::money(p.realized)).monospace().color(theme.signed(p.realized)));
            ui.end_row();
            ui.label("Dividends");
            ui.label(RichText::new(fmt::money(p.dividends)).monospace());
            ui.label("Interest on cash");
            ui.label(RichText::new(fmt::money(p.interest)).monospace());
            ui.end_row();
            ui.label("Fees paid");
            ui.label(RichText::new(fmt::money(p.fees)).monospace());
            ui.label("Cash earns");
            ui.label(RichText::new(format!("{} (policy rate)", fmt::rate(f.drivers.policy_rate))).monospace());
            ui.end_row();
        });
        ui.separator();
        ui.label(RichText::new("Allocation by sector").strong());
        let total: f64 = p.sector_value.iter().sum();
        if total > 0.0 {
            for (i, v) in p.sector_value.iter().enumerate().filter(|(_, v)| **v > 0.0) {
                ui.horizontal(|ui| {
                    ui.add_sized([90.0, 14.0], egui::Label::new(RichText::new(sigma_core::market::SECTORS[i].name()).small()));
                    let w = 200.0 * (*v / total) as f32;
                    let (r, _) = ui.allocate_exact_size(egui::vec2(200.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(egui::Rect::from_min_size(r.min, egui::vec2(w, 10.0)), 2.0, pal.accent);
                    ui.label(RichText::new(fmt::rate(v / total)).small().monospace());
                });
            }
        } else {
            ui.label(RichText::new("No positions yet.").color(pal.weak));
        }
        ui.separator();
        ui.label(RichText::new("Positions").strong());
        let mut select = None;
        egui::ScrollArea::vertical().id_salt("positions").max_height(160.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("posgrid").num_columns(8).striped(true).show(ui, |ui| {
                for h in ["Ticker", "Shares", "Avg cost", "Price", "Value", "Unrealised", "Day", ""] {
                    ui.label(RichText::new(h).small().strong());
                }
                ui.end_row();
                for pos in &p.positions {
                    if ui.link(pos.ticker.as_str()).clicked() {
                        select = Some(pos.id);
                    }
                    ui.label(RichText::new(fmt::compact(pos.shares)).monospace());
                    ui.label(RichText::new(fmt::price(pos.avg_cost)).monospace());
                    ui.label(RichText::new(fmt::price(pos.price)).monospace());
                    ui.label(RichText::new(fmt::money(pos.value)).monospace());
                    ui.label(RichText::new(fmt::money(pos.unrealized)).monospace().color(theme.signed(pos.unrealized)));
                    ui.label(RichText::new(fmt::pct(pos.change_pct)).monospace().color(theme.signed(pos.change_pct)));
                    ui.horizontal(|ui| {
                        if ui.small_button("½").on_hover_text("Sell half").clicked() {
                            app.engine.send(Command::Sell { id: pos.id, fraction: 0.5 });
                        }
                        if ui.small_button("Close").clicked() {
                            app.engine.send(Command::Sell { id: pos.id, fraction: 1.0 });
                        }
                    });
                    ui.end_row();
                }
            });
        });
        if let Some(id) = select {
            app.select(id);
        }
        if !p.orders.is_empty() {
            ui.label(RichText::new("Resting orders").strong());
            for o in &p.orders {
                ui.horizontal(|ui| {
                    let amount = if o.side == sigma_core::protocol::Side::Buy { fmt::money(o.amount) } else { format!("{:.0}% of position", o.amount * 100.0) };
                    ui.label(RichText::new(format!("#{} {:?} {:?} {} at {} ({}) · last {}", o.order_id, o.side, o.kind, o.ticker.as_str(), fmt::price(o.trigger), amount, fmt::price(o.last_price))).monospace().small());
                    if ui.small_button("Cancel").clicked() {
                        app.engine.send(Command::CancelOrder(o.order_id));
                    }
                });
            }
        }
        ui.separator();
        ui.label(RichText::new("Found a company (IPO)").strong());
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("iposector").selected_text(app.ipo.sector.name()).show_ui(ui, |ui| {
                for s in sectors() {
                    ui.selectable_value(&mut app.ipo.sector, *s, s.name());
                }
            });
            ui.label("invest");
            ui.add(egui::DragValue::new(&mut app.ipo.capital).speed(10_000.0).range(100_000.0..=1e12).prefix(fmt::CURRENCY));
            ui.label("keep");
            ui.add(egui::Slider::new(&mut app.ipo.stake, 0.05..=0.9).custom_formatter(|v, _| format!("{:.0}%", v * 100.0)));
            if ui.button("Launch").clicked() {
                app.engine.send(Command::LaunchIpo { sector: app.ipo.sector, capital: app.ipo.capital, stake: app.ipo.stake });
            }
        });
        ui.label(RichText::new(format!(
            "Lists at a market cap of {} with a random persona drawn for its sector. 1% listing fee.",
            fmt::money(app.ipo.capital / app.ipo.stake)
        ))
        .small()
        .color(pal.weak));
        ui.separator();
        ui.label(RichText::new("Recent activity").strong());
        for fill in p.fills.iter().take(10) {
            let c = sigma_core::clock::ClockParts::from_t(fill.t);
            ui.label(RichText::new(format!("d{} {:02}:{:02}  {}", c.day, c.hour, c.minute, fill.text)).monospace().small());
        }
    });
    app.show_personal = open;
}

pub fn settings(app: &mut SigmaApp, ctx: &egui::Context) {
    if !app.show_settings {
        return;
    }
    let mut open = app.show_settings;
    egui::Window::new("Settings").open(&mut open).default_width(420.0).show(ctx, |ui| {
        let s = &mut app.settings;
        egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Layout");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.layout, Layout::Desk, "Desk");
                ui.radio_value(&mut s.layout, Layout::Terminal, "Terminal");
            });
            ui.end_row();
            ui.label("Panel width");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.width, PanelWidth::Narrow, "Narrow");
                ui.radio_value(&mut s.width, PanelWidth::Normal, "Normal");
                ui.radio_value(&mut s.width, PanelWidth::Wide, "Wide");
            });
            ui.end_row();
            ui.label("Theme");
            ui.horizontal(|ui| {
                ui.radio_value(&mut s.theme, Theme::Dark, "Dark");
                ui.radio_value(&mut s.theme, Theme::Light, "Light");
            });
            ui.end_row();

            ui.label("Speed");
            let r = ui.add(egui::Slider::new(&mut s.speed, 0.25..=1024.0).logarithmic(true).suffix("x"));
            ui.end_row();
            ui.label("");
            let real_min = 90.0 / s.speed;
            ui.label(RichText::new(format!("1 sim day = {} real", human_minutes(real_min))).small());
            ui.end_row();
            if r.changed() {
                app.engine.send(Command::SetSpeed(s.speed));
            }

            ui.label("Trading session");
            let before = s.session;
            egui::ComboBox::from_id_salt("session").selected_text(s.session.label()).show_ui(ui, |ui| {
                for m in SessionMode::ALL {
                    ui.selectable_value(&mut s.session, m, m.label());
                }
            });
            if s.session != before {
                app.engine.send(Command::SetSession(s.session));
            }
            ui.end_row();

            ui.label("Engine budget");
            if ui.add(egui::Slider::new(&mut s.budget_ms, 0.1..=8.0).suffix(" ms/tick")).changed() {
                app.engine.send(Command::SetBudgetMs(s.budget_ms));
            }
            ui.end_row();

            ui.label("Crawl speed");
            ui.add(egui::Slider::new(&mut s.crawl_speed, 10.0..=200.0).suffix(" px/s"));
            ui.end_row();

            ui.label("Console log");
            let before = s.log_level;
            egui::ComboBox::from_id_salt("loglevel").selected_text(s.log_level.name()).show_ui(ui, |ui| {
                for l in Level::ALL {
                    ui.selectable_value(&mut s.log_level, l, l.name());
                }
            });
            if s.log_level != before {
                app.engine.send(Command::SetLogLevel(s.log_level));
            }
            ui.end_row();

            ui.label("Seed");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut s.seed_text).desired_width(120.0));
                if ui.button("Reset market").clicked() {
                    let seed = s.seed_text.trim().parse::<u64>().unwrap_or_else(|_| hash(&s.seed_text));
                    app.engine.send(Command::Reset { seed });
                    app.selected = None;
                }
            });
            ui.end_row();
        });
        ui.separator();
        ui.label(
            RichText::new("Console output goes to the terminal that launched the app (stderr). Set SIGMA_LOG=debug before launch for more detail.")
                .small(),
        );
    });
    app.show_settings = open;
}

fn human_minutes(m: f64) -> String {
    if m >= 60.0 { format!("{:.1} h", m / 60.0) } else if m >= 1.0 { format!("{m:.1} min") } else { format!("{:.0} s", m * 60.0) }
}

/// FNV-1a, so any text works as a seed.
fn hash(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}
