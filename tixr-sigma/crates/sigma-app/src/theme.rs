//! Colour palettes (carried over from the alpha's dark/light themes).

use eframe::egui::{self, Color32, Stroke, Visuals};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub panel_bg: Color32,
    pub plot_bg: Color32,
    pub text: Color32,
    pub text_weak: Color32,
    pub accent: Color32,
    pub up: Color32,
    pub down: Color32,
    pub grid: Color32,
    pub band1: Color32,
    pub band2: Color32,
}

impl Theme {
    pub fn label(self) -> &'static str {
        match self {
            Theme::Dark => "Dark",
            Theme::Light => "Light",
        }
    }

    pub fn palette(self) -> Palette {
        match self {
            Theme::Dark => Palette {
                panel_bg: Color32::from_rgb(9, 11, 9),
                plot_bg: Color32::from_rgb(2, 4, 2),
                text: Color32::from_rgb(222, 252, 222),
                text_weak: Color32::from_rgb(117, 180, 117),
                accent: Color32::from_rgb(9, 243, 145),
                up: Color32::from_rgb(36, 243, 117),
                down: Color32::from_rgb(243, 66, 48),
                grid: Color32::from_rgb(24, 36, 24),
                band1: Color32::from_rgba_unmultiplied(9, 243, 145, 40),
                band2: Color32::from_rgba_unmultiplied(9, 243, 145, 18),
            },
            Theme::Light => Palette {
                panel_bg: Color32::from_rgb(234, 222, 207),
                plot_bg: Color32::from_rgb(243, 234, 226),
                text: Color32::from_rgb(36, 33, 27),
                text_weak: Color32::from_rgb(111, 99, 89),
                accent: Color32::from_rgb(144, 27, 27),
                up: Color32::from_rgb(27, 127, 66),
                down: Color32::from_rgb(167, 33, 33),
                grid: Color32::from_rgb(214, 200, 188),
                band1: Color32::from_rgba_unmultiplied(144, 27, 27, 40),
                band2: Color32::from_rgba_unmultiplied(144, 27, 27, 18),
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
        v.panel_fill = p.panel_bg;
        v.window_fill = p.panel_bg;
        v.faint_bg_color = p.grid;
        v.extreme_bg_color = p.plot_bg;
        v.hyperlink_color = p.accent;
        v.selection.bg_fill = Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 70);
        v.selection.stroke = Stroke::new(1.0, p.accent);
        ctx.set_visuals(v);
    }
}
