//! Financials pop-up: chart history of market-wide series (the four
//! indices, rates, inflation, credit, and the broker / firm sectors).
//!
//! The engine keeps history for every series all the time, but a snapshot
//! only carries the one this window asks for (`Command::SelectSeries`), so
//! a closed window costs nothing per frame.

use eframe::egui::{self, vec2};
use sigma_engine::history::{ALL_RANGES, ChartRange};
use sigma_engine::protocol::{ALL_MARKET_SERIES, Command, MarketSeries, SeriesUnit};

use crate::app::App;
use crate::chart;
use crate::format;

pub struct FinancialsUi {
    pub series: MarketSeries,
    pub range: ChartRange,
    /// What the engine was last asked to send (None = nothing).
    requested: Option<MarketSeries>,
}

impl Default for FinancialsUi {
    fn default() -> Self {
        Self { series: MarketSeries::Index0, range: ChartRange::Year, requested: None }
    }
}

fn label_for(unit: SeriesUnit) -> fn(f64) -> String {
    match unit {
        SeriesUnit::Level => |v| format!("{v:.1}"),
        SeriesUnit::Percent => |v| format!("{v:.2}%"),
        SeriesUnit::Money => format::money,
        SeriesUnit::Count => |v| format!("{v:.0}"),
    }
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    // Keep the engine's selection in step with the window, both ways.
    let want = app.windows.financials.then_some(app.financials_ui.series);
    if app.financials_ui.requested != want {
        app.send(Command::SelectSeries(want));
        app.financials_ui.requested = want;
    }
    if !app.windows.financials {
        return;
    }
    let mut open = true;
    let pal = app.settings.theme.palette();
    egui::Window::new("Financials").open(&mut open).default_size([640.0, 420.0]).resizable(true).show(ctx, |ui| {
        let st = &mut app.financials_ui;
        ui.horizontal_wrapped(|ui| {
            for s in ALL_MARKET_SERIES {
                ui.selectable_value(&mut st.series, s, s.label());
            }
        });
        ui.horizontal(|ui| {
            for r in ALL_RANGES {
                ui.selectable_value(&mut st.range, r, r.label());
            }
        });
        let size = vec2(ui.available_width(), ui.available_height().max(160.0));
        match app.snap.as_ref().and_then(|s| s.series.as_ref().map(|x| (s.time, x))) {
            Some((now, (series, history))) if *series == st.series => {
                chart::history_chart(ui, size, history, now, st.range, label_for(series.unit()), &pal);
            }
            _ => {
                ui.allocate_ui(size, |ui| ui.centered_and_justified(|ui| ui.label("loading...")));
            }
        }
    });
    app.windows.financials = open;
}
