//! Chyron / marquee ticker crawl, rebuilt to be jitter-free.
//!
//! Why the alpha jittered, and what this does instead:
//!
//! 1. **Time-based fractional offsets.** The alpha computed `t * speed`
//!    every frame; with a 60 Hz display that is e.g. 0.83 px per frame, which
//!    the rasteriser rounds to 0 or 1 px: visible stutter. Here the crawl
//!    moves a *whole number of pixels per frame*, the way broadcast tickers do.
//! 2. **Text that changed while visible.** Each snapshot rewrote every cell's
//!    price, re-laying out text mid-scroll. Here a cell's text is laid out
//!    once when it enters on the right and never changes until it leaves on
//!    the left. New prices appear in the next cell to enter.
//! 3. **Re-measuring every frame.** Galleys (laid-out text) are cached per
//!    cell, so a frame only translates already-shaped text.
//! 4. **Fixed cell spacing that ignored text width.** Widths come from the
//!    galley, rounded up to whole pixels, so spacing is exact and stable.

use std::collections::VecDeque;
use std::sync::Arc;

use eframe::egui::{self, Color32, FontId, Galley, Sense, Ui, text::LayoutJob, text::TextFormat};

struct Cell {
    galley: Arc<Galley>,
    width: f32,
}

#[derive(Default)]
pub struct Crawl {
    cells: VecDeque<Cell>,
    /// How far (px) the first cell has scrolled past the left edge.
    offset: f32,
}

/// One cell's content: segments of (text, colour).
pub type CellText = Vec<(String, Color32)>;

impl Crawl {
    pub fn clear(&mut self) {
        self.cells.clear();
        self.offset = 0.0;
    }

    /// Draw the crawl in a strip of `height` px. `step_px` is the whole
    /// number of pixels to move this frame (0 = paused). `next_cell` is asked
    /// for new content whenever the strip has room on the right.
    pub fn ui(
        &mut self,
        ui: &mut Ui,
        height: f32,
        step_px: u32,
        font: &FontId,
        mut next_cell: impl FnMut() -> Option<CellText>,
    ) {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
        let painter = ui.painter_at(rect);

        self.offset += step_px as f32;
        while let Some(first) = self.cells.front() {
            if self.offset >= first.width {
                self.offset -= first.width;
                self.cells.pop_front();
            } else {
                break;
            }
        }
        let mut filled: f32 = self.cells.iter().map(|c| c.width).sum::<f32>() - self.offset;
        let mut guard = 0;
        while filled < rect.width() + 1.0 && guard < 64 {
            guard += 1;
            let Some(segments) = next_cell() else { break };
            let mut job = LayoutJob::default();
            for (text, color) in segments {
                job.append(&text, 0.0, TextFormat { font_id: font.clone(), color, ..Default::default() });
            }
            let galley = painter.layout_job(job);
            // Whole-pixel widths plus a fixed gap keep every step identical.
            let width = galley.size().x.ceil() + 28.0;
            filled += width;
            self.cells.push_back(Cell { galley, width });
        }

        let ppp = ui.ctx().pixels_per_point();
        let snap = |x: f32| (x * ppp).round() / ppp;
        let mut x = rect.left() - self.offset;
        let y = rect.center().y;
        for cell in &self.cells {
            let pos = egui::pos2(snap(x), snap(y - cell.galley.size().y / 2.0));
            if x <= rect.right() {
                painter.galley(pos, cell.galley.clone(), Color32::WHITE);
            }
            x += cell.width;
        }
    }
}
