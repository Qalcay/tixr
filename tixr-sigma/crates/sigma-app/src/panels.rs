//! The always-visible panels: security list, detail view, drivers dock.

use eframe::egui::{self, Align2, FontId, RichText, Sense, Ui, pos2, vec2};
use sigma_engine::exchange::EXCHANGE_SPECS;
use sigma_engine::protocol::{Command, Side};
use sigma_engine::security::SecurityKind;
use sigma_engine::time::clock_string;

use crate::app::{App, SortKey};
use crate::chart::{self, ALL_RANGES};
use crate::format;

const ROW_H: f32 = 18.0;

/// Virtualised list: only visible rows are laid out, so 30,000 listings cost
/// the same per frame as 30. Rows are painted at fixed column offsets.
pub fn security_list(app: &mut App, ui: &mut Ui) {
    // Take the snapshot out while drawing so `app` can be mutated freely;
    // it is a `Box`, so this is a pointer move, not a copy.
    let Some(snap) = app.snap.take() else { return };
    security_list_inner(app, ui, &snap);
    app.snap = Some(snap);
}

fn security_list_inner(app: &mut App, ui: &mut Ui, snap: &sigma_engine::Snapshot) {
    let pal = app.settings.theme.palette();
    ui.horizontal(|ui| {
        ui.label("Find");
        let r = ui.add(egui::TextEdit::singleline(&mut app.search).desired_width(110.0).hint_text("ticker / perks"));
        if r.changed() {
            app.mark_view_dirty();
        }
        let current = app.exchange_filter.map_or("All", |e| EXCHANGE_SPECS[e as usize].name);
        egui::ComboBox::from_id_salt("exfilter").width(64.0).selected_text(current).show_ui(ui, |ui| {
            let mut f = app.exchange_filter;
            ui.selectable_value(&mut f, None, "All");
            for (i, s) in EXCHANGE_SPECS.iter().enumerate() {
                ui.selectable_value(&mut f, Some(i as u8), s.name);
            }
            if f != app.exchange_filter {
                app.exchange_filter = f;
                app.mark_view_dirty();
            }
        });
    });
    ui.label(
        RichText::new(format!("{} shown / {} listed / cap 65,536", app.view.len(), snap.rows.len()))
            .small()
            .color(pal.text_weak),
    );
    let cols = [
        ("Ticker", SortKey::Ticker, 0.0),
        ("Price", SortKey::Price, 60.0),
        ("Chg", SortKey::Change, 132.0),
        ("Cap", SortKey::Cap, 196.0),
    ];
    let (hdr, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
    for (name, key, x) in cols {
        let r = egui::Rect::from_min_size(pos2(hdr.left() + x, hdr.top()), vec2(56.0, ROW_H));
        let arrow = if app.sort == key { if app.sort_desc { " v" } else { " ^" } } else { "" };
        if ui.put(r, egui::Button::new(RichText::new(format!("{name}{arrow}")).small()).frame(false)).clicked() {
            if app.sort == key {
                app.sort_desc = !app.sort_desc;
            } else {
                app.sort = key;
                app.sort_desc = key != SortKey::Ticker;
            }
            app.mark_view_dirty();
        }
    }
    ui.separator();

    let mut clicked = None;
    let selected = app.selected;
    let view = &app.view;
    let rows = &snap.rows;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, view.len(), |ui, range| {
        let font = FontId::monospace(12.0);
        for &i in &view[range] {
            let r = &rows[i];
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
            let p = ui.painter_at(rect);
            if selected == Some(r.id) {
                p.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
            } else if resp.hovered() {
                p.rect_filled(rect, 2.0, ui.visuals().faint_bg_color);
            }
            let y = rect.center().y;
            let name_color = if r.kind == SecurityKind::Fund { pal.accent } else { pal.text };
            p.text(pos2(rect.left() + 2.0, y), Align2::LEFT_CENTER, r.ticker.as_str(), font.clone(), name_color);
            p.text(pos2(rect.left() + 124.0, y), Align2::RIGHT_CENTER, format::price(r.price), font.clone(), pal.text);
            let col = if r.change_pct >= 0.0 { pal.up } else { pal.down };
            p.text(
                pos2(rect.left() + 190.0, y),
                Align2::RIGHT_CENTER,
                format::pct(r.change_pct as f64),
                font.clone(),
                col,
            );
            p.text(
                pos2(rect.right() - 4.0, y),
                Align2::RIGHT_CENTER,
                format::compact(r.market_cap, 1),
                font.clone(),
                pal.text_weak,
            );
            if resp.clicked() {
                clicked = Some(r.id);
            }
        }
    });
    if let Some(id) = clicked {
        app.select(Some(id));
    }
}

fn persona_label(ui: &mut Ui, persona: sigma_engine::persona::Persona) {
    let text = format!("persona {}", persona.letters());
    let r = ui.label(RichText::new(text).monospace());
    if !persona.is_empty() {
        r.on_hover_ui(|ui| {
            for p in persona.perks() {
                ui.label(format!("{}  {:<11} {}", p.letter(), p.name(), p.blurb()));
            }
        });
    } else {
        r.on_hover_text("No perks: a plain-vanilla listing.");
    }
}

/// `reserve` keeps that many pixels free below the detail view (the
/// Terminal layout puts the market feed there).
pub fn detail(app: &mut App, ui: &mut Ui, reserve: f32) {
    let Some(snap) = app.snap.take() else {
        ui.centered_and_justified(|ui| ui.label("Starting engine..."));
        return;
    };
    detail_inner(app, ui, &snap, reserve);
    app.snap = Some(snap);
}

fn detail_inner(app: &mut App, ui: &mut Ui, snap: &sigma_engine::Snapshot, reserve: f32) {
    let pal = app.settings.theme.palette();
    let Some(d) = &snap.detail else {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("Pick a security from the list");
            ui.label(format!(
                "{} listings across {} exchanges. Prices update only when a listing's own random clock fires,",
                snap.rows.len(),
                snap.exchanges.len()
            ));
            ui.label("when someone trades it, or at its exchange's open and close.");
        });
        return;
    };
    let change = (d.price / d.prev_close - 1.0) * 100.0;
    ui.horizontal(|ui| {
        ui.heading(RichText::new(d.ticker.as_str()).strong());
        let kind = if d.kind == SecurityKind::Fund { "Fund" } else { "Stock" };
        let listed =
            if d.listed_day <= 0 { "listed before day 1".to_string() } else { format!("listed day {}", d.listed_day) };
        ui.label(format!("{kind} · {} · {listed} · {}", EXCHANGE_SPECS[d.exchange as usize].name, d.sponsor));
        persona_label(ui, d.persona);
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new(format::price(d.price)).size(20.0).monospace());
        ui.colored_label(if change >= 0.0 { pal.up } else { pal.down }, RichText::new(format::pct(change)).monospace());
        ui.label(
            RichText::new(format!(
                "O {}  H {}  L {}  vol {}",
                format::price(d.day_open),
                format::price(d.day_high),
                format::price(d.day_low),
                format::money(d.volume_today)
            ))
            .monospace()
            .color(pal.text_weak),
        );
    });
    ui.horizontal_wrapped(|ui| {
        let stat = |ui: &mut Ui, k: &str, v: String, tip: &str| {
            ui.label(RichText::new(format!("{k} {v}")).monospace()).on_hover_text(tip);
        };
        stat(ui, "fair", format::price(d.fair_value), "Model fair value: price with the mispricing term removed.");
        stat(
            ui,
            "misp",
            format!("{:+.1}%", d.mispricing * 100.0),
            "ln(price / fair). Pulled back toward 0 over the half-life.",
        );
        stat(ui, "β", format!("{:.2}", d.beta), "Sensitivity to its exchange's market factor.");
        stat(ui, "dur", format!("{:.1}", d.duration), "Rate sensitivity: % move for a 1pt drop in the 10Y yield.");
        stat(ui, "vol", format!("{:.0}%", d.annual_vol * 100.0), "Annualised volatility in the current regime.");
        stat(ui, "t½", format!("{:.0}d", d.half_life_days), "Half-life of mispricing (mean reversion speed).");
        stat(ui, "DtD", format!("{:.2}", d.distance_to_default), "Distance to default in log units; bankrupt below 0.");
        stat(ui, "cap", format::money(d.market_cap), "Market capitalisation.");
    });
    ui.horizontal(|ui| {
        for r in ALL_RANGES {
            if ui.selectable_label(app.chart_range == r, r.label()).clicked() {
                app.chart_range = r;
            }
        }
        ui.checkbox(&mut app.settings.show_forecast, "forecast cone");
        ui.label(RichText::new("dashed: fair value / mean forecast; bands: 68% and 95%").small().color(pal.text_weak));
    });
    let chart_h = (ui.available_height() - 64.0 - reserve).max(120.0);
    chart::price_chart(ui, vec2(ui.available_width(), chart_h), d, app.chart_range, app.settings.show_forecast, &pal);

    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut app.ticket.amount)
                .range(0.0..=1e13)
                .speed(100.0)
                .max_decimals(2)
                .prefix(if app.ticket.by_notional { "$" } else { "" })
                .suffix(if app.ticket.by_notional { "" } else { " sh" }),
        );
        ui.selectable_value(&mut app.ticket.by_notional, true, "$");
        ui.selectable_value(&mut app.ticket.by_notional, false, "shares");
        let open = snap.exchanges[d.exchange as usize].is_open;
        ui.add_enabled_ui(open, |ui| {
            if ui.button(RichText::new(" BUY ").color(pal.up)).clicked() {
                app.order(Side::Buy);
            }
            if ui.button(RichText::new(" SELL ").color(pal.down)).clicked() {
                app.order(Side::Sell);
            }
            if d.player_shares > 0.0 && ui.button("Sell all").clicked() {
                app.send(Command::Liquidate(d.id));
            }
        });
        if !open {
            let (o, c) = snap.exchanges[d.exchange as usize].session;
            ui.colored_label(
                pal.down,
                format!(
                    "{} closed (opens {:02.0}:{:02.0}, closes {:02.0}:{:02.0})",
                    EXCHANGE_SPECS[d.exchange as usize].name,
                    o.floor(),
                    o.fract() * 60.0,
                    c.floor(),
                    c.fract() * 60.0
                ),
            );
        }
    });
    ui.horizontal(|ui| {
        if d.player_shares > 0.0 {
            let value = d.player_shares * d.price;
            let pnl = value - d.player_shares * d.player_avg_cost;
            ui.monospace(format!(
                "You hold {} sh @ {} = {}",
                format::shares(d.player_shares),
                format::price(d.player_avg_cost),
                format::money(value)
            ));
            ui.colored_label(if pnl >= 0.0 { pal.up } else { pal.down }, RichText::new(format::money(pnl)).monospace());
        } else {
            ui.label(
                RichText::new(format!("Cash {}", format::money(snap.player.cash))).monospace().color(pal.text_weak),
            );
        }
    });
}

fn kv(ui: &mut Ui, k: &str, v: String, tip: &str) {
    ui.label(k).on_hover_text(tip);
    ui.label(RichText::new(v).monospace());
    ui.end_row();
}

pub fn drivers(app: &App, ui: &mut Ui) {
    let Some(snap) = &app.snap else { return };
    let pal = app.settings.theme.palette();
    let m = &snap.macro_view;
    ui.strong("MARKET DRIVERS");
    egui::Grid::new("drivers").num_columns(2).spacing([8.0, 0.0]).show(ui, |ui| {
        let regime_col = match snap.regime {
            sigma_engine::macroecon::Regime::Crisis => pal.down,
            sigma_engine::macroecon::Regime::Correction => pal.down,
            _ => pal.up,
        };
        ui.label("Regime");
        ui.colored_label(
            regime_col,
            RichText::new(format!("{} ({}d)", snap.regime.label(), snap.regime_days)).monospace(),
        );
        ui.end_row();
        kv(
            ui,
            "Stress",
            format!("{:.2}", m.stress),
            "Composite of slack, curve inversion, inflation and credit stress. Raises crash odds.",
        );
        kv(
            ui,
            "Policy rate",
            format::rate(m.policy_rate),
            "Central bank rate; follows the Taylor rule with smoothing.",
        );
        kv(ui, "Taylor target", format::rate(m.taylor_target), "r* + pi + 0.5(pi - 2%) + 0.5 gap");
        kv(ui, "Inflation", format::rate(m.inflation), "Annual inflation; target 2%.");
        kv(ui, "Output gap", format!("{:+.2}%", m.output_gap * 100.0), "Economy vs potential. Negative = slack.");
        let inv = if m.spread_10y_3m < 0.0 { " INVERTED" } else { "" };
        kv(
            ui,
            "10Y-3M",
            format!("{:+.2}%{inv}", m.spread_10y_3m * 100.0),
            "Curve slope. Inversion predicts downturns.",
        );
        kv(ui, "Equity prem.", format::rate(m.erp), "Excess return demanded on stocks over the 10Y.");
        kv(
            ui,
            "Credit spread",
            format::rate(m.credit_spread),
            "Corporate borrowing premium. Junk/cyclical names load on it.",
        );
        kv(ui, "Fair P/E", format!("{:.1}", m.fair_pe), "Gordon model: payout / (10Y + ERP - growth).");
        kv(ui, "Money supply", format::money(m.money_supply), "Grows with nominal GDP, faster when policy is loose.");
    });
}

pub fn population(app: &App, ui: &mut Ui) {
    let Some(snap) = &app.snap else { return };
    let p = &snap.population;
    ui.strong("POPULATION");
    egui::Grid::new("pop").num_columns(2).spacing([8.0, 1.0]).show(ui, |ui| {
        kv(
            ui,
            "Live / K",
            format!("{} / {:.0}", p.live, p.carrying_capacity),
            "Listings vs the carrying capacity the economy supports (8,192..32,768 band, hard cap 65,536).",
        );
        kv(ui, "Stocks/funds", format!("{} / {}", p.stocks, p.funds), "");
        kv(ui, "IPOs", format!("{}", p.ipos), "New listings since start (engine, firms and you).");
        kv(ui, "Bankrupt", format!("{}", p.bankruptcies), "Fair value fell through the default barrier.");
        kv(ui, "Acquired", format!("{}", p.acquisitions), "Bought out at a 30% premium; more likely when crowded.");
        kv(
            ui,
            "Penny/fund",
            format!("{} / {}", p.penny_delistings, p.fund_closures),
            "Delisted below $1 for 20 sessions / funds wound up.",
        );
        kv(ui, "Brokers", format!("{} alive, {} broke", p.brokers_alive, p.brokers_broke), "");
        kv(
            ui,
            "Engine",
            format!("{} ev/s, d{:.2}", format::compact(snap.events_per_real_sec, 0), snap.tick_density),
            "Events processed per real second, and the print-rate multiplier the engine chose to stay within budget.",
        );
    });
}

pub fn exchanges(app: &App, ui: &mut Ui) {
    let Some(snap) = &app.snap else { return };
    let pal = app.settings.theme.palette();
    ui.strong("EXCHANGES");
    egui::Grid::new("ex").num_columns(4).spacing([8.0, 1.0]).show(ui, |ui| {
        for e in &snap.exchanges {
            ui.colored_label(if e.is_open { pal.up } else { pal.text_weak }, e.name).on_hover_text(format!(
                "Session {:02.0}:{:02.0}-{:02.0}:{:02.0}, Mon-Fri. {} listings.",
                e.session.0.floor(),
                e.session.0.fract() * 60.0,
                e.session.1.floor(),
                e.session.1.fract() * 60.0,
                e.members
            ));
            ui.label(RichText::new(format::price(e.index)).monospace());
            ui.colored_label(
                if e.change_pct >= 0.0 { pal.up } else { pal.down },
                RichText::new(format::pct(e.change_pct)).monospace(),
            );
            ui.label(RichText::new(format!("P/E {:.1}", e.pe)).monospace());
            ui.end_row();
        }
    });
}

pub fn personal_summary(app: &mut App, ui: &mut Ui) {
    let Some(snap) = &app.snap else { return };
    let pal = app.settings.theme.palette();
    let p = &snap.player;
    ui.strong("PERSONAL");
    let pnl = p.equity - p.starting_cash;
    egui::Grid::new("me").num_columns(2).spacing([8.0, 1.0]).show(ui, |ui| {
        kv(ui, "Cash", format::money(p.cash), "Earns the policy rate.");
        kv(ui, "Net worth", format::money(p.equity), "Cash plus holdings at market.");
        ui.label("Total P&L");
        ui.colored_label(if pnl >= 0.0 { pal.up } else { pal.down }, RichText::new(format::money(pnl)).monospace());
        ui.end_row();
        kv(ui, "Positions", format!("{}", p.holdings.len()), "");
    });
    if ui.button("Open personal finance").clicked() {
        app.windows.personal = true;
    }
}

pub fn feed(app: &App, ui: &mut Ui, height: f32) {
    let Some(snap) = &app.snap else { return };
    let pal = app.settings.theme.palette();
    ui.strong("MARKET FEED");
    egui::ScrollArea::vertical().id_salt("feed").max_height(height).auto_shrink([false, true]).show(ui, |ui| {
        for f in &snap.feed {
            ui.horizontal(|ui| {
                ui.label(RichText::new(clock_string(f.time)).monospace().small().color(pal.text_weak));
                ui.add(egui::Label::new(RichText::new(&f.text).small()).truncate());
            });
        }
    });
}

/// Classic layout: one row of fixed-width columns along the bottom.
///
/// Each column gets an explicit rect (`UiBuilder::max_rect`) and its own
/// scroll area, so a column with more rows than fit scrolls inside its box
/// instead of being clipped or stretching the panel.
pub fn dock(app: &mut App, ui: &mut Ui) {
    let pal = app.settings.theme.palette();
    let full = ui.available_rect_before_wrap();
    let widths = [258.0, 236.0, 236.0, 0.0, 210.0];
    let fixed: f32 = widths.iter().sum::<f32>() + 8.0 * 5.0;
    let feed_w = (full.width() - fixed).max(140.0);
    let mut x = full.left();
    for (i, &w) in widths.iter().enumerate() {
        let w = if i == 3 { feed_w } else { w };
        let rect = egui::Rect::from_min_size(pos2(x, full.top()), vec2(w, full.height()));
        x += w + 8.0;
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        let ui = &mut child;
        match i {
            0 => {
                ui.strong("YIELD CURVE");
                if let Some(s) = &app.snap {
                    chart::yield_curve(ui, vec2(w - 6.0, full.height() - 22.0), &s.macro_view, &pal);
                }
            }
            3 => feed(app, ui, full.height() - 22.0),
            _ => {
                egui::ScrollArea::vertical().id_salt(("dock", i)).auto_shrink([false, false]).show(ui, |ui| match i {
                    1 => drivers(app, ui),
                    2 => {
                        exchanges(app, ui);
                        ui.add_space(4.0);
                        population(app, ui);
                    }
                    _ => personal_summary(app, ui),
                });
            }
        }
        if i < widths.len() - 1 {
            let sx = rect.right() + 4.0;
            ui.painter().line_segment(
                [pos2(sx, full.top()), pos2(sx, full.bottom())],
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
        }
    }
    ui.advance_cursor_after_rect(full);
}

/// Terminal layout: the same widgets stacked in one column.
pub fn drivers_column(app: &mut App, ui: &mut Ui) {
    let pal = app.settings.theme.palette();
    ui.strong("YIELD CURVE");
    if let Some(s) = &app.snap {
        chart::yield_curve(ui, vec2(ui.available_width(), 150.0), &s.macro_view, &pal);
    }
    ui.separator();
    drivers(app, ui);
    ui.separator();
    exchanges(app, ui);
    ui.separator();
    population(app, ui);
    ui.separator();
    personal_summary(app, ui);
}
