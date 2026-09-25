//! 10만 행 × 20열 결과 그리드의 프레임 시간 측정.

mod common;

use egui::vec2;
use egui_kittest::Harness;
use kiln_db::{ColumnInfo, ConnId, DbManager, DbTab, ResultSet, Value};
use std::time::{Duration, Instant};

const ROWS: usize = 100_000;
const COLS: usize = 20;

fn big_result() -> ResultSet {
    let types = [
        "INT8",
        "TEXT",
        "FLOAT8",
        "NUMERIC",
        "BOOL",
        "TIMESTAMPTZ",
        "JSONB",
        "UUID",
        "BYTEA",
        "TEXT",
    ];
    let columns: Vec<ColumnInfo> = (0..COLS)
        .map(|c| ColumnInfo::new(format!("col_{c:02}"), types[c % types.len()]))
        .collect();
    let rows: Vec<Vec<Value>> = (0..ROWS)
        .map(|r| {
            (0..COLS)
                .map(|c| match c % types.len() {
                    0 => Value::Int(r as i64 * 7 + c as i64),
                    1 => Value::Text(format!(
                        "row {r} some longer text value that will be truncated in the grid cell {c}"
                    )),
                    2 => Value::Float(r as f64 * 0.25),
                    3 => Value::Decimal(format!("{}.{:02}", r, c)),
                    4 => Value::Bool(r % 2 == 0),
                    5 => Value::Timestamptz(format!("2024-01-{:02} 10:{:02}:00+00", r % 28 + 1, c)),
                    6 => Value::Json(format!(
                        "{{\"id\":{r},\"tags\":[\"a\",\"b\"],\"nested\":{{\"k\":{c}}}}}"
                    )),
                    7 => Value::Uuid(format!("00000000-0000-0000-0000-{:012x}", r)),
                    8 => Value::Bytes(vec![(r % 256) as u8; 24]),
                    _ => {
                        if r % 5 == 0 {
                            Value::Null
                        } else {
                            Value::Text(format!("v{r}"))
                        }
                    }
                })
                .collect()
        })
        .collect();
    ResultSet::new(columns, rows)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

#[test]
fn grid_frame_time_with_100k_rows_by_20_columns() {
    let t0 = Instant::now();
    let rs = big_result();
    let build = t0.elapsed();
    assert_eq!(rs.len(), ROWS);

    let m = DbManager::in_memory();
    let tab = DbTab::console_with_result(m, ConnId(1), "SELECT * FROM big", rs);
    let mut h = Harness::builder()
        .with_size(vec2(1600.0, 1000.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, t: &mut DbTab| {
                kiln_common::Theme::current().apply(ui.ctx());
                if common::install_korean_font(ui.ctx()) {
                    t.ui(ui);
                }
            },
            tab,
        );
    for _ in 0..5 {
        h.step();
    }
    let mut idle = Vec::new();
    for _ in 0..30 {
        let s = Instant::now();
        h.step();
        idle.push(s.elapsed());
    }
    // 스크롤하면서 매 프레임 새 행을 그린다.
    let mut scrolling = Vec::new();
    for i in 0..60 {
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, -600.0 - i as f32 * 3.0),
            modifiers: egui::Modifiers::NONE,
            phase: egui::TouchPhase::Move,
        });
        h.hover_at(egui::pos2(800.0, 700.0));
        let s = Instant::now();
        h.step();
        scrolling.push(s.elapsed());
    }
    let gpu = {
        let s = Instant::now();
        let ok = h.render().is_ok();
        (ok, s.elapsed())
    };
    if let Ok(img) = h.render() {
        let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/snapshots/grid_100k.png");
        img.save(p).unwrap();
        eprintln!("snapshot: {p}");
    }
    let idle_m = median(idle);
    let scroll_m = median(scrolling.clone());
    let scroll_max = scrolling.iter().max().copied().unwrap_or_default();
    eprintln!(
        "PERF build+format {ROWS}x{COLS}: {build:?}; idle frame median {idle_m:?}; \
         scroll frame median {scroll_m:?} (max {scroll_max:?}); wgpu render {:?} (ok={})",
        gpu.1, gpu.0
    );
    assert!(
        scroll_m < Duration::from_millis(250),
        "scroll frame too slow: {scroll_m:?}"
    );
}
