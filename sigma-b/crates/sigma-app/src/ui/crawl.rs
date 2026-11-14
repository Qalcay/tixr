//! The ticker crawl (marquee), rebuilt for smoothness.
//!
//! ## Why tixr alpha jittered
//! 1. The offset was `(time * speed) % total_width`, and `total_width`
//!    changed whenever the number of cells changed (stocks listing or
//!    failing), so the whole crawl jumped.
//! 2. Every cell was given the same fixed width while its text had a
//!    different real width, so cells overlapped or left gaps as the
//!    numbers changed.
//! 3. Text was drawn at fractional pixel positions, so glyphs snapped to
//!    the pixel grid independently and appeared to wobble.
//!
//! ## What this does
//! * The scroll offset is *state* that advances by `speed * dt` each frame,
//!   with `dt` clamped, so it never jumps.
//! * Cells form a queue. When the first one scrolls fully off the left
//!   edge it is popped and a new cell joins at the right, taken round-robin
//!   from the engine's latest list of movers. The set of visible cells only
//!   changes off-screen.
//! * Cell text is fixed-width monospace (`TICKR  123.45  +1.23%`), so a
//!   cell's width never changes while it is on screen; prices inside it
//!   update in place.
//! * The whole row is shifted by one offset rounded to a physical pixel,
//!   so all glyphs move together.

use crate::fmt;
use crate::theme::Palette;
use eframe::egui::{Align2, FontId, Sense, Ui, Vec2, pos2};
use sigma_core::protocol::CrawlCell;
use std::collections::{HashMap, VecDeque};

const GAP: f32 = 22.0;

#[derive(Default)]
pub struct CrawlRow {
    cells: VecDeque<CrawlCell>,
    offset: f32,
    cursor: usize,
    cell_w: f32,
}

impl CrawlRow {
    /// Advance and draw. `source` is the engine's current movers for this
    /// row; `live` maps ids to fresh quotes so on-screen cells update.
    pub fn show(
        &mut self,
        ui: &mut Ui,
        pal: &Palette,
        source: &[CrawlCell],
        live: &HashMap<u32, (f32, f32)>,
        speed: f32,
        dt: f32,
        height: f32,
    ) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);
        let font = FontId::monospace(12.5);
        if self.cell_w == 0.0 {
            let sample = "WWWWW 9999.99 +99.99%".to_string();
            self.cell_w = painter.layout_no_wrap(sample, font.clone(), pal.text).size().x + GAP;
        }
        if source.is_empty() {
            painter.text(rect.left_center(), Align2::LEFT_CENTER, " waiting for quotes...", font, pal.weak);
            return;
        }
        // Keep enough cells queued to cover the row plus one spare.
        let needed = (rect.width() / self.cell_w).ceil() as usize + 2;
        while self.cells.len() < needed {
            self.push_next(source);
        }
        self.offset += speed * dt.clamp(0.0, 0.05);
        while self.offset >= self.cell_w {
            self.offset -= self.cell_w;
            self.cells.pop_front();
            self.push_next(source);
        }
        let ppp = ui.pixels_per_point();
        let shift = (self.offset * ppp).round() / ppp;
        let y = (rect.center().y * ppp).round() / ppp;
        let mut x = rect.left() - shift;
        for cell in &self.cells {
            let (price, chg) = live.get(&cell.id.0).copied().unwrap_or((cell.price, cell.change_pct));
            let col = if chg > 0.0 {
                pal.up
            } else if chg < 0.0 {
                pal.down
            } else {
                pal.weak
            };
            let arrow = if chg > 0.0 {
                "▲"
            } else if chg < 0.0 {
                "▼"
            } else {
                " "
            };
            let text = format!("{:<5} {:>7} {}{:>6.2}%", cell.ticker.as_str(), fmt::price(price as f64), arrow, chg.abs().min(99.99));
            painter.text(pos2(x, y), Align2::LEFT_CENTER, text, font.clone(), col);
            x += self.cell_w;
            if x > rect.right() {
                break;
            }
        }
    }

    /// Append the next source cell that is not already queued, so the same
    /// ticker never shows twice in one row (the source list reorders as
    /// prices move, so a plain round-robin could repeat).
    fn push_next(&mut self, source: &[CrawlCell]) {
        for _ in 0..source.len() {
            let c = source[self.cursor % source.len()];
            self.cursor = self.cursor.wrapping_add(1);
            if !self.cells.iter().any(|q| q.id == c.id) {
                self.cells.push_back(c);
                return;
            }
        }
        // Fewer distinct tickers than slots: allow a repeat.
        self.cells.push_back(source[self.cursor % source.len()]);
        self.cursor = self.cursor.wrapping_add(1);
    }

    pub fn visible_ids(&self) -> impl Iterator<Item = sigma_core::security::SecurityId> + '_ {
        self.cells.iter().map(|c| c.id)
    }
}
