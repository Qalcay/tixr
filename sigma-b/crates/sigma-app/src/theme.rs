//! Colours. Applied once when the theme changes, not every frame.

use eframe::egui::{self, Color32, Visuals};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub accent: Color32,
    pub up: Color32,
    pub down: Color32,
    pub grid: Color32,
    pub chip: Color32,
    pub band: Color32,
}

impl Theme {
    pub fn palette(self) -> Palette {
        match self {
            Theme::Dark => Palette {
                bg: Color32::from_rgb(14, 15, 18),
                panel: Color32::from_rgb(22, 24, 29),
                text: Color32::from_rgb(214, 210, 200),
                weak: Color32::from_rgb(128, 126, 120),
                accent: Color32::from_rgb(230, 170, 60),
                up: Color32::from_rgb(70, 200, 120),
                down: Color32::from_rgb(235, 85, 80),
                grid: Color32::from_rgb(44, 47, 55),
                chip: Color32::from_rgb(50, 54, 66),
                band: Color32::from_rgba_unmultiplied(230, 170, 60, 36),
            },
            Theme::Light => Palette {
                bg: Color32::from_rgb(246, 243, 236),
                panel: Color32::from_rgb(236, 231, 222),
                text: Color32::from_rgb(34, 32, 28),
                weak: Color32::from_rgb(110, 102, 92),
                accent: Color32::from_rgb(150, 40, 30),
                up: Color32::from_rgb(20, 120, 60),
                down: Color32::from_rgb(170, 35, 35),
                grid: Color32::from_rgb(214, 206, 194),
                chip: Color32::from_rgb(222, 214, 200),
                band: Color32::from_rgba_unmultiplied(150, 40, 30, 30),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p = self.palette();
        let mut v = match self {
            Theme::Dark => Visuals::dark(),
            Theme::Light => Visuals::light(),
        };
        v.override_text_color = Some(p.text);
        v.panel_fill = p.panel;
        v.window_fill = p.panel;
        v.extreme_bg_color = p.bg;
        v.faint_bg_color = p.chip;
        v.hyperlink_color = p.accent;
        v.selection.bg_fill = p.accent.gamma_multiply(0.35);
        v.selection.stroke = egui::Stroke::new(1.0, p.accent);
        ctx.set_visuals(v);
    }

    pub fn signed(self, v: f64) -> Color32 {
        let p = self.palette();
        if v > 0.0 {
            p.up
        } else if v < 0.0 {
            p.down
        } else {
            p.weak
        }
    }
}
