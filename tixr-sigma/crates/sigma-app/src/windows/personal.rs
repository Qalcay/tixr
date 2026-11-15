//! Personal finance pop-up: balance sheet, positions, order history, and
//! launching your own company or fund.

use eframe::egui::{self, RichText};
use sigma_engine::exchange::EXCHANGE_SPECS;
use sigma_engine::persona::{ALL_PERKS, MAX_PERKS, Persona};
use sigma_engine::protocol::{Command, LaunchRequest, Side};
use sigma_engine::time::clock_string;

use crate::app::App;
use crate::format;

pub struct PersonalUi {
    pub capital: f64,
    pub exchange: u8,
    pub fund: bool,
    pub ticker: String,
    pub random_persona: bool,
    pub persona: Persona,
    pub tab: Tab,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Positions,
    History,
    Launch,
}

impl Default for PersonalUi {
    fn default() -> Self {
        Self {
            capital: 250_000.0,
            exchange: 0,
            fund: false,
            ticker: String::new(),
            random_persona: true,
            persona: Persona::EMPTY,
            tab: Tab::Positions,
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.windows.personal {
        return;
    }
    let mut open = true;
    let pal = app.settings.theme.palette();
    let mut commands: Vec<Command> = Vec::new();
    let mut select = None;
    egui::Window::new("Personal finance")
        .open(&mut open)
        .default_size([620.0, 480.0])
        .resizable(true)
        .show(ctx, |ui| {
            let Some(snap) = &app.snap else { return };
            let p = &snap.player;
            let epoch_days = snap.epoch_days as i64;
            let st = &mut app.personal_ui;
            let total = p.equity - p.starting_cash;
            egui::Grid::new("pf_summary").num_columns(4).spacing([16.0, 2.0]).show(ui, |ui| {
                ui.label("Cash");
                ui.monospace(format::money(p.cash));
                ui.label("Net worth");
                ui.monospace(format::money(p.equity));
                ui.end_row();
                ui.label("Invested");
                ui.monospace(format::money(p.equity - p.cash));
                ui.label("Total P&L");
                ui.colored_label(
                    if total >= 0.0 { pal.up } else { pal.down },
                    RichText::new(format!("{} ({})", format::money(total), format::pct(total / p.starting_cash * 100.0))).monospace(),
                );
                ui.end_row();
                ui.label("Realised");
                ui.monospace(format::money(p.realized_pnl));
                ui.label("Unrealised");
                ui.monospace(format::money(p.unrealized_pnl));
                ui.end_row();
                ui.label("Interest earned");
                ui.monospace(format::money(p.interest_earned));
                ui.label("Fees paid");
                ui.monospace(format::money(p.fees_paid));
                ui.end_row();
            });
            ui.label(
                RichText::new(format!("Idle cash earns the policy rate ({}). Trades cost 5 bps plus price impact.", format::rate(snap.macro_view.policy_rate)))
                    .small()
                    .color(pal.text_weak),
            );
            ui.separator();
            ui.horizontal(|ui| {
                ui.selectable_value(&mut st.tab, Tab::Positions, format!("Positions ({})", p.holdings.len()));
                ui.selectable_value(&mut st.tab, Tab::History, "Order history");
                ui.selectable_value(&mut st.tab, Tab::Launch, "Launch a listing");
            });
            ui.separator();
            match st.tab {
                Tab::Positions => {
                    if p.holdings.is_empty() {
                        ui.label("No positions yet. Pick a listing and use BUY.");
                    }
                    egui::ScrollArea::vertical().id_salt("pf_pos").show(ui, |ui| {
                        egui::Grid::new("pf_pos_grid").striped(true).num_columns(8).spacing([10.0, 2.0]).show(ui, |ui| {
                            for h in ["Ticker", "Shares", "Avg cost", "Price", "Value", "P&L", "Weight", ""] {
                                ui.label(RichText::new(h).strong());
                            }
                            ui.end_row();
                            for h in &p.holdings {
                                if ui.link(h.ticker.as_str()).clicked() {
                                    select = Some(h.id);
                                }
                                ui.monospace(format::shares(h.shares));
                                ui.monospace(format::price(h.avg_cost));
                                ui.monospace(format::price(h.price));
                                ui.monospace(format::money(h.value));
                                ui.colored_label(if h.unrealized >= 0.0 { pal.up } else { pal.down }, RichText::new(format::money(h.unrealized)).monospace());
                                ui.monospace(format!("{:.1}%", h.value / p.equity.max(1.0) * 100.0));
                                if ui.small_button("Sell all").clicked() {
                                    commands.push(Command::Liquidate(h.id));
                                }
                                ui.end_row();
                            }
                        });
                    });
                }
                Tab::History => {
                    egui::ScrollArea::vertical().id_salt("pf_hist").show(ui, |ui| {
                        egui::Grid::new("pf_hist_grid").striped(true).num_columns(7).spacing([10.0, 2.0]).show(ui, |ui| {
                            for h in ["Time", "Side", "Ticker", "Shares", "Price", "Fee", "Realised"] {
                                ui.label(RichText::new(h).strong());
                            }
                            ui.end_row();
                            for f in &p.fills {
                                ui.monospace(format!("d{} {}", sigma_engine::time::day_index(f.time) as i64 - epoch_days + 1, clock_string(f.time)));
                                let (s, c) = if f.side == Side::Buy { ("BUY", pal.up) } else { ("SELL", pal.down) };
                                ui.colored_label(c, s);
                                ui.monospace(f.ticker.as_str());
                                ui.monospace(format::shares(f.shares));
                                ui.monospace(format::price(f.price));
                                ui.monospace(format::money(f.fee));
                                ui.monospace(if f.side == Side::Sell { format::money(f.realized) } else { String::new() });
                                ui.end_row();
                            }
                        });
                    });
                }
                Tab::Launch => {
                    ui.label("Found a company (or a fund) and list it. You put up the capital and keep a 40% founder stake; a 1% listing fee applies.");
                    egui::Grid::new("launch").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                        ui.label("Capital");
                        ui.add(egui::DragValue::new(&mut st.capital).range(50_000.0..=1e12).speed(1_000.0).prefix("$"));
                        ui.end_row();
                        ui.label("Exchange");
                        egui::ComboBox::from_id_salt("launch_ex")
                            .selected_text(EXCHANGE_SPECS[st.exchange as usize].name)
                            .show_ui(ui, |ui| {
                                for (i, e) in EXCHANGE_SPECS.iter().enumerate() {
                                    ui.selectable_value(&mut st.exchange, i as u8, e.name);
                                }
                            });
                        ui.end_row();
                        ui.label("Kind");
                        ui.horizontal(|ui| {
                            ui.selectable_value(&mut st.fund, false, "Stock");
                            ui.selectable_value(&mut st.fund, true, "Index fund");
                        });
                        ui.end_row();
                        ui.label("Ticker");
                        ui.add(egui::TextEdit::singleline(&mut st.ticker).char_limit(5).desired_width(70.0).hint_text("random"));
                        ui.end_row();
                        ui.label("Persona");
                        ui.checkbox(&mut st.random_persona, "random");
                        ui.end_row();
                    });
                    if !st.random_persona && !st.fund {
                        ui.horizontal_wrapped(|ui| {
                            for perk in ALL_PERKS {
                                let mut on = st.persona.has(perk);
                                let full = st.persona.len() >= MAX_PERKS && !on;
                                let r = ui
                                    .add_enabled(!full, egui::Button::new(perk.letter().to_string()).selected(on))
                                    .on_hover_text(format!("{}: {}", perk.name(), perk.blurb()));
                                if r.clicked() {
                                    on = !on;
                                    st.persona = if on { st.persona.with(perk) } else { st.persona.without(perk) };
                                }
                            }
                        });
                        ui.label(RichText::new(format!("{} of {MAX_PERKS} perks: {}", st.persona.len(), st.persona.letters())).small());
                    }
                    let cost = st.capital * 1.01;
                    let can = cost <= p.cash;
                    if ui.add_enabled(can, egui::Button::new(format!("Launch for {}", format::money(cost)))).clicked() {
                        commands.push(Command::Launch(LaunchRequest {
                            capital: st.capital,
                            exchange: st.exchange,
                            persona: if st.random_persona || st.fund { None } else { Some(st.persona) },
                            ticker: Some(st.ticker.clone()).filter(|t| !t.trim().is_empty()),
                            fund: st.fund,
                        }));
                    }
                    if !p.companies.is_empty() {
                        ui.separator();
                        ui.label("Your listings:");
                        ui.horizontal_wrapped(|ui| {
                            for (id, t) in &p.companies {
                                if ui.link(t.as_str()).clicked() {
                                    select = Some(*id);
                                }
                            }
                        });
                    }
                }
            }
        });
    for c in commands {
        app.send(c);
    }
    if let Some(id) = select {
        app.select(Some(id));
    }
    app.windows.personal = open;
}
