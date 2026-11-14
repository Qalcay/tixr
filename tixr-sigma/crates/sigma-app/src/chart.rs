//! Hand-drawn charts using egui's `Painter`.
//!
//! Replaces `egui_plot` (one fewer dependency tree, and the charts here are
//! simple: a line, a forecast cone, a few grid lines). Drawing directly also
//! lets the chart fill exactly the rect it is given, so it can never push
//! neighbouring panels around.

use eframe::egui::{self, Color32, FontId, Mesh, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2};
use sigma_engine::macroecon::{TENOR_LABELS, TENORS};
use sigma_engine::protocol::{MacroView, SecurityDetail};

use crate::format;
use crate::theme::Palette;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChartRange {
    Intraday,
    Month,
    Quarter,
    Year,
    All,
}

pub const ALL_RANGES: [ChartRange; 5] =
    [ChartRange::Intraday, ChartRange::Month, ChartRange::Quarter, ChartRange::Year, ChartRange::All];

impl ChartRange {
    pub fn label(self) -> &'static str {
        match self {
            ChartRange::Intraday => "1D",
            ChartRange::Month => "1M",
            ChartRange::Quarter => "3M",
            ChartRange::Year => "1Y",
            ChartRange::All => "All",
        }
    }
    fn sessions(self) -> usize {
        match self {
            ChartRange::Intraday => 0,
            ChartRange::Month => 22,
            ChartRange::Quarter => 66,
            ChartRange::Year => 260,
            ChartRange::All => usize::MAX,
        }
    }
}

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

/// Price history plus the model's forecast cone (±1σ, ±2σ, mean).
pub fn price_chart(
    ui: &mut Ui,
    size: egui::Vec2,
    d: &SecurityDetail,
    range: ChartRange,
    forecast: bool,
    pal: &Palette,
) {
    let (resp, painter) = ui.allocate_painter(size, Sense::hover());
    let rect = resp.rect;
    painter.rect_filled(rect, 2.0, pal.plot_bg);
    let rect = rect.shrink(4.0);

    let mut series: Vec<f64> = if range == ChartRange::Intraday {
        d.intraday.iter().map(|&v| v as f64).collect()
    } else {
        let n = d.closes.len();
        let take = range.sessions().min(n);
        d.closes[n - take..].iter().map(|&v| v as f64).collect()
    };
    series.push(d.price);
    if series.len() < 2 {
        series.insert(0, d.prev_close);
    }
    let last_x = (series.len() - 1) as f64;

    // Forecast in trading sessions (5 per 7 calendar days), limited to a
    // third of the visible history so it never dominates the chart.
    let cone: Vec<(f64, f64, f64, f64, f64, f64)> = if forecast && range != ChartRange::Intraday {
        let max_sessions = (series.len() as f64 / 3.0).clamp(5.0, 43.0);
        d.forecast
            .iter()
            .map(|f| (last_x + f.days_ahead * 5.0 / 7.0, f.mean, f.lo1, f.hi1, f.lo2, f.hi2))
            .filter(|p| p.0 - last_x <= max_sessions)
            .collect()
    } else {
        Vec::new()
    };

    let mut lo = series.iter().copied().fold(f64::INFINITY, f64::min);
    let mut hi = series.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    for c in &cone {
        lo = lo.min(c.2);
        hi = hi.max(c.3);
    }
    let pad = ((hi - lo) * 0.08).max(hi.abs() * 0.002).max(1e-6);
    let x1 = cone.last().map_or(last_x, |c| c.0).max(1.0);
    let frame = Frame { rect, x0: 0.0, x1, y0: lo - pad, y1: hi + pad };
    draw_grid(&painter, &frame, pal, format::price);

    let clip = painter.with_clip_rect(rect);
    if !cone.is_empty() {
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
    let up = d.price >= series[0];
    let color = if up { pal.up } else { pal.down };
    let line: Vec<Pos2> = series.iter().enumerate().map(|(i, &v)| frame.to_screen(i as f64, v)).collect();
    clip.add(Shape::line(line, Stroke::new(1.4, color)));

    // Fair value marker.
    if range != ChartRange::Intraday {
        let fv = d.fair_value.clamp(frame.y0, frame.y1);
        let y = frame.to_screen(0.0, fv).y;
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
            let i = (fx.round() as usize).min(series.len() - 1);
            format::price(series[i]).to_string()
        } else if let Some(c) = cone.iter().min_by(|a, b| (a.0 - fx).abs().total_cmp(&(b.0 - fx).abs())) {
            format!(
                "+{:.0}d mean {} (68%: {}..{})",
                (c.0 - last_x) * 7.0 / 5.0,
                format::price(c.1),
                format::price(c.2),
                format::price(c.3)
            )
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
