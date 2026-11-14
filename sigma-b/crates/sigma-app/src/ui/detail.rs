//! Centre panel: the selected security, its persona, chart, fundamentals,
//! model forecast and order entry.

use crate::app::SigmaApp;
use crate::fmt;
use crate::ui::chart::{ChartOpts, Series, line_chart};
use eframe::egui::{self, RichText};
use sigma_core::market::EXCHANGE_NAMES;
use sigma_core::persona::Perk;
use sigma_core::protocol::{Command, Detail, OrderKind, Side};

pub fn show(app: &mut SigmaApp, ui: &mut egui::Ui) {
    let Some(d) = app.frame.as_ref().and_then(|f| f.selected.clone()) else {
        overview(app, ui);
        return;
    };
    let pal = app.pal();
    egui::ScrollArea::vertical().id_salt("detail").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(d.ticker.as_str()).heading().strong().color(pal.accent));
            ui.label(RichText::new(fmt::price(d.price)).heading().monospace());
            let ch = (d.price / d.prev_close - 1.0) * 100.0;
            ui.label(RichText::new(fmt::pct(ch)).monospace().color(app.settings.theme.signed(ch)));
            ui.separator();
            ui.label(format!("{} · {} · {}", EXCHANGE_NAMES[d.exchange as usize], d.sector.name(), d.issuer));
        });
        persona_chips(app, ui, &d);
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.selectable_value(&mut app.chart_daily, false, "Intraday");
            ui.selectable_value(&mut app.chart_daily, true, "Daily + 1Y forecast");
            ui.label(RichText::new(format!("quote age {:.1} min · {} updates", d.age_min, d.updates)).small().color(pal.weak));
        });
        let h = (ui.available_height() * 0.45).clamp(140.0, 340.0);
        if app.chart_daily {
            let mut series: Vec<f32> = d.history.clone();
            series.push(d.price as f32);
            let (c, lo, hi) = d.forecast;
            line_chart(
                ui,
                &pal,
                &[Series::new(&series, pal.accent)],
                ChartOpts { height: h, fill_first: true, forecast: Some((c as f32, lo as f32, hi as f32)), ..Default::default() },
            );
        } else {
            let pts = if d.intraday.is_empty() { vec![d.price as f32] } else { d.intraday.clone() };
            line_chart(ui, &pal, &[Series::new(&pts, pal.accent)], ChartOpts { height: h, fill_first: true, ..Default::default() });
        }
        ui.add_space(6.0);

        ui.columns(3, |cols| {
            egui::Grid::new("fund1").num_columns(2).striped(true).show(&mut cols[0], |ui| {
                kv(ui, "Model value", fmt::price(d.fair));
                kv(ui, "Value gap", fmt::pct(d.mispricing * 100.0));
                kv(ui, "P/E", if d.pe.is_finite() { format!("{:.1}", d.pe) } else { "book".into() });
                kv(ui, "Dividend yield", fmt::rate(d.div_yield));
                kv(ui, "Day range", format!("{} - {}", fmt::price(d.day_low), fmt::price(d.day_high)));
            });
            egui::Grid::new("fund2").num_columns(2).striped(true).show(&mut cols[1], |ui| {
                kv(ui, "Beta", format!("{:.2}", d.beta));
                kv(ui, "Volatility", fmt::rate(d.vol));
                kv(ui, "Cost of equity", fmt::rate(d.cost_of_equity));
                kv(ui, "Excess growth", fmt::rate(d.growth));
                kv(ui, "Leverage", fmt::rate(d.leverage));
            });
            egui::Grid::new("fund3").num_columns(2).striped(true).show(&mut cols[2], |ui| {
                kv(ui, "Market cap", fmt::money(d.market_cap));
                kv(ui, "Shares", fmt::compact(d.shares));
                kv(
                    ui,
                    "Dist. to default",
                    if d.distance_to_default.is_finite() { format!("{:.1} sd", d.distance_to_default) } else { "no debt".into() },
                );
                kv(ui, "Listed", format!("{:.0} days", d.listed_days));
                kv(ui, "Splits", if d.split_factor > 1.0 { format!("x{:.0}", d.split_factor) } else { "none".into() });
            });
        });
        let (c, lo, hi) = d.forecast;
        ui.label(
            RichText::new(format!(
                "1Y model forecast: {}  (90% range {} - {}), implied return {}",
                fmt::price(c),
                fmt::price(lo),
                fmt::price(hi),
                fmt::pct((c / d.price - 1.0) * 100.0)
            ))
            .color(pal.weak),
        );
        ui.separator();
        order_entry(app, ui, &d);
    });
}

fn kv(ui: &mut egui::Ui, k: &str, v: String) {
    ui.label(RichText::new(k).small());
    ui.label(RichText::new(v).monospace());
    ui.end_row();
}

fn persona_chips(app: &SigmaApp, ui: &mut egui::Ui, d: &Detail) {
    let pal = app.pal();
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Persona ({}):", d.perks.len())).small().color(pal.weak));
        if d.perks.is_empty() {
            ui.label(RichText::new("plain, no perks").small().color(pal.weak));
        }
        for p in d.perks.iter() {
            chip(ui, p, pal.chip, pal.text);
        }
    });
}

fn chip(ui: &mut egui::Ui, p: Perk, bg: egui::Color32, fg: egui::Color32) {
    egui::Frame::new().fill(bg).corner_radius(3.0).inner_margin(egui::Margin::symmetric(5, 1)).show(ui, |ui| {
        ui.label(RichText::new(format!("{} {}", p.letter(), p.name())).small().color(fg));
    })
    .response
    .on_hover_text(p.blurb());
}

fn order_entry(app: &mut SigmaApp, ui: &mut egui::Ui, d: &Detail) {
    let pal = app.pal();
    let open = app.frame.as_ref().is_some_and(|f| f.clock.open);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Trade").strong());
        ui.add(egui::DragValue::new(&mut app.order.notional).speed(1000.0).range(100.0..=1e12).prefix(fmt::CURRENCY));
        if ui.add_enabled(open, egui::Button::new("Buy")).clicked() {
            app.send(Command::Buy { id: d.id, notional: app.order.notional });
        }
        ui.add_enabled_ui(open && d.held_shares > 0.0, |ui| {
            for (label, frac) in [("Sell 25%", 0.25), ("Sell 50%", 0.5), ("Close", 1.0)] {
                if ui.button(label).clicked() {
                    app.send(Command::Sell { id: d.id, fraction: frac });
                }
            }
        });
        if !open {
            ui.label(RichText::new("market closed").color(pal.down));
        }
        if d.held_shares > 0.0 {
            ui.label(RichText::new(format!("you hold {}", fmt::compact(d.held_shares))).color(pal.weak));
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resting order").strong());
        egui::ComboBox::from_id_salt("oside").width(56.0).selected_text(format!("{:?}", app.order.side)).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.order.side, Side::Buy, "Buy");
            ui.selectable_value(&mut app.order.side, Side::Sell, "Sell");
        });
        egui::ComboBox::from_id_salt("okind").width(60.0).selected_text(format!("{:?}", app.order.kind)).show_ui(ui, |ui| {
            ui.selectable_value(&mut app.order.kind, OrderKind::Limit, "Limit");
            ui.selectable_value(&mut app.order.kind, OrderKind::Stop, "Stop");
        });
        if app.order.trigger <= 0.0 {
            app.order.trigger = (d.price * 100.0).round() / 100.0;
        }
        ui.label("at");
        ui.add(egui::DragValue::new(&mut app.order.trigger).speed(d.price * 0.002).range(0.01..=1e9));
        let amount_label = if app.order.side == Side::Buy { "spend" } else { "fraction" };
        ui.label(amount_label);
        if app.order.side == Side::Sell {
            app.order.amount = app.order.amount.clamp(0.01, 1.0);
            ui.add(egui::DragValue::new(&mut app.order.amount).speed(0.01).range(0.01..=1.0));
        } else {
            if app.order.amount < 100.0 {
                app.order.amount = app.order.notional;
            }
            ui.add(egui::DragValue::new(&mut app.order.amount).speed(1000.0).range(100.0..=1e12).prefix(fmt::CURRENCY));
        }
        if ui.button("Place").clicked() {
            app.send(Command::PlaceOrder { id: d.id, side: app.order.side, kind: app.order.kind, trigger: app.order.trigger, amount: app.order.amount });
        }
    });
    ui.label(RichText::new("Limit buys fill at or below the trigger, stops sell at or below it. Orders are checked whenever the stock is sampled, and stocks with orders are sampled more often.").small().color(pal.weak));
}

/// Shown when nothing is selected.
fn overview(app: &mut SigmaApp, ui: &mut egui::Ui) {
    let pal = app.pal();
    let Some(f) = app.frame.as_ref() else {
        ui.centered_and_justified(|ui| ui.label("Starting the engine (warming up a few simulated days)..."));
        return;
    };
    ui.heading("Market overview");
    ui.label(RichText::new("Pick a stock from the list or the crawl.").color(pal.weak));
    let live = f.macro_history.live.clone();
    let comp = f.indices.first().cloned().unwrap_or_default();
    ui.label(format!("Composite {:.1} ({}) · {} members", comp.level, fmt::pct(comp.change_pct), comp.members));
    let h = (ui.available_height() * 0.6).clamp(140.0, 420.0);
    let hist: Vec<f32> = if comp.history.len() > 4 { comp.history.clone() } else { live };
    line_chart(ui, &pal, &[Series::new(&hist, pal.accent)], ChartOpts { height: h, fill_first: true, ..Default::default() });
}
