mod values;
mod magic;
mod engine;
mod knobs;
mod themes;
use crate::themes::Theme;

use crate::values::*;
use crate::magic::*;
use crate::engine::*;
use crate::knobs::Knobs;

use std::collections::VecDeque;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread;
use std::time::{Duration, Instant};
use std::fs;

use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::egui::{self, *};
//use eframe::egui;
//use egui::*;
use egui_plot::{Line, Plot, PlotPoints};
//use egui_extras::{FontIcons, IconData};

enum SimulatorResponsiveness { Snapshot(Box<SnapshotEconomy>), Saved(Vec<u8>) }
enum SimulatorCommands {
    Pause   (bool),
    Select  (usize),
    Buys    { ix: usize, notion: f64 },
    Sell    { ix: usize, notion: f64 },
    SetKnobs(Knobs),
    Save,
    Load(Vec<u8>),
    Stop,
}

fn simulation_thread(
    seed: SeedMarketTheory,
    rx: Receiver<SimulatorCommands>,
    tx: Sender<SimulatorResponsiveness>,
    stop: Arc<AtomicBool>) {
        let mut engine = EconomyEngine::new(seed); // <- boots already pre-cooked
        let mut pauses = false;
        let mut last_case = Instant::now();

        while !stop.load(Ordering::Relaxed) {
            let loop_init = Instant::now();

            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    SimulatorCommands::Pause(p)             => pauses = p,
                    SimulatorCommands::Select(i)            => engine.select(i),
                    SimulatorCommands::Buys { ix, notion }  => { engine.player_buy(ix, notion); }
                    SimulatorCommands::Sell { ix, notion }  => { engine.player_sell(ix, notion); }
                    SimulatorCommands::SetKnobs(kb)         => engine.knobs = kb,
                    SimulatorCommands::Save => {
                        if let Ok(bytes) = rkyv::to_bytes::<rkyv::rancor::Error>(&engine.saver()) {
                            let _ = tx.try_send(SimulatorResponsiveness::Saved(bytes.to_vec()));
                        }
                    }
                    SimulatorCommands::Load(bytes) => {
                        if let Ok(save) = rkyv::from_bytes::<SaveGameState, rkyv::rancor::Error>(&bytes) {
                            engine = EconomyEngine::loads(save);
                        }
                    }
                    SimulatorCommands::Stop                 => stop.store(true, Ordering::Relaxed),
                }
            }

            let speed = engine.knobs.speed.clamp(0.1, 8.8);
            let steps = if speed >= 1.0 { speed.round() as u32 } else { 1 };
            for _ in 0..steps { if !pauses { engine.step(); } }

            if last_case.elapsed() >= Duration::from_millis(FORCE_SNAPSHOT_MS) {
                let _ = tx.try_send(SimulatorResponsiveness::Snapshot(Box::new(engine.snapshot())));
                last_case = Instant::now();
            }

            let slow = if speed < 1.0 { 1.0 / speed } else { 1.0 };
            let budget = Duration::from_millis((STEP_BUDGET_MS as f32 * slow) as u64);
            if let Some(rest) = budget.checked_sub(loop_init.elapsed()) {
                thread::sleep(rest); // pace is set here -- not in step() !!!
            }
        }
}

struct TraderLedger { line: String}

struct MarketDynamo {
    rx:                 Receiver<SimulatorResponsiveness>,
    tx:                 Sender<SimulatorCommands>,
    recent:             Option<SnapshotEconomy>,
    save_blob:          Option<Vec<u8>>,
    // ui state
    search:             String,
    selected:           usize,
    order_notional:     f64,
    npc_focus:          Option<usize>,
    paused:             bool,
    recent_trades:      VecDeque<TraderLedger>,
    // sandboxxing -- alter those preset simulation values present in 'knobs.rs'
    knobs:              Knobs,
    theme:              Theme,
    font_present:        bool,


}

impl MarketDynamo {
    fn push_trade(&mut self, s: String) {
        self.recent_trades.push_front(TraderLedger { line: s });
        while self.recent_trades.len() > 12 { self.recent_trades.pop_back(); }
    }
}

fn arrow_direction(ui: &mut Ui, perc: f32, up: Color32, down: Color32) {
    let (arrow, col) = if perc >= 0.0
        {       ("▲", up) }
        else {  ("▼", down) };
        //{       (FontIcons::UP_ARROW.to_string()) }
        //else {  (FontIcons::DOWN_ARROW.to_string()) };
        //{       ("Λ", up) }
        //else {  ("V", down) };
        //{       ("▲", up) }
        //else {  ("▼", down) };
    ui.colored_label(col, format!(" {arrow} {:+.2}%", perc));
}

fn money(v: f64) -> String {
    let (n, suf) = match v.abs() {
        x if x >= 1e18 => (v / 1e18, "Qi."),
        x if x >= 1e15 => (v / 1e15, "Q."),
        x if x >= 1e12 => (v / 1e12, "T."),
        x if x >= 1e9  => (v / 1e9,  "B."),
        x if x >= 1e6  => (v / 1e6,  "M."),
        _              => (v, ""),
    };
    format!("{TYPE_CURRENCY}{n:.3}{suf}")
}

fn setup_fonts(ctx: &egui::Context, path: &str, case: &str) {
    let font_data = fs::read(path).expect("");
    //let mut fonts = ctx.fonts().clone();
    //let mut fonts = ctx.fonts(|f| f.clone());
    let mut fonts = egui::FontDefinitions::default();

    fonts.font_data.insert(
        case.to_owned(),
        Arc::new(egui::FontData::from_owned(font_data)),
    );

    //include_bytes!("../fetch/font/notosans/NotoSans-Regular.ttf")).into());

    fonts.families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, case.to_owned());

    ctx.set_fonts(fonts);
}

fn chirons_marquee(ui: &mut Ui, cells: &[(String, Color32)], t: f64, pixels_shunt: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), TALL_CHIRON), Sense::hover());
    let paints = ui.painter_at(rect);
    let font = FontId::proportional(FONT_CHIRON);
    let with_gap = 28.0;
    let widths: Vec<f32> = cells.iter()
        .map(|(txt, _)| ui.fonts(|f| f.layout_no_wrap(txt.clone(), font.clone(), Color32::WHITE).size().x))
        .collect();
    let total = ((CELL_CHIRON + ((CELL_CHIRON * CELL_CRAWLS)/CELL_CRAWLS)) - CELL_CRAWLS)
        * cells.len() as f32 + (1.0 / CELL_CRAWLS);
    /*let total: f32 = widths.iter().sum::<f32>()
        + with_gap
        * cells.len() as f32;*/
    if total <= 0.0 { return; }

    let offset = (t as f32 * pixels_shunt).rem_euclid(total);
    for pass in 0..2 {
        let mut x = rect.left()
            - offset
            + pass as f32
            * total;
        for (txt, col) in cells { // for (i, (txt, col)) in cells.iter().enumerate()
            //if x + CELL_CHIRON /*widths[i]*/ >= rect.left() && x <= rect.right() {
            paints
                .text(pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                txt,
                font.clone(),
                *col);
            //}
            x += (CELL_CHIRON + ((CELL_CHIRON * CELL_CRAWLS)/CELL_CRAWLS)) - CELL_CRAWLS; /*widths[i] + with_gap;*/
        }
    }
}

impl eframe::App for MarketDynamo {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        //let font_loaded = false;
        if !self.font_present {
            //setup_fonts(ctx, "fetch/font/liberationsans/LiberationSans-Regular.ttf", "liberationsans");
            setup_fonts(ctx, "fetch/font/liberationsans/LiberationSans-Bold.ttf", "liberationsansbold");
            //setup_fonts(ctx, "fetch/font/notosans/NotoSans-Regular.ttf", "notosans");
            //setup_fonts(ctx, "fetch/font/timesnewroman/times.ttf", "times");
            //setup_fonts(ctx, "fetch/font/arial/arial.ttf", "arial");
            self.font_present = true;
        }

        //let mut vis  = egui::Visuals::dark();
        //vis.override_text_color = Some(Color32::from_rgb(202, 189, 198));
        //ctx.set_visuals(vis);

        self.theme.apply(ctx);
        let palette = self.theme.palette();
        let mut deconflict_trade_state_from_ui: Vec<String> = Vec::new();

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                SimulatorResponsiveness::Snapshot(s) => self.recent = Some(*s),
                SimulatorResponsiveness::Saved(bytes) => {
                    let n = bytes.len();
                    self.save_blob = Some(bytes);
                    self.push_trade(format!("saved {n} bytes"));
                }
            }
        }

        egui::TopBottomPanel::top("chirons").show(ctx, |ui| {
            if let Some(s) = &self.recent {
                let t = ctx.input(|i| i.time);
                for m in 0..s.markets.len().min(EXCHANGES) {
                    ui.horizontal(|ui| {
                        //ui.strong(format!(" {}: ", s.markets[m].0));
                        let cells: Vec<(String, Color32)> = s.rows.iter()
                            .filter(|r| r.market_id as usize == m).take(COUNT_CHIRON)
                            .map(|r| (format!(" {} {:>9.2} {:>+7.1}% ",
                                r.nameplate,
                                r.price,
                                r.perc),
                                if r.perc >= 0.0 { palette.up } else { palette.down }))
                                .collect();
                        ui.add_sized([MARKET_NAMES_X, MARKET_NAMES_Y], egui::Label::new(
                            egui::RichText::new(&s.markets[m].0).strong()));
                        chirons_marquee(ui, &cells, ctx.input(|i| i.time), CELL_CRAWLS);


                        /*let mut cells: Vec<&StockRow> =
                            s.rows.iter().filter(|r| r.market_id as usize == m).take(19).collect();*/
                        /*let shift = ((t * 0.3336) as usize) % cells.len().max(1);
                        cells.rotate_left(shift);
                        for r in cells.iter().take(14) {
                            //arrow_direction(ui, r.perc, palette.up, palette.down);
                            let col = if r.perc >= 0.0 { COLOR_UPWARDS } else { COLOR_DOWNWARDS };
                            ui.colored_label(col, format!(" {}  {:.2} {:+.1} %",
                                r.nameplate,
                                r.price,
                                r.perc));
                        }*/
                    });
                }
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    if ui.button(if self.paused { ">> Resume" } else { "II Paused"}).clicked() {
                        self.paused = !self.paused;
                        let _ = self.tx.send(SimulatorCommands::Pause(self.paused));
                    }
                    if ui.button("$ Save").clicked() { let _ = self.tx.send(SimulatorCommands::Save); }
                    if ui.button("# Load").clicked() {
                        if let Some(b) = self.save_blob.clone() {
                            let _ = self.tx.send(SimulatorCommands::Load(b));
                        }
                    }
                    ui.separator();
                    let open = if s.market_open { " OPEN " } else { " CLOSED " };
                    ui.colored_label(if s.market_open { COLOR_UPWARDS } else { COLOR_DOWNWARDS }, open);
                    ui.label(format!("|  Day: {} |  Tick: {} |  {}",
                        s.day + 1,
                        s.tick,
                        s.flow.label()));
                });
            } else {
                ui.label("..., .., ., running...");
            }
        });

        egui::SidePanel::left("stocklist").resizable(true).default_width(128.0).show(ctx, |ui| {
            //ui.heading("listing");
            ui.horizontal(|ui| {
                ui.label("@= ");
                ui.text_edit_singleline(&mut self.search);
            });
            ui.separator();
            if let Some(s) = &self.recent {
                let q = self.search.to_ascii_uppercase();
                let filtered: Vec<&StockRow> = s.rows.iter()
                    .filter(|r| q.is_empty() || r.nameplate.contains(&q)).collect();
                ui.label(format!("{} | {}", filtered.len(), s.rows.len()));
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    for r in filtered {
                        ui.horizontal(|ui| {
                            let sel = self.selected == r.ix;
                            if ui.selectable_label(sel, format!("{:<6} {:>9.2}", r.nameplate, r.price)).clicked() {
                                self.selected = r.ix;
                                let _ = self.tx.send(SimulatorCommands::Select(r.ix));
                            }
                            arrow_direction(ui, r.perc, palette.up, palette.down);
                        });
                    }
                });
            }
        });

        egui::TopBottomPanel::bottom("dock").resizable(true).default_height(256.0).show(ctx, |ui| {
            if let Some(s) = &self.recent {
                ui.columns(5, |col| {
                    let mc = &s.macro_state;
                    // (2) yield curve mini-plot, inverts before recession
                    col[0].heading("YIELD CURVE"); // "yield curve"
                    let plpt: PlotPoints = mc.yield_curve
                        .iter()
                        .enumerate()
                        .map(|(i, &y)| [i as f64, (y * 100.0) as f64]).collect();
                    Plot::new("curve").height(150.0).show_axes([false, true]).show(&mut col[0], |pu| {
                        pu.line(Line::new(plpt));
                        //pu.line(Line::new("yield", plpt));
                    });

                    col[1].heading("ECONOMY");
                    col[1].label(format!("Policy Rate:     {:.2}% ",       mc.short_rate * 100.0));
                    col[1].label(format!("10yr Yield:      {:.2}% ",       mc.long_rate * 100.0));
                    col[1].label(format!("Inflation:       {:.2}% ",       mc.inflation * 100.0));
                    col[1].label(format!("Output Gap:      {:+.2}% ",      mc.output_gap * 100.0));
                    col[1].label(format!("Money Supply:    {} ",           money(mc.money_supply)));
                    col[1].label(format!("Market Cap:      {} ",           money(s.total_market_cap)));
                    col[1].label(format!("Volume / tick:   {} ",           money(s.total_volume)));

                    //col[2].heading("");
                    /*if let Some((name, series)) = &s.selected {

                        //ui.horizontal(|ui| {
                            ui.add(egui::DragValue::new(&mut self.order_notional)
                                .speed(1000.0)
                                .prefix(TYPE_CURRENCY)
                                .range(0.0..=1e15));
                            if ui.button("BUY").clicked() {
                                let _ = self.tx.send(SimulatorCommands::Buys { ix: self.selected, notion: self.order_notional });
                                deconflict_trade_state_from_ui.push(format!("Bought '{name}' {}", money(self.order_notional)));
                            }
                            if ui.button("SELL").clicked() {
                                let _ = self.tx.send(SimulatorCommands::Sell { ix: self.selected, notion: self.order_notional });
                                deconflict_trade_state_from_ui.push(format!(" Sold  '{name}' {}", money(self.order_notional)));
                            }
                    }
                            //});
                    //}*/


                    col[3].heading("PERS. LEDGER");
                    col[3].label(format!("Net Worth     {}",            money(s.player_cash)));
                    col[3].label(format!("Equity        {}",            money(s.player_equity)));
                    let pnl = s.player_equity - PLAYER_WALLET_BEGINS;
                    col[3].colored_label(if pnl >= 0.0 { COLOR_UPWARDS } else { COLOR_DOWNWARDS },
                        format!("Wager      {}", money(pnl)));
                    //col[2].label("POSITION");
                    //col[2].separator();
                    for t in self.recent_trades
                        .iter()
                        .take(16) {
                            col[2].small(&t.line);
                        }

                    col[4].heading("BROKERS");
                    egui::ScrollArea::vertical()
                        .id_salt("npcscroll")
                        .max_height(180.0)
                            .show(&mut col[4], |ui| {
                            for (i, n) in s.npcs.iter().enumerate() {
                                let sel = self.npc_focus == Some(i);
                                if ui.selectable_label(sel, format!("{:<8} {}", n.name, money(n.equity))).clicked() {
                                    self.npc_focus = Some(i);
                                }
                                if sel { ui.small(format!("Strategy:   {:?}", n.strategy)); }
                            }
                    });
                });

                ui.separator();
                ui.horizontal_wrapped(|ui| {
                    //ui.label("&= ");
                    ui.spacing_mut().slider_width = 60.0;
                    let mut changed = false;
                    for (label, val, lo, hi) in self.knobs.sliders() {
                        ui.label(label);
                        changed |= ui.add(egui::Slider::new(val, lo..=hi)).changed();
                    }
                    if ui.button("rush").clicked() { self.knobs = Knobs::rush(); changed = true; }
                    if ui.button("quiet").clicked() { self.knobs = Knobs::boring(); changed = true; }
                    if ui.button("casino").clicked() { self.knobs = Knobs::casino(); changed = true; }
                    if ui.button("default").clicked() { self.knobs = Knobs::default(); changed = true; }
                    if changed { let _ = self.tx.send(SimulatorCommands::SetKnobs(self.knobs)); }
                });
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(s) = &self.recent {
                if let Some((name, series)) = &s.selected {
                    ui.horizontal(|ui| {
                        ui.heading(name);
                        /*ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),
                            |ui| {
                                if ui.button("X").clicked() {
                                    std::process::exit(0);
                                }
                            });
                        ui.interact(
                            ui.max_rect(),
                            ui.id().with("T1XR"),
                            egui::Sense::drag(),
                        );
                        ui.separator();*/
                        if let Some(r) = s.rows.iter().find(|r| r.ix == self.selected) {
                            ui.label(format!("{:.2}", r.price));
                            arrow_direction(ui, r.perc, palette.up, palette.down);
                        }
                    });



                    let pointers: PlotPoints = series
                        .iter()
                        .enumerate()
                        .map(|(i, &v)| [i as f64, v as f64]).collect();
                    Plot::new("plot")
                        .view_aspect(PLOT_ASPECT_RATIO)
                        //.show_axes([true, true])
                        //.show_grid([true, true])
                        .allow_scroll(true)
                        .allow_drag(true)
                        .allow_zoom(true)
                        .label_formatter(|n, v| format!("{n}: {:.2}", v.y))
                        .show(ui, |pu| {
                            pu.line(Line::new(pointers)
                            //pu.line(Line::new(name.clone(), pointers)
                                .color(palette.accent)
                                .width(1.3)
                                .fill(0.11));
                        });

                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.order_notional)
                            .speed(1000.0)
                            .prefix(TYPE_CURRENCY)
                            .range(0.0..=1e15));
                        if ui.button("BUY").clicked() {
                            let _ = self.tx.send(SimulatorCommands::Buys { ix: self.selected, notion: self.order_notional });
                            deconflict_trade_state_from_ui.push(format!("Bought '{name}' {}", money(self.order_notional)));
                        }
                        if ui.button("SELL").clicked() {
                            let _ = self.tx.send(SimulatorCommands::Sell { ix: self.selected, notion: self.order_notional });
                            deconflict_trade_state_from_ui.push(format!(" Sold  '{name}' {}", money(self.order_notional)));
                        }
                        if !s.market_open { ui.colored_label(COLOR_DOWNWARDS, " MARKET CLOSED "); }
                    });
                }
            } else {
                ui.centered_and_justified(|ui| ui.label("Starting Up..."));
            }
        });

        for line in deconflict_trade_state_from_ui { self.push_trade(line); }
        ctx.request_repaint(); // to animate the chirons and live feeds
    }
}





fn main() -> eframe::Result<()> {
    let dot_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let seed = SeedMarketTheory {
        q_high: (dot_time as u64).wrapping_mul(QUOTIENT_HIGH),
        q_low: ((dot_time >> 64) as u64) ^ QUOTIENT_LOW,
    };

    let (tx_resp, rx_resp) = bounded::<SimulatorResponsiveness>(PRESET_MAX_QUEUE);
    let (tx_cmd, rx_cmd) = bounded::<SimulatorCommands>(64);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_c = stop.clone();
    thread::spawn(move || simulation_thread(seed, rx_cmd, tx_resp, stop_c));

    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WINDOW_SCALE_WIDTHS, WINDOW_SCALE_HEIGHT])
            .with_decorations(true)
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native("T1XR", native, Box::new(move |_cc| {
        //_cc.egui_ctx.set_fonts(fonts);
        Ok(Box::new(MarketDynamo {
            rx: rx_resp,
            tx: tx_cmd,
            recent: None,
            save_blob: None,
            search: String::new(),
            selected: 0,
            order_notional: 10_000.0,
            npc_focus: None,
            paused: false,
            recent_trades: VecDeque::new(),
            knobs: Knobs::default(),
            theme: Theme::default(),
            font_present: false,
        }))
    }))
}
