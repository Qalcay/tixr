//! Hand-drawn charts using egui's `Painter`.
//!
//! Every chart range (1H ... 10Y) is drawn from the engine's pre-aggregated
//! history tiers, so a redraw is "copy a few hundred points and stroke one
//! polyline": the same cost for an hour as for ten years, with no cached
//! images and nothing to recompute when the range changes.
//!
//! Replaces `egui_plot` (one fewer dependency tree, and the charts here are
//! simple: a line, a forecast cone, a few grid lines). Drawing directly also
//! lets the chart fill exactly the rect it is given, so it can never push
//! neighbouring panels around.

use eframe::egui::{self, Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use sigma_engine::history::{ChartRange, History};
use sigma_engine::macroecon::{TENOR_LABELS, TENORS};
use sigma_engine::protocol::{MacroView, SecurityDetail};
use sigma_engine::time::SECS_PER_DAY;

use crate::format;
use crate::theme::Palette;

/// Maps data space to screen space for one chart.
struct Frame {
    rect: Rect,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
}

impl Frame {
    fn to_screen(&self, x: f64, y: f64) -> Pos2 {
        let fx = ((x - self.x0) / (self.x1 - self.x0).max(1e-12)) as f32;
        let fy = ((y - self.y0) / (self.y1 - self.y0).max(1e-12)) as f32;
        pos2(self.rect.left() + fx * self.rect.width(), self.rect.bottom() - fy * self.rect.height())
    }
}

fn band_mesh(frame: &Frame, pts: &[(f64, f64, f64)], color: Color32) -> Mesh {
    let mut mesh = Mesh::default();
    for (i, &(x, lo, hi)) in pts.iter().enumerate() {
        mesh.colored_vertex(frame.to_screen(x, lo), color);
        mesh.colored_vertex(frame.to_screen(x, hi), color);
        if i > 0 {
            let b = (i as u32 - 1) * 2;
            mesh.add_triangle(b, b + 1, b + 2);
            mesh.add_triangle(b + 1, b + 3, b + 2);
        }
    }
    mesh
}

fn draw_grid(painter: &egui::Painter, frame: &Frame, pal: &Palette, label: impl Fn(f64) -> String) {
    let font = FontId::monospace(10.0);
    for k in 0..=4 {
        let y = frame.y0 + (frame.y1 - frame.y0) * k as f64 / 4.0;
        let p = frame.to_screen(frame.x0, y);
        painter.line_segment([pos2(frame.rect.left(), p.y), pos2(frame.rect.right(), p.y)], Stroke::new(1.0, pal.grid));
        painter.text(
            pos2(frame.rect.right() - 2.0, p.y - 1.0),
            egui::Align2::RIGHT_BOTTOM,
            label(y),
            font.clone(),
            pal.text_weak,
        );
    }
}

/// "3.2d ago" style label for a point `secs` sim seconds in the past.
fn ago(secs: f64) -> String {
    let s = secs.max(0.0);
    if s < 90.0 {
        "now".to_string()
    } else if s < 2.0 * 3_600.0 {
        format!("{:.0}m ago", s / 60.0)
    } else if s < 2.0 * SECS_PER_DAY {
        format!("{:.1}h ago", s / 3_600.0)
    } else if s < 2.0 * 365.0 * SECS_PER_DAY {
        format!("{:.0}d ago", s / SECS_PER_DAY)
    } else {
        format!("{:.1}y ago", s / (365.0 * SECS_PER_DAY))
    }
}

/// Price history plus the model's forecast cone (±1σ, ±2σ, mean).
///
/// The x axis is the bar index, not the clock, so nights and weekends take
/// no room (the convention on every trading screen).
pub fn price_chart(
    ui: &mut Ui,
    size: egui::Vec2,
    d: &SecurityDetail,
    now: f64,
    range: ChartRange,
    forecast: bool,
    pal: &Palette,
) {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, pal.plot_bg);
    let rect = rect.shrink(4.0);

    let mut series: Vec<(f64, f64)> =
        d.history.session_points(range, now).into_iter().map(|(t, v)| (t, v as f64)).collect();
    // The live price is the right-hand end of every range.
    match series.last_mut() {
        Some(last) if now - last.0 < 1.0 => last.1 = d.price,
        _ => series.push((now, d.price)),
    }
    if series.len() < 2 {
        series.insert(0, (now - range.window_secs(), d.prev_close));
    }
    let last_x = (series.len() - 1) as f64;

    // Forecast cone on the longer ranges, limited to a third of the visible
    // history so it never dominates. Bars per sim day converts the
    // forecast's calendar days into this range's x units.
    let long_range =
        matches!(range, ChartRange::Month | ChartRange::Year | ChartRange::ThreeYear | ChartRange::TenYear);
    let cone: Vec<(f64, f64, f64, f64, f64, f64)> = if forecast && long_range {
        let span_days = ((now - series[0].0) / SECS_PER_DAY).max(1e-6);
        let bars_per_day = last_x / span_days;
        let max_bars = (series.len() as f64 / 3.0).max(3.0);
        d.forecast
            .iter()
            .map(|f| (last_x + f.days_ahead * bars_per_day, f.mean, f.lo1, f.hi1, f.lo2, f.hi2))
            .filter(|p| p.0 - last_x <= max_bars)
            .collect()
    } else {
        Vec::new()
    };

    let mut lo = series.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let mut hi = series.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    for c in &cone {
        lo = lo.min(c.2);
        hi = hi.max(c.3);
    }
    let pad = ((hi - lo) * 0.08).max(hi.abs() * 0.002).max(1e-6);
    let x1 = cone.last().map_or(last_x, |c| c.0).max(1.0);
    let frame = Frame { rect, x0: 0.0, x1, y0: lo - pad, y1: hi + pad };
    draw_grid(&painter, &frame, pal, format::price);

    let clip = painter.with_clip_rect(rect);
    if cone.len() > 1 {
        let mut band2: Vec<(f64, f64, f64)> = vec![(last_x, d.price, d.price)];
        let mut band1 = band2.clone();
        band2.extend(cone.iter().map(|c| (c.0, c.4, c.5)));
        band1.extend(cone.iter().map(|c| (c.0, c.2, c.3)));
        clip.add(Shape::mesh(band_mesh(&frame, &band2, pal.band2)));
        clip.add(Shape::mesh(band_mesh(&frame, &band1, pal.band1)));
        let mean: Vec<Pos2> = std::iter::once(frame.to_screen(last_x, d.price))
            .chain(cone.iter().map(|c| frame.to_screen(c.0, c.1)))
            .collect();
        clip.add(Shape::dashed_line(&mean, Stroke::new(1.0, pal.accent), 4.0, 3.0));
    }
    let up = d.price >= series[0].1;
    let color = if up { pal.up } else { pal.down };
    let line: Vec<Pos2> = series.iter().enumerate().map(|(i, p)| frame.to_screen(i as f64, p.1)).collect();
    clip.add(Shape::line(line, Stroke::new(1.4, color)));

    // Fair value marker.
    if long_range && d.fair_value >= frame.y0 && d.fair_value <= frame.y1 {
        let y = frame.to_screen(0.0, d.fair_value).y;
        clip.add(Shape::dashed_line(
            &[pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, pal.text_weak),
            2.0,
            4.0,
        ));
    }

    if let Some(hover) = resp.hover_pos()
        && rect.contains(hover)
    {
        let fx = ((hover.x - rect.left()) / rect.width()) as f64 * (frame.x1 - frame.x0);
        let text = if fx <= last_x {
            let (t, v) = series[(fx.round() as usize).min(series.len() - 1)];
            format!("{}  {}", format::price(v), ago(now - t))
        } else if let Some(c) = cone.iter().min_by(|a, b| (a.0 - fx).abs().total_cmp(&(b.0 - fx).abs())) {
            format!("forecast mean {} (68%: {}..{})", format::price(c.1), format::price(c.2), format::price(c.3))
        } else {
            String::new()
        };
        clip.line_segment([pos2(hover.x, rect.top()), pos2(hover.x, rect.bottom())], Stroke::new(1.0, pal.grid));
        painter.text(
            pos2(rect.left() + 4.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            text,
            FontId::monospace(11.0),
            pal.text,
        );
    }
}

/// A plain line chart of one history range (broker equity, market series).
/// The x axis is time here, because these series run around the clock.
pub fn history_chart(
    ui: &mut Ui,
    size: egui::Vec2,
    history: &History,
    now: f64,
    range: ChartRange,
    label: impl Fn(f64) -> String,
    pal: &Palette,
) {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, pal.plot_bg);
    let rect = rect.shrink(4.0);
    let pts = history.points(range, now);
    if pts.len() < 2 {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "not enough history for this range yet",
            FontId::proportional(12.0),
            pal.text_weak,
        );
        return;
    }
    let lo = pts.iter().map(|p| p.1 as f64).fold(f64::INFINITY, f64::min);
    let hi = pts.iter().map(|p| p.1 as f64).fold(f64::NEG_INFINITY, f64::max);
    let pad = ((hi - lo) * 0.08).max(hi.abs() * 0.002).max(1e-9);
    let frame = Frame { rect, x0: pts[0].0, x1: pts[pts.len() - 1].0.max(pts[0].0 + 1.0), y0: lo - pad, y1: hi + pad };
    draw_grid(&painter, &frame, pal, &label);
    let up = pts[pts.len() - 1].1 >= pts[0].1;
    let line: Vec<Pos2> = pts.iter().map(|&(t, v)| frame.to_screen(t, v as f64)).collect();
    painter.with_clip_rect(rect).add(Shape::line(line, Stroke::new(1.4, if up { pal.up } else { pal.down })));
    if let Some(hover) = resp.hover_pos()
        && rect.contains(hover)
    {
        let t = frame.x0 + ((hover.x - rect.left()) / rect.width()) as f64 * (frame.x1 - frame.x0);
        if let Some(&(pt, pv)) = pts.iter().min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs())) {
            painter.line_segment([pos2(hover.x, rect.top()), pos2(hover.x, rect.bottom())], Stroke::new(1.0, pal.grid));
            painter.text(
                pos2(rect.left() + 4.0, rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                format!("{}  {}", label(pv as f64), ago(now - pt)),
                FontId::monospace(11.0),
                pal.text,
            );
        }
    }
}

/// Yield curve (solid) and the curve-implied policy path (dashed).
pub fn yield_curve(ui: &mut Ui, size: egui::Vec2, m: &MacroView, pal: &Palette) {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, pal.plot_bg);
    let rect = Rect::from_min_max(rect.min + vec2(4.0, 4.0), rect.max - vec2(4.0, 14.0));
    // sqrt spacing spreads out the short end, as on a trading desk screen.
    let xs = |tau: f64| tau.sqrt();
    let mut lo = m.curve.iter().copied().fold(f64::INFINITY, f64::min);
    let mut hi = m.curve.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for &(_, f) in &m.forward_path {
        lo = lo.min(f);
        hi = hi.max(f);
    }
    let pad = ((hi - lo) * 0.15).max(0.002);
    let frame = Frame { rect, x0: 0.0, x1: xs(30.0), y0: (lo - pad).max(-0.05), y1: hi + pad };
    draw_grid(&painter, &frame, pal, |y| format!("{:.2}%", y * 100.0));
    let pts: Vec<Pos2> = TENORS.iter().zip(m.curve.iter()).map(|(&t, &y)| frame.to_screen(xs(t), y)).collect();
    let inverted = m.spread_10y_3m < 0.0;
    painter.add(Shape::line(pts.clone(), Stroke::new(1.6, if inverted { pal.down } else { pal.accent })));
    for p in &pts {
        painter.circle_filled(*p, 2.0, pal.accent);
    }
    let fwd: Vec<Pos2> = m.forward_path.iter().map(|&(t, f)| frame.to_screen(xs(t.max(1e-3)), f)).collect();
    painter.add(Shape::dashed_line(&fwd, Stroke::new(1.0, pal.text_weak), 3.0, 3.0));
    let font = FontId::monospace(9.0);
    for (i, &t) in TENORS.iter().enumerate() {
        if i % 2 == 0 || i == TENORS.len() - 1 {
            let p = frame.to_screen(xs(t), frame.y0);
            painter.text(
                pos2(p.x, rect.bottom() + 2.0),
                egui::Align2::CENTER_TOP,
                TENOR_LABELS[i],
                font.clone(),
                pal.text_weak,
            );
        }
    }
    if let Some(h) = resp.hover_pos()
        && let Some((i, _)) = pts.iter().enumerate().min_by(|a, b| (a.1.x - h.x).abs().total_cmp(&(b.1.x - h.x).abs()))
    {
        painter.text(
            pos2(rect.left() + 4.0, rect.top()),
            egui::Align2::LEFT_TOP,
            format!("{} {}", TENOR_LABELS[i], format::rate(m.curve[i])),
            FontId::monospace(11.0),
            pal.text,
        );
    }
}
