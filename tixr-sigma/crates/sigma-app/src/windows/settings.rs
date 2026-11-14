//! Settings pop-up.

use eframe::egui::{self, RichText};
use sigma_engine::console::ALL_LEVELS;
use sigma_engine::protocol::Command;
use sigma_engine::time::REAL_SECS_PER_SIM_DAY;

use crate::app::{App, LayoutKind};
use crate::theme::Theme;

const SPEEDS: [f64; 9] = [0.25, 0.5, 1.0, 2.0, 4.0, 16.0, 64.0, 256.0, 1024.0];

#[derive(Default)]
pub struct SettingsUi {
    pub seed_text: String,
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    if !app.windows.settings {
        return;
    }
    let mut open = true;
    let mut commands = Vec::new();
    egui::Window::new("Settings").open(&mut open).resizable(false).default_width(380.0).show(ctx, |ui| {
        let s = &mut app.settings;
        egui::Grid::new("settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Simulation speed");
            ui.horizontal_wrapped(|ui| {
                for v in SPEEDS {
                    if ui.selectable_label(s.speed == v, format!("{v}x")).clicked() {
                        s.speed = v;
                        commands.push(Command::SetSpeed(v));
                    }
                }
            });
            ui.end_row();
            ui.label("");
            let mins = REAL_SECS_PER_SIM_DAY / 60.0 / s.speed;
            ui.label(RichText::new(format!("1 sim day = {mins:.1} real minutes (1x = 90 min)")).small());
            ui.end_row();

            ui.label("Fast-forward closed hours");
            if ui.checkbox(&mut s.closed_boost, "8x while every exchange is shut").changed() {
                commands.push(Command::SetClosedBoost(s.closed_boost));
            }
            ui.end_row();

            ui.label("Layout");
            ui.horizontal(|ui| {
                for l in [LayoutKind::Classic, LayoutKind::Terminal] {
                    ui.radio_value(&mut s.layout, l, l.label());
                }
            });
            ui.end_row();

            ui.label("Theme");
            ui.horizontal(|ui| {
                for t in [Theme::Dark, Theme::Light] {
                    ui.radio_value(&mut s.theme, t, t.label());
                }
            });
            ui.end_row();

            ui.label("UI scale");
            ui.add(egui::Slider::new(&mut s.ui_scale, 0.75..=1.75).step_by(0.05));
            ui.end_row();

            ui.label("Ticker speed");
            ui.add(egui::Slider::new(&mut s.ticker_step, 0..=4).suffix(" px/frame"));
            ui.end_row();

            ui.label("Ticker rows");
            ui.add(egui::Slider::new(&mut s.ticker_rows, 1..=4));
            ui.end_row();

            ui.label("Forecast cone");
            ui.checkbox(&mut s.show_forecast, "show on chart");
            ui.end_row();

            ui.label("Console log level");
            ui.horizontal(|ui| {
                for l in ALL_LEVELS {
                    if ui.selectable_label(s.log_level == l, l.label()).clicked() {
                        s.log_level = l;
                        commands.push(Command::SetLogLevel(l));
                    }
                }
            });
            ui.end_row();
        });
        ui.label(RichText::new("Debug output goes to the terminal that launched Tixr-sigma.").small());
        ui.separator();
        ui.horizontal(|ui| {
            let current = app.snap.as_ref().map(|s| s.seed).unwrap_or(0);
            ui.label(format!("Seed {current}"));
            ui.add(
                egui::TextEdit::singleline(&mut app.settings_ui.seed_text).desired_width(140.0).hint_text("new seed"),
            );
            if ui.button("Restart").clicked() {
                let seed = app.settings_ui.seed_text.trim().parse::<u64>().unwrap_or(current);
                commands.push(Command::Restart { seed });
            }
        });
        ui.label(RichText::new("The same seed always produces the same market.").small());
    });
    for c in commands {
        if matches!(c, Command::Restart { .. }) {
            app.selected = None;
        }
        app.send(c);
    }
    app.windows.settings = open;
}
