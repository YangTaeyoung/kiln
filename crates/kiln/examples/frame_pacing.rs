//! Native Metal pacing probe using synthetic terminal content; no daemon or user configuration.
use eframe::{App, Frame};
use egui::{Event, Rect, pos2, vec2};
use kiln::app::{
    conn::{Conn, Screen},
    terminal::{TermSettings, TermView},
};
use kiln_proto::{Cell, Line};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

struct Probe {
    views: [TermView; 3],
    conn: Conn,
    rx: mpsc::Receiver<Instant>,
    delays: Vec<f64>,
    frames: Vec<f64>,
    last_frame: Option<Instant>,
    first: bool,
    output: String,
}
impl App for Probe {
    fn raw_input_hook(&mut self, _: &egui::Context, input: &mut egui::RawInput) {
        input.events.clear();
        input.focused = true;
        for at in self.rx.try_iter() {
            self.delays.push(at.elapsed().as_secs_f64() * 1000.0);
            input.events.push(Event::Text("x".into()));
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut Frame) {
        let now = Instant::now();
        if !self.delays.is_empty() {
            if let Some(last) = self.last_frame {
                self.frames
                    .push(now.duration_since(last).as_secs_f64() * 1000.0);
            }
        }
        self.last_frame = Some(now);
        let area = ui.available_rect_before_wrap();
        for index in 0..3 {
            let width = area.width() / 3.0;
            let rect = Rect::from_min_size(
                area.min + vec2(width * index as f32, 0.0),
                vec2(width - 4.0, area.height()),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).id_salt(index));
            self.views[index].ui(
                &mut child,
                &mut self.conn,
                &TermSettings::default(),
                self.first && index == 0,
                None,
            );
        }
        self.first = false;
    }
    fn on_exit(&mut self) {
        fn percentile(values: &mut [f64], p: f64) -> f64 {
            values.sort_by(f64::total_cmp);
            values
                .get(((values.len().saturating_sub(1)) as f64 * p).round() as usize)
                .copied()
                .unwrap_or_default()
        }
        let result = serde_json::json!({"input_events":self.delays.len(),"input_queue_p50_ms":percentile(&mut self.delays,0.5),"input_queue_p95_ms":percentile(&mut self.delays,0.95),"frame_p95_ms":percentile(&mut self.frames,0.95),"frames":self.frames.len()});
        std::fs::write(&self.output, format!("{result}\n")).unwrap();
    }
}
fn main() -> eframe::Result {
    let args: Vec<_> = std::env::args().collect();
    let surface = if args.get(1).map(String::as_str) == Some("two") {
        eframe::egui_wgpu::SurfaceConfig::HIGH_THROUGHPUT
    } else {
        eframe::egui_wgpu::SurfaceConfig::LOW_LATENCY
    };
    let output = args
        .get(2)
        .expect("frame_pacing one|two /absolute/output.json")
        .clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kiln frame pacing probe")
            .with_inner_size([1500.0, 900.0])
            .with_position(pos2(100.0, 100.0))
            .with_active(false),
        wgpu_options: eframe::egui_wgpu::WgpuConfiguration {
            surface,
            ..Default::default()
        },
        ..Default::default()
    };
    eframe::run_native(
        "Kiln frame pacing probe",
        options,
        Box::new(move |cc| {
            let mut conn = Conn::offline(cc.egui_ctx.clone());
            for sid in 1..=3 {
                let lines = (0..50)
                    .map(|_| Line {
                        cells: (0..100)
                            .map(|col| Cell {
                                c: (b'!' + (col % 90) as u8) as char,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    })
                    .collect();
                conn.screens.insert(
                    sid,
                    Screen {
                        cols: 100,
                        rows: 50,
                        lines,
                        row_versions: vec![1; 50],
                        ..Default::default()
                    },
                );
            }
            let (tx, rx) = mpsc::channel();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(1));
                for _ in 0..250 {
                    if tx.send(Instant::now()).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                    std::thread::sleep(Duration::from_millis(8));
                }
                std::thread::sleep(Duration::from_millis(100));
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            });
            Ok(Box::new(Probe {
                views: [TermView::new(1), TermView::new(2), TermView::new(3)],
                conn,
                rx,
                delays: Vec::new(),
                frames: Vec::new(),
                last_frame: None,
                first: true,
                output,
            }))
        }),
    )
}
