//! 터미널 렌더링 비용 측정: 200x60 화면 전체가 매 프레임 바뀌는 최악의 경우.

use egui_kittest::Harness;
use kiln::app::conn::{Conn, Screen};
use kiln::app::terminal::{TermSettings, TermView};
use kiln_proto::{Cell, Color, Frame, Line, flags};
use std::time::Instant;

fn frame(seed: u64, cols: u16, rows: u16) -> Frame {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let lines = (0..rows)
        .map(|r| {
            let cells = (0..cols)
                .map(|_| {
                    let v = next();
                    let c = (b'!' + (v % 90) as u8) as char;
                    let fg = if v % 7 == 0 { Color::Rgb((v >> 8) as u8, (v >> 16) as u8, (v >> 24) as u8) } else { Color::Idx((v % 16) as u8) };
                    let bg = if v % 11 == 0 { Color::Idx(4) } else { Color::DefaultBg };
                    Cell { c, fg, bg, flags: if v % 13 == 0 { flags::BOLD } else { 0 } }
                })
                .collect();
            (r, Line { cells, combining: vec![] })
        })
        .collect();
    Frame { session: 1, cols, rows, full: true, lines, cursor: None, mode: 0, display_offset: 0, history: 0 }
}

#[test]
fn full_screen_redraw_cost() {
    let (cols, rows) = (200u16, 60u16);
    let mut state: Option<(Conn, TermView)> = None;
    let mut seed = 0u64;
    let mut h = Harness::builder().with_size([200.0 * 8.2 + 16.0, 60.0 * 16.5 + 8.0]).build_ui(move |ui| {
        let (conn, view) = state.get_or_insert_with(|| (Conn::offline(ui.ctx().clone()), TermView::new(1)));
        seed += 1;
        let mut sc = conn.screens.remove(&1).unwrap_or_else(Screen::default);
        sc.apply(frame(seed, cols, rows));
        conn.screens.insert(1, sc);
        view.ui(ui, conn, &TermSettings::default(), false, None);
    });
    for _ in 0..5 {
        h.step();
    }
    let n = 60;
    let t = Instant::now();
    for _ in 0..n {
        h.step();
    }
    let per = t.elapsed().as_secs_f64() * 1000.0 / n as f64;
    println!("full redraw 200x60: {per:.2} ms/frame (layout + tessellation, no GPU)");
    let limit = if cfg!(debug_assertions) { 250.0 } else { 16.0 };
    assert!(per < limit, "{per} ms per frame");
}
