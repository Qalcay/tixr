use eframe::egui::{self, Color32, Visuals};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub panel_bg:                           Color32,
    pub plot_bg:                            Color32,
    pub text:                               Color32,
    pub text_weak:                          Color32,
    pub accent:                             Color32,
    pub up:                                 Color32,
    pub down:                               Color32,
    pub grid:                               Color32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Theme { DarkMode, LightMode }

impl Default for Theme { fn default() -> Self { Theme::DarkMode } }

impl Theme {
    pub fn toggle(self) -> Self { match self { Theme::DarkMode => Theme::LightMode, Theme::LightMode => Theme::DarkMode } }
    pub fn icon(self)   -> &'static str { match self { Theme::DarkMode => "swap light", Theme::LightMode => "swap dark" } }

    pub fn palette(self) -> Palette {
        match self {
            Theme::DarkMode => Palette {
                panel_bg:                   Color32::from_rgb(11, 18, 11),
                plot_bg:                    Color32::from_rgb(9, 11, 9),
                text:                       Color32::from_rgb(202, 222, 202),
                text_weak:                  Color32::from_rgb(111, 145, 111),
                accent:                     Color32::from_rgb(11, 234, 128),
                up:                         Color32::from_rgb(44, 222, 128),
                down:                       Color32::from_rgb(234, 77, 66),
                grid:                       Color32::from_rgb(27, 36, 27),
            },
            Theme::LightMode => Palette {
                panel_bg:                   Color32::from_rgb(234, 222, 207),
                plot_bg:                    Color32::from_rgb(243, 232, 234),
                text:                       Color32::from_rgb(36, 33, 27),
                text_weak:                  Color32::from_rgb(111, 99, 89),
                accent:                     Color32::from_rgb(144, 27, 27),
                up:                         Color32::from_rgb(27, 127, 66),
                down:                       Color32::from_rgb(167, 33, 33),
                grid:                       Color32::from_rgb(222, 207, 198),
            },
        }
    }

    pub fn apply(self, ctx: &egui::Context) {
        let p =  self.palette();
        let mut v = match self {
            Theme::DarkMode                 => Visuals::dark(),
            Theme::LightMode                => Visuals::light()
        };
        v.override_text_color               = Some(p.text);
        v.panel_fill                        = p.panel_bg;
        v.window_fill                       = p.panel_bg;
        v.faint_bg_color                    = p.panel_bg;
        v.extreme_bg_color                  = p.plot_bg;
        v.hyperlink_color                   = p.accent;
        v.selection.bg_fill                 = Color32::from_rgba_unmultiplied(p.accent.r(), p.accent.g(), p.accent.b(), 90);
        v.selection.stroke                  = egui::Stroke::new(1.0, p.accent);
        v.widgets.noninteractive.bg_fill    = p.panel_bg;
        v.widgets.inactive.bg_fill          = p.panel_bg;
        ctx.set_visuals(v);
    }
}
