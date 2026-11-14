//! The stock list: up to 65,536 rows, virtualised.
//!
//! Only the rows inside the scroll viewport are drawn (`show_rows`), and
//! each row is painted at fixed column positions inside a clip rectangle,
//! so the list costs the same with 8k or 32k securities and no row can ever
//! widen the panel.

use crate::app::{SigmaApp, SortKey};
use crate::fmt;
use eframe::egui::{self, Align2, FontId, RichText, Sense, Stroke, Vec2, pos2};
use sigma_core::market::{EXCHANGE_NAMES, EXCHANGES};
use sigma_core::protocol::Row;

const ROW_H: f32 = 18.0;

/// Filter and sort the latest table into `app.view`. Runs only when the
/// table or the filters change (a few times a second at most).
pub fn rebuild_view(app: &mut SigmaApp) {
    app.view_dirty = false;
    let q = app.search.trim().to_ascii_uppercase();
    let perks: Vec<u8> = app
        .perk_filter
        .to_ascii_uppercase()
        .bytes()
        .filter(|b| (b'A'..=b'P').contains(b))
        .map(|b| b - b'A')
        .collect();
    let rows = &app.table;
    let mut view: Vec<u32> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| app.exchange_filter.is_none_or(|e| r.exchange == e))
        .filter(|(_, r)| q.is_empty() || r.ticker.as_str().starts_with(&q))
        .filter(|(_, r)| perks.iter().all(|&p| r.perks.0 & (1 << p) != 0))
        .map(|(i, _)| i as u32)
        .collect();
    let key = |r: &Row| -> f32 {
        match app.sort {
            SortKey::Ticker => 0.0,
            SortKey::Price => r.price,
            SortKey::Change => r.change_pct,
            SortKey::Cap => r.market_cap,
            SortKey::ValueGap => r.value_gap_pct,
            SortKey::Vol => r.vol,
        }
    };
    if app.sort == SortKey::Ticker {
        view.sort_unstable_by(|a, b| rows[*a as usize].ticker.as_str().cmp(rows[*b as usize].ticker.as_str()));
    } else {
        view.sort_unstable_by(|a, b| key(&rows[*a as usize]).total_cmp(&key(&rows[*b as usize])));
    }
    if app.sort_desc {
        view.reverse();
    }
    app.view = view;
}

pub fn show(app: &mut SigmaApp, ui: &mut egui::Ui) {
    let pal = app.pal();
    ui.horizontal(|ui| {
        ui.label("Find");
        let r = ui.add(egui::TextEdit::singleline(&mut app.search).desired_width(70.0).hint_text("ticker"));
        ui.label("Perks");
        let r2 = ui.add(egui::TextEdit::singleline(&mut app.perk_filter).desired_width(60.0).hint_text("e.g. AD"));
        if r.changed() || r2.changed() {
            app.view_dirty = true;
        }
    });
    ui.horizontal(|ui| {
        let mut ex = app.exchange_filter;
        egui::ComboBox::from_id_salt("exfilter")
            .width(80.0)
            .selected_text(ex.map_or("All boards", |e| EXCHANGE_NAMES[e as usize]))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut ex, None, "All boards");
                for e in (0..EXCHANGES as u8).rev() {
                    ui.selectable_value(&mut ex, Some(e), EXCHANGE_NAMES[e as usize]);
                }
            });
        if ex != app.exchange_filter {
            app.exchange_filter = ex;
            app.view_dirty = true;
        }
        let mut sort = app.sort;
        egui::ComboBox::from_id_salt("sort")
            .width(80.0)
            .selected_text(sort_label(sort))
            .show_ui(ui, |ui| {
                for k in [SortKey::Cap, SortKey::Change, SortKey::Price, SortKey::ValueGap, SortKey::Vol, SortKey::Ticker] {
                    ui.selectable_value(&mut sort, k, sort_label(k));
                }
            });
        if sort != app.sort {
            app.sort = sort;
            app.view_dirty = true;
        }
        if ui.small_button(RichText::new(if app.sort_desc { "desc" } else { "asc" }).small()).clicked() {
            app.sort_desc = !app.sort_desc;
            app.view_dirty = true;
        }
    });
    ui.label(RichText::new(format!("{} of {} shown", app.view.len(), app.table.len())).small().color(pal.weak));
    ui.separator();

    // Header.
    let width = ui.available_width();
    let cols = columns(width);
    let (hdr, _) = ui.allocate_exact_size(Vec2::new(width, 14.0), Sense::hover());
    let hp = ui.painter_at(hdr);
    let small = FontId::monospace(10.5);
    hp.text(pos2(hdr.left() + cols[0], hdr.center().y), Align2::LEFT_CENTER, "TICKER", small.clone(), pal.weak);
    hp.text(pos2(hdr.left() + cols[1], hdr.center().y), Align2::RIGHT_CENTER, "PRICE", small.clone(), pal.weak);
    hp.text(pos2(hdr.left() + cols[2], hdr.center().y), Align2::RIGHT_CENTER, "CHG", small.clone(), pal.weak);
    hp.text(pos2(hdr.left() + cols[3], hdr.center().y), Align2::LEFT_CENTER, "PERKS", small, pal.weak);

    let font = FontId::monospace(12.0);
    let mut visible = Vec::new();
    let mut clicked = None;
    egui::ScrollArea::vertical().id_salt("stocklist").auto_shrink([false, false]).show_rows(
        ui,
        ROW_H,
        app.view.len(),
        |ui, range| {
            for i in range {
                let Some(r) = app.view.get(i).and_then(|&ix| app.table.get(ix as usize)) else { continue };
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
                let p = ui.painter_at(rect);
                let selected = app.selected == Some(r.id);
                if selected {
                    p.rect_filled(rect, 2.0, pal.accent.gamma_multiply(0.25));
                } else if resp.hovered() {
                    p.rect_filled(rect, 2.0, pal.chip);
                }
                let y = rect.center().y;
                let chg_col = app.settings.theme.signed(r.change_pct as f64);
                p.text(pos2(rect.left() + cols[0], y), Align2::LEFT_CENTER, r.ticker.as_str(), font.clone(), pal.text);
                p.text(pos2(rect.left() + cols[1], y), Align2::RIGHT_CENTER, fmt::price(r.price as f64), font.clone(), pal.text);
                p.text(pos2(rect.left() + cols[2], y), Align2::RIGHT_CENTER, fmt::pct(r.change_pct as f64), font.clone(), chg_col);
                p.text(pos2(rect.left() + cols[3], y), Align2::LEFT_CENTER, r.perks.letters(), FontId::monospace(10.5), pal.weak);
                if selected {
                    p.line_segment([rect.left_top(), rect.left_bottom()], Stroke::new(2.0, pal.accent));
                }
                if resp.clicked() {
                    clicked = Some(r.id);
                }
                visible.push(r.id);
            }
        },
    );
    if let Some(id) = clicked {
        app.select(id);
    }
    app.visible_rows = visible;
}

/// Column anchor x-offsets for a given panel width: ticker (left aligned),
/// price and change (right aligned), perks (left aligned, clipped).
fn columns(width: f32) -> [f32; 4] {
    let price_right = 52.0 + 66.0;
    let chg_right = price_right + 70.0;
    [4.0, price_right, chg_right, (chg_right + 8.0).min(width - 20.0)]
}

fn sort_label(k: SortKey) -> &'static str {
    match k {
        SortKey::Ticker => "Ticker",
        SortKey::Price => "Price",
        SortKey::Change => "Change",
        SortKey::Cap => "Mkt cap",
        SortKey::ValueGap => "Value gap",
        SortKey::Vol => "Volatility",
    }
}
