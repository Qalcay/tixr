//! Financial market drivers: yield curve, rates, indices, market health.
//! The same four sections are laid out as a bottom dock (Desk layout) or a
//! side column (Terminal layout).

use crate::app::SigmaApp;
use crate::fmt;
use crate::ui::chart::{ChartOpts, Series, line_chart, sparkline};
use eframe::egui::{self, RichText, Vec2};
use sigma_core::macro_econ::TENOR_LABELS;
use sigma_core::protocol::Frame;

pub fn dock(app: &mut SigmaApp, ui: &mut egui::Ui) {
    let Some(f) = app.frame.clone() else { return };
    // A scroll area keeps content top-aligned and clipped inside the
    // fixed-height dock instead of spilling past its edges.
    egui::ScrollArea::vertical().id_salt("dock").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.columns(4, |cols| {
            yield_curve(app, &mut cols[0], &f, 150.0);
            rates(app, &mut cols[1], &f);
            indices(app, &mut cols[2], &f);
            health(app, &mut cols[3], &f);
        });
    });
}

pub fn column(app: &mut SigmaApp, ui: &mut egui::Ui) {
    let Some(f) = app.frame.clone() else { return };
    egui::ScrollArea::vertical().id_salt("drivers").auto_shrink([false, false]).show(ui, |ui| {
        yield_curve(app, ui, &f, 140.0);
        ui.separator();
        rates(app, ui, &f);
        ui.separator();
        indices(app, ui, &f);
        ui.separator();
        health(app, ui, &f);
    });
}

fn yield_curve(app: &SigmaApp, ui: &mut egui::Ui, f: &Frame, h: f32) {
    let pal = app.pal();
    let d = &f.drivers;
    ui.horizontal(|ui| {
        ui.label(RichText::new("YIELD CURVE").strong());
        if d.term_spread < 0.0 {
            ui.label(RichText::new("INVERTED").small().color(pal.down));
        }
    });
    // Tenors are spaced by sqrt(years) so the short end is readable.
    let xs: Vec<f32> = sigma_core::macro_econ::TENORS.iter().map(|t| (t.sqrt() / 30f64.sqrt()) as f32).collect();
    let now: Vec<f32> = d.curve.iter().map(|y| (y * 100.0) as f32).collect();
    let fwd: Vec<f32> = d.forwards.iter().map(|y| (y * 100.0) as f32).collect();
    let ago: Vec<f32> = d.curve_month_ago.map(|c| c.iter().map(|y| (y * 100.0) as f32).collect()).unwrap_or_default();
    let mut series = vec![Series::new(&now, pal.accent).xs(&xs), Series::new(&fwd, pal.weak).dashed().xs(&xs)];
    if !ago.is_empty() {
        series.push(Series::new(&ago, pal.grid).xs(&xs));
    }
    let labels = ["1M", "", "", "1Y", "2Y", "5Y", "10Y", "30Y"];
    line_chart(ui, &pal, &series, ChartOpts { height: h, x_labels: Some(&labels), y_fmt: |v, _| format!("{v:.2}%"), id: "yc", ..Default::default() });
    let _ = TENOR_LABELS;
    ui.label(RichText::new("solid: today · dashed: forward (market forecast) · faint: month ago").small().color(pal.weak));
}

fn rates(app: &SigmaApp, ui: &mut egui::Ui, f: &Frame) {
    let pal = app.pal();
    let d = &f.drivers;
    ui.label(RichText::new("RATES & ECONOMY").strong());
    egui::Grid::new(ui.next_auto_id()).num_columns(2).show(ui, |ui| {
        row(ui, "Policy rate", fmt::rate(d.policy_rate), None);
        row(ui, "Taylor target", fmt::rate(d.taylor_target), None);
        row(ui, "Inflation", fmt::rate(d.inflation), None);
        row(ui, "Output gap", fmt::pct(d.output_gap * 100.0), Some(app.settings.theme.signed(d.output_gap)));
        row(ui, "3M / 2Y / 10Y", format!("{:.2} {:.2} {:.2}", d.y3m * 100.0, d.y2 * 100.0, d.y10 * 100.0), None);
        row(ui, "10Y-3M spread", format!("{:+.0} bp", d.term_spread * 1e4), Some(app.settings.theme.signed(d.term_spread)));
        row(ui, "Recession in 12m", fmt::rate(d.recession_prob), Some(if d.recession_prob > 0.3 { pal.down } else { pal.text }));
        row(ui, "HY credit spread", format!("{:.0} bp", d.credit_spread * 1e4), None);
        row(ui, "Equity premium", fmt::rate(d.erp), None);
        row(ui, "Fear index", format!("{:.1}", d.fear), Some(if d.fear > 22.0 { pal.down } else { pal.text }));
    });
}

fn row(ui: &mut egui::Ui, k: &str, v: String, color: Option<egui::Color32>) {
    ui.label(RichText::new(k).small());
    let t = RichText::new(v).monospace();
    ui.label(if let Some(c) = color { t.color(c) } else { t });
    ui.end_row();
}

fn indices(app: &SigmaApp, ui: &mut egui::Ui, f: &Frame) {
    let pal = app.pal();
    ui.label(RichText::new("INDICES").strong());
    for ix in &f.indices {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_sized([62.0, 16.0], egui::Label::new(RichText::new(&ix.name).monospace().small()));
            ui.add_sized([52.0, 16.0], egui::Label::new(RichText::new(format!("{:.1}", ix.level)).monospace().small()));
            ui.add_sized([54.0, 16.0], egui::Label::new(RichText::new(fmt::pct(ix.change_pct)).monospace().small().color(app.settings.theme.signed(ix.change_pct))));
            let w = (ui.available_width() - 4.0).clamp(0.0, 60.0);
            sparkline(ui, &ix.history, pal.accent, Vec2::new(w, 14.0));
        });
    }
    let d = &f.drivers;
    let total = (d.advancers + d.decliners).max(1) as f32;
    ui.label(RichText::new(format!("Breadth: {} up / {} down", d.advancers, d.decliners)).small());
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(220.0), 6.0), egui::Sense::hover());
    let split = rect.left() + rect.width() * d.advancers as f32 / total;
    ui.painter().rect_filled(egui::Rect::from_min_max(rect.min, egui::pos2(split, rect.max.y)), 1.0, pal.up);
    ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(split, rect.min.y), rect.max), 1.0, pal.down);
}

fn health(app: &SigmaApp, ui: &mut egui::Ui, f: &Frame) {
    let pal = app.pal();
    let s = &f.stats;
    ui.label(RichText::new("MARKET HEALTH").strong());
    // Population bar: log scale from 2^12 to 2^16 with the target band marked.
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().min(220.0), 10.0), egui::Sense::hover());
    let lx = |n: f64| rect.left() + rect.width() * (((n.max(4096.0)).log2() - 12.0) / 4.0) as f32;
    let p = ui.painter();
    p.rect_filled(rect, 2.0, pal.chip);
    p.rect_filled(egui::Rect::from_min_max(egui::pos2(lx(s.band.0 as f64), rect.top()), egui::pos2(lx(s.band.1 as f64), rect.bottom())), 2.0, pal.band);
    let x = lx(s.live as f64);
    p.line_segment([egui::pos2(x, rect.top() - 2.0), egui::pos2(x, rect.bottom() + 2.0)], egui::Stroke::new(2.0, pal.accent));
    egui::Grid::new(ui.next_auto_id()).num_columns(2).show(ui, |ui| {
        row(ui, "Listed", format!("{} / {}", s.live, s.capacity), None);
        row(ui, "IPOs / exits a day", format!("{:.1} / {:.1}", s.births_per_day, s.deaths_per_day), None);
        row(ui, "Def / Acq / Delist", format!("{} / {} / {}", s.defaults, s.acquisitions, s.delistings), None);
        row(ui, "Brokers alive", format!("{} (broke {})", s.brokers_alive, s.brokers_broke), None);
        row(ui, "Engine", format!("{:.0}k upd/s", s.updates_per_sec / 1000.0), None);
        row(ui, "Quote age", format!("avg {:.1}m max {:.0}m", s.mean_age_min, s.max_age_min), None);
        row(ui, "Tick", format!("{:.2} ms", s.tick_ms), Some(if s.non_finite_events > 0 { pal.down } else { pal.text }));
    });
}
