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
            speed:              0.8,
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
            (" speed=",         &mut self.speed,            0.01, 800.0),
            (" volume=",        &mut self.vol_mult,         0.0, 100.0),
            (" jumps=",         &mut self.jump_mult,        0.0, 16.0),
            (" drifts=",        &mut self.drift_mult,      -1.0, 16.0),
            (" dumps=",         &mut self.crash_mult,       0.5, 16.0),
            (" defuncts=",      &mut self.ending_mult,      0.0, 16.0),
            (" zeal=",          &mut self.broker_aggro,     0.0, 64.0),
            (" liquid=",        &mut self.impact_mult,      0.0, 128.0),
        ]
    }

    pub fn boring() -> Self {
        Self {
            speed:              0.7,
            vol_mult:           0.7,
            jump_mult:          0.7,
            crash_mult:         0.7,
            ending_mult:        0.7,
            ..Default::default()
        }
    }
    pub fn casino() -> Self {
        Self {
            speed:              10.0,
            vol_mult:           2.0,
            jump_mult:          4.0,
            crash_mult:         4.0,
            ending_mult:        0.5,
            broker_aggro:       1.5,
            impact_mult:        1.5,
            drift_mult:         2.0,
        }
    }
    pub fn rush() -> Self {
        Self {
            speed:              800.0,
            vol_mult:           1.5,
            jump_mult:          1.5,
            crash_mult:         1.5,
            ending_mult:        1.5,
            broker_aggro:       1.5,
            impact_mult:        1.5,
            drift_mult:         1.5,
        }
    }
}
