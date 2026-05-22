/*
RUNNING BUG LIST:
- BUG #1 (Logic): INDEX_MATRIX used ^ (XOR) instead of exponentiation. (Line 24)
- BUG #2 (Crash): samples() would index out of bounds if INDEX_MATRIX < USER_INTERFACE_SAMPLING. (Line 230)
- BUG #3 (Math): fat_tail_noise Box-Muller transform was missing .sqrt(), breaking the normal distribution. (Line 214)
- BUG #4 (Math): Mean reversion was subtracting a value from itself (log_price - price.ln()), resulting in 0. (Line 167)
- BUG #5 (Concurrency): Sim thread panics were silent. Added basic error handling.
*/
// BUG LOG / TODO
// 1) Verify plotting backend choice on the target OS (egui_plot vs custom painter).
// 2) Tune the crash hazard so it feels plausible, not theatrical.
// 3) Add file format versioning before shipping save/load.
// 4) Confirm seed-to-state determinism across platforms with endianness tests.
// 5) Add backpressure when UI falls behind the simulation thread.
//
//
//! - 4096 stocks
//! - deterministic from a 128-bit seed
//! - two-thread design: simulation thread + GUI thread
//! - compact save-state / replay-friendly



mod values;
mod engine;

use crate::values::*;
use crate::engine::*;

use std::collections::VecDeque;
//use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, Receiver, Sender};
use eframe::{egui, egui_glow};
use egui::*;
//use egui::{Color32, RichText};
use egui_plot::{Line, Plot, PlotPoints};
use rand::prelude::*;
use rand_chacha::ChaCha8Rng;
use rkyv::{Archive, Deserialize, Serialize};
use winit::event_loop;
use winit::monitor;
use winit::{
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::Window,
};
use std::error::Error;
use rancor;
//use serde::{Archive, Deserialize, Serialize};
//use postcard;

// makes the ticker name scale (36**3..5, i.e. 'X9Z', 'TSL4', or 'YYYG7')
pub fn form_ticker_nameplate(rng: &mut ChaCha8Rng) -> String {
    let len = rng.random_range(3..=5);
    let charset = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    (0..len).map(|_| {
        let index = rng.random_range(0..charset.len());
        charset.chars().nth(index).unwrap()
    }).collect()
}

#[derive(Debug)]
enum SimulatedResponse {
    Snapshot(SnapshotEconomy),
    SaveNow(Vec<u8>),
}

pub struct StockComplex {
    rx: Receiver<SimulatedResponse>,
    tx_cmd: Sender<SimulatedCommand>,
    recent: Option<SnapshotEconomy>,
    save_blob: Option<Vec<u8>>,
    index_picker: usize,
    pause_active: bool,
}

#[derive(Debug)]
enum SimulatedCommand {
    ToPause(bool),
    ToSave,
    ToStop,
}

impl eframe::App for StockComplex {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        //let mut vis = egui::Visuals::light();
        let mut vis = egui::Visuals::dark();
        vis.override_text_color = Some(Color32::from_rgb(201, 189, 198));
        //vis.override_text_color = Some(Color32::from_rgb(22, 22, 21));
        vis.widgets.noninteractive.bg_fill = Color32::from_rgb(200, 136, 128);
        ctx.set_visuals(vis);

        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                SimulatedResponse::Snapshot(s) => self.recent = Some(s),
                SimulatedResponse::SaveNow(bytes) => self.save_blob = Some(bytes),
            }
        }

        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button(if self.pause_active { "resume" } else { "pause" }).clicked() {
                    self.pause_active = !self.pause_active;
                    let _ = self.tx_cmd.send(SimulatedCommand::ToPause(self.pause_active));
                }
                if ui.button("save now").clicked() {
                    let _ = self.tx_cmd.send(SimulatedCommand::ToSave);
                }
                ui.label(" o ");
                if let Some(s) = &self.recent {
                    ui.separator();
                    ui.label(format!("{:?}", s.seed));
                    ui.label(format!("tick: {}", s.tick));
                    ui.label(format!("heat: {:.3}", s.driver));
                    ui.label(format!("flow: {:?}", s.flow));
                }
            });
        });

        egui::SidePanel::left("left").resizable(true).show(ctx, |ui| {
            ui.heading("indexing");
            if let Some(s) = &self.recent {
                for (i, (name, value)) in s.index.iter().enumerate() {
                    if ui.selectable_label(
                        self.index_picker == i,
                        format!("{} {:.2}", name, value)).clicked() {
                            self.index_picker = i;
                    }
                }
            }

            ui.separator();
            if let Some(blob) = &self.save_blob {
                ui.label(format!("recent save blob? = {} bytes", blob.len()));
            }
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            if let Some(s) = &self.recent {
                //ui.heading("sample price");
                if let Some((name, series)) = s.samples
                    .get(self.index_picker.min(s.samples.len().saturating_sub(1))) {
                        ui.label(format!("{}", name));
                    }

                    let plot = Plot::new("plot_main")
                        .view_aspect(PLOT_ASPECT_RATIO)
                        //.view_aspect(2.4)
                        .show_axes([true, true])
                        .show_grid([true, true])
                        .allow_scroll(true)
                        .allow_drag(true)
                        .allow_zoom(true)
                        .label_formatter(|name, value| format!("{}: {:.2}", name, value.y))
                        .set_margin_fraction(egui::Vec2::new(0.02, 0.02));

                    plot.show(ui, |plot_ui| {
                        for (name, series) in s.samples.iter().take(1) {
                            let points: PlotPoints = series
                                .iter()
                                .enumerate()
                                .map(|(i, &v)| [i as f64, v as f64]).collect();

                            //let line = Line::new(name.clone())
                            plot_ui.line(Line::new(name.clone(), points)
                                .color(COLOR_LINE_ACCENT)
                                .width(1.334)
                                .fill(0.1));
                            //plot_ui.line(Line::new(name.clone(), points));
                        }
                    });

                    ui.separator();
                    ui.label("stock grid picker");
                    egui::Grid::new("gridx4").num_columns(4).show(ui, |ui| {
                        for (work_index, st) in s.stocks.iter().take(16).enumerate() {
                            ui.label(format!("-> {}", work_index + 1));
                            ui.label(format!("{:.2}", st.price));
                            ui.label(format!("r={:+.3}", st.last_value * 100.0));
                            ui.label(format!("v={:.3}", st.vol));
                            ui.end_row();
                        }
                    });
                } else {
                    ui.label("getting 1st snapshot...");
                    /*for i in 0..4 {
                        ui.label("getting 1st snapshot...");
                        println!("\t at {}", i);
                        thread::sleep(Duration::from_millis(500));
                    }*/
            }
        });

        ctx.request_repaint();
    }
}

fn sim_thread(
    seed: SeedMarketTheory,
    rx_cmd: Receiver<SimulatedCommand>,
    tx: Sender<SimulatedResponse>,
    stop: Arc<AtomicBool>) {
    let mut engine = EconomyEngine::new(seed);
    let mut pause_game = false;
    let mut latest_image = Instant::now();

    let tick_rate_factor = Duration::from_millis(FORCE_TICK_RATE); // 16 = ~62.5 t/s

    while !stop.load(Ordering::Relaxed) {
        while let Ok(cmd) = rx_cmd.try_recv() {
            match cmd {
                SimulatedCommand::ToPause(p) => pause_game = p,
                SimulatedCommand::ToSave => {
                    let save = engine.saver();
                    if let Ok(bytes) =
                        rkyv::to_bytes::<rkyv::rancor::Error>(&save) {
                            let _ = tx
                                .try_send(SimulatedResponse::SaveNow(
                                    bytes.to_vec()
                                )
                            );
                    }
                    /*let bytes = match rkyv::to_bytes::<rancor::Error>(&save) {
                        Ok(v) => v,
                        Err(e) => {
                            println!("{e}");
                            return;
                        }
                    };*/
                    //std::fs::write("save.plot", &bytes[..]);

                    /*let archiver = match rkyv::access::<ArchivedSaveGameState,
                        rkyv::rancor::Error>(&bytes[..]);*/
                    /*if let Ok(bytes) = postcard::to_allocvec(save,
                        postcard::standard()) {
                            let _ = tx.send(SimulatedResponse::SaveNow(bytes));
                    }}*/
                }
                SimulatedCommand::ToStop => {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        }

        if !pause_game {
            engine.step();
        }

        if latest_image.elapsed() >= Duration::from_millis(FORCE_TICK_RATE) {
            let _ = tx.send(SimulatedResponse::Snapshot(engine.snapshot()));
            latest_image = Instant::now();
        }
        thread::sleep(tick_rate_factor);
    }
}

fn main() -> eframe::Result<()> {
    let seed = SeedMarketTheory { qlo: QUOTIANT_LO, qhi: QUOTIANT_HI};

    let (tx_msg, rx_msg) = bounded::<SimulatedResponse>(PRESET_MAX_QUEUE);
    let (tx_cmd, rx_cmd) = bounded::<SimulatedCommand>(PRESET_MAX_QUEUE);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = stop.clone();

    thread::spawn(move || sim_thread(seed, rx_cmd, tx_msg, stop_clone));

    //let event_loop = EventLoop::new().unwrap();
    //eframe::winit::event_loop::EventLoop::primary_monitor;
    //let monitor = event_loop..unwrap();
    //let scale_window = monitor.size();

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_drag_and_drop(true),..Default::default()
    };

    eframe::run_native(
        "T1XR",
        native_options,
        Box::new(move |_cc| {
            Ok(Box::new(StockComplex {
                rx: rx_msg,
                tx_cmd,
                recent: None,
                save_blob: None,
                index_picker: 0,
                pause_active: false,
            }))
        }),
    )
}
