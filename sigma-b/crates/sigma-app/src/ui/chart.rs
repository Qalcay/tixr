//! A small line-chart widget drawn with egui's painter.
//!
//! This replaces `egui_plot` (tixr alpha even pulled two egui versions in
//! through `egui_plot` 0.30 + `egui_extras` 0.35). We only need lines, a
//! grid, a shaded band and a hover readout, which is ~150 lines here and
//! zero extra crates.

use crate::theme::Palette;
use eframe::egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2, pos2};

pub struct Series<'a> {
    pub values: &'a [f32],
    pub color: Color32,
    pub width: f32,
    pub dashed: bool,
    /// Optional explicit x positions in [0, 1]; otherwise evenly spaced.
    pub xs: Option<&'a [f32]>,
}

impl<'a> Series<'a> {
    pub fn new(values: &'a [f32], color: Color32) -> Self {
        Self { values, color, width: 1.5, dashed: false, xs: None }
    }
    pub fn dashed(mut self) -> Self {
        self.dashed = true;
        self.width = 1.0;
        self
    }
    pub fn xs(mut self, xs: &'a [f32]) -> Self {
        self.xs = Some(xs);
        self
    }
}

pub struct ChartOpts<'a> {
    pub height: f32,
    pub fill_first: bool,
    /// A shaded forecast cone after the last point: (center, low, high).
    pub forecast: Option<(f32, f32, f32)>,
    pub x_labels: Option<&'a [&'a str]>,
    /// Formats a y value; the second argument is the visible y span, so
    /// labels can show more decimals when zoomed into a narrow range.
    pub y_fmt: fn(f64, f64) -> String,
    pub id: &'a str,
}

impl Default for ChartOpts<'_> {
    fn default() -> Self {
        Self { height: 160.0, fill_first: false, forecast: None, x_labels: None, y_fmt: axis_number, id: "chart" }
    }
}

pub fn line_chart(ui: &mut Ui, pal: &Palette, series: &[Series<'_>], opts: ChartOpts<'_>) {
    let width = ui.available_width().max(40.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, opts.height), Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, pal.bg);

    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for s in series {
        for &v in s.values.iter().filter(|v| v.is_finite()) {
            lo = lo.min(v);
            hi = hi.max(v);
        }
    }
    if let Some((c, l, h)) = opts.forecast {
        for v in [c, l, h] {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if !lo.is_finite() || !hi.is_finite() {
        painter.text(rect.center(), Align2::CENTER_CENTER, "waiting for data", FontId::proportional(12.0), pal.weak);
        return;
    }
    if (hi - lo).abs() < 1e-6 {
        hi += 0.5;
        lo -= 0.5;
    }
    let pad = (hi - lo) * 0.08;
    let (lo, hi) = (lo - pad, hi + pad);

    // Plot area: leave room on the right for y labels, bottom for x labels.
    let label_w = 56.0;
    let bottom = if opts.x_labels.is_some() { 14.0 } else { 8.0 };
    let cone_w = if opts.forecast.is_some() { (rect.width() * 0.18).min(90.0) } else { 0.0 };
    let plot = Rect::from_min_max(rect.min + Vec2::new(4.0, 4.0), rect.max - Vec2::new(label_w + cone_w, bottom));
    let y_to = |v: f32| plot.bottom() - (v - lo) / (hi - lo) * plot.height();

    // Grid and y labels.
    let font = FontId::monospace(10.0);
    for k in 0..=4 {
        let v = lo + (hi - lo) * k as f32 / 4.0;
        let y = y_to(v);
        painter.line_segment([pos2(plot.left(), y), pos2(plot.right() + cone_w, y)], Stroke::new(1.0, pal.grid));
        painter.text(pos2(rect.right() - 2.0, y), Align2::RIGHT_CENTER, (opts.y_fmt)(v as f64, (hi - lo) as f64), font.clone(), pal.weak);
    }
    if let Some(labels) = opts.x_labels {
        let n = labels.len().max(2) - 1;
        for (i, l) in labels.iter().enumerate() {
            let x = plot.left() + plot.width() * i as f32 / n as f32;
            painter.text(pos2(x, rect.bottom() - 1.0), Align2::CENTER_BOTTOM, *l, font.clone(), pal.weak);
        }
    }

    let mut last_point: Option<Pos2> = None;
    for (si, s) in series.iter().enumerate() {
        let n = s.values.len();
        if n == 0 {
            continue;
        }
        let pts: Vec<Pos2> = s
            .values
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite())
            .map(|(i, &v)| {
                let fx = match s.xs {
                    Some(xs) => xs.get(i).copied().unwrap_or(1.0),
                    None if n > 1 => i as f32 / (n - 1) as f32,
                    None => 1.0,
                };
                pos2(plot.left() + fx * plot.width(), y_to(v))
            })
            .collect();
        if si == 0 && opts.fill_first && pts.len() > 1 {
            // Translucent area under the main line, as vertical strips.
            let fill = s.color.gamma_multiply(0.12);
            for w in pts.windows(2) {
                let quad = vec![w[0], w[1], pos2(w[1].x, plot.bottom()), pos2(w[0].x, plot.bottom())];
                painter.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
            }
        }
        let stroke = Stroke::new(s.width, s.color);
        if pts.len() == 1 {
            painter.circle_filled(pts[0], 2.0, s.color);
        } else if s.dashed {
            painter.extend(Shape::dashed_line(&pts, stroke, 4.0, 3.0));
        } else {
            painter.add(Shape::line(pts.clone(), stroke));
        }
        if si == 0 {
            last_point = pts.last().copied();
        }
    }

    if let (Some((c, l, h)), Some(p0)) = (opts.forecast, last_point) {
        let x1 = plot.right() + cone_w;
        let cone = vec![p0, pos2(x1, y_to(h)), pos2(x1, y_to(l))];
        painter.add(Shape::convex_polygon(cone, pal.band, Stroke::NONE));
        painter.line_segment([p0, pos2(x1, y_to(c))], Stroke::new(1.0, pal.accent));
    }

    // Hover readout for the first series.
    if let (Some(pos), Some(s)) = (resp.hover_pos(), series.first())
        && plot.contains(pos)
        && !s.values.is_empty()
        && s.xs.is_none()
    {
        let n = s.values.len();
        let fx = ((pos.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
        let i = ((fx * (n.max(2) - 1) as f32).round() as usize).min(n - 1);
        let v = s.values[i];
        let x = plot.left() + if n > 1 { i as f32 / (n - 1) as f32 } else { 1.0 } * plot.width();
        painter.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, pal.weak));
        painter.circle_filled(pos2(x, y_to(v)), 3.0, s.color);
        let label = (opts.y_fmt)(v as f64, (hi - lo) as f64);
        let anchor = if x > plot.center().x { Align2::RIGHT_TOP } else { Align2::LEFT_TOP };
        painter.text(pos2(x + if x > plot.center().x { -6.0 } else { 6.0 }, plot.top() + 2.0), anchor, label, FontId::monospace(11.0), pal.text);
    }
    let _ = opts.id;
}

/// Default axis label: enough decimals to tell grid lines apart.
pub fn axis_number(v: f64, span: f64) -> String {
    let decimals = if span < 0.05 {
        4
    } else if span < 0.5 {
        3
    } else if span < 50.0 {
        2
    } else {
        0
    };
    if v.abs() >= 1e4 { crate::fmt::compact(v) } else { format!("{v:.decimals$}") }
}

/// Tiny inline sparkline, fixed size.
pub fn sparkline(ui: &mut Ui, values: &[f32], color: Color32, size: Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    if values.len() < 2 {
        return;
    }
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for &v in values {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if hi - lo < 1e-9 {
        hi = lo + 1.0;
    }
    let n = values.len() - 1;
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, &v)| pos2(rect.left() + rect.width() * i as f32 / n as f32, rect.bottom() - (v - lo) / (hi - lo) * rect.height()))
        .collect();
    ui.painter_at(rect).add(Shape::line(pts, Stroke::new(1.0, color)));
}
