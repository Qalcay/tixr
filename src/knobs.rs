#[derive(Clone, Copy, Debug)]
pub struct Knobs {
    pub speed:                  f32,
    pub vol_mult:               f32,
    pub jump_mult:              f32,
    pub drift_mult:             f32,
    pub crash_mult:             f32,
    pub ending_mult:            f32,
    pub broker_aggro:           f32,
    //pub broker_hype: ,
    pub impact_mult:            f32,
}

impl Default for Knobs {
    fn default() -> Self {
        Self {
            speed:              1.0,
            vol_mult:           1.0,
            jump_mult:          1.0,
            drift_mult:         1.0,
            crash_mult:         1.0,
            ending_mult:        1.0,
            broker_aggro:       1.0,
            impact_mult:        1.0,
        }
    }
}

impl Knobs {
    pub fn sliders(&mut self) -> [(&'static str, &mut f32, f32, f32); 8] {
        [
            ("speed",   &mut self.speed,            0.1, 8.0),
            ("vol",     &mut self.vol_mult,         0.0, 4.0),
            ("jumps",   &mut self.jump_mult,        0.0, 6.0),
            ("drift",   &mut self.drift_mult,      -1.0, 3.0),
            ("crash",   &mut self.crash_mult,       0.5, 4.0),
            ("defunct", &mut self.ending_mult,      0.0, 6.0),
            ("zealous", &mut self.broker_aggro,     0.0, 5.0),
            ("liquid",  &mut self.impact_mult,      0.0, 4.0),
        ]
    }

    pub fn boring() -> Self {
        Self {
            speed:              0.6,
            vol_mult:           0.5,
            jump_mult:          0.3,
            crash_mult:         0.6,
            ending_mult:        0.3,
            ..Default::default()
        }
    }
    pub fn casino() -> Self {
        Self {
            speed:              3.0,
            vol_mult:           2.2,
            jump_mult:          3.5,
            crash_mult:         2.5,
            ending_mult:        3.0,
            broker_aggro:       2.5,
            impact_mult:        2.0,
            drift_mult:         1.0,
        }
    }
}
