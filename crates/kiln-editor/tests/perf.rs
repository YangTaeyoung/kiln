//! 큰 파일의 구문 강조·편집·프레임 시간 측정. 결과는 `--nocapture` 로 출력된다.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use kiln_editor::buffer::{Buffer, Pos};
use kiln_editor::highlight::Highlighter;
use kiln_editor::{Editor, syntax};

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
fn a_syntax_set_load_and_first_open() {
    let t = Instant::now();
    let _ = syntax::highlighter();
    let load = t.elapsed();
    let src = common::rust_source(20_000);
    let t = Instant::now();
    let ed = Editor::from_text("big.rs", &src);
    let open = t.elapsed();
    println!("PERF syntax set + theme load (once per process): {:.1} ms", ms(load));
    println!("PERF Editor::from_text 20k lines (after load): {:.2} ms", ms(open));
    assert_eq!(ed.status().language, "Rust");
}

#[test]
fn highlight_20k_line_rust_file_full_and_incremental() {
    let src = common::rust_source(20_000);
    let mut b = Buffer::from_text(&src);
    let n = b.line_count();
    assert!(n >= 20_000);
    let _ = syntax::highlighter();
    let rust = syntax::detect(Path::new("big.rs"), "");

    let t = Instant::now();
    let mut h = Highlighter::new(rust, n);
    h.ensure(b.lines(), n, None);
    let full = t.elapsed();

    // 한 글자 입력: 수렴하면 한두 줄만 다시 계산한다.
    let mut single = Duration::ZERO;
    let mut max_work = 0;
    let mut total_work = 0;
    for k in 0..50 {
        let line = 1000 + k * 300;
        let at = b.clamp(Pos::new(line, 4));
        b.insert(at, "x");
        let t = Instant::now();
        for c in b.take_changes() {
            h.on_edit(c);
        }
        h.ensure(b.lines(), n, None);
        single += t.elapsed();
        max_work = max_work.max(h.last_work);
        total_work += h.last_work;
    }
    let single_avg = single / 50;

    // 뒤쪽 전체 상태를 바꾸는 편집(블록 주석 열기) 후 화면 분량만 계산.
    b.insert(Pos::new(100, 0), "/*");
    let t = Instant::now();
    for c in b.take_changes() {
        h.on_edit(c);
    }
    h.ensure(b.lines(), 100 + 80, None);
    let visible_after_state_change = t.elapsed();
    let t = Instant::now();
    h.ensure(b.lines(), n, None);
    let rest_after_state_change = t.elapsed();

    println!("PERF lines={n}");
    println!("PERF full highlight: {:.1} ms ({:.2} us/line)", ms(full), ms(full) * 1000.0 / n as f64);
    println!("PERF single-char edit + rehighlight: avg {:.3} ms, lines recomputed avg {:.1} max {max_work}", ms(single_avg), total_work as f64 / 50.0);
    println!("PERF '/*' edit, visible window: {:.2} ms; remaining file: {:.1} ms", ms(visible_after_state_change), ms(rest_after_state_change));

    assert!(total_work <= 50 * 4, "local edits should converge, recomputed {total_work} lines");
    assert!(max_work < 100);
    assert!(single_avg < Duration::from_millis(20));
}

#[test]
fn editor_frame_time_on_20k_line_file() {
    let src = common::rust_source(20_000);
    let mut ed = Editor::from_text("big.rs", &src);
    let ctx = egui::Context::default();
    common::apply_theme(&ctx);
    let mut input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0))),
        ..Default::default()
    };
    let frame = |ed: &mut Editor, input: egui::RawInput| {
        let t = Instant::now();
        let mut out = ctx.run_ui(input, |ui| {
            ed.ui(ui);
        });
        out.textures_delta.clear();
        t.elapsed()
    };
    let first = frame(&mut ed, input.take());
    // 유휴 프레임에서 파일 전체 강조가 끝날 때까지 걸리는 프레임 수.
    let mut idle_frames = 0;
    let t = Instant::now();
    let mut idle_worst = Duration::ZERO;
    while ed.highlighted_lines() < ed.text().lines().count() && idle_frames < 2000 {
        idle_worst = idle_worst.max(frame(&mut ed, input.take()));
        idle_frames += 1;
    }
    println!(
        "PERF background highlight of whole file in idle frames: {idle_frames} frames, {:.1} ms wall, worst frame {:.2} ms",
        ms(t.elapsed()),
        ms(idle_worst)
    );
    ctx.memory_mut(|m| m.request_focus(ed.id()));
    let _ = frame(&mut ed, input.take());
    ed.goto(10_000, 1);
    let jump = frame(&mut ed, input.take());
    let mut typing = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for c in "let value = compute(42);".chars() {
        input.events.push(egui::Event::Text(c.to_string()));
        let d = frame(&mut ed, input.take());
        typing += d;
        worst = worst.max(d);
    }
    let avg = typing / 24;
    // 줄 10000 까지의 강조 따라잡기가 끝날 때까지 프레임을 돌린다.
    let mut catchup_frames = 0;
    let t = Instant::now();
    while catchup_frames < 500 && ed.highlighted_lines() < 10_050 {
        frame(&mut ed, input.take());
        catchup_frames += 1;
    }
    let catchup = t.elapsed();
    let mut typing2 = Duration::ZERO;
    for c in "let other = 1;".chars() {
        input.events.push(egui::Event::Text(c.to_string()));
        typing2 += frame(&mut ed, input.take());
    }
    let mut idle = Duration::ZERO;
    for _ in 0..20 {
        idle += frame(&mut ed, input.take());
    }
    println!("PERF highlight catch-up to line 10000: {catchup_frames} frames, {:.1} ms total", ms(catchup));
    println!("PERF editor typing frame after catch-up: avg {:.2} ms", ms(typing2 / 14));
    println!("PERF editor first frame (20k lines): {:.2} ms", ms(first));
    println!("PERF editor jump to line 10000 frame: {:.2} ms", ms(jump));
    println!("PERF editor typing frame: avg {:.2} ms, worst {:.2} ms", ms(avg), ms(worst));
    println!("PERF editor idle frame: avg {:.2} ms", ms(idle / 20));
    assert!(ed.text().contains("let value = compute(42);"));
    assert!(avg < Duration::from_millis(50));
}

/// `KILN_BENCH_DIR` 가 지정되면 그 디렉터리를 실제로 수집하고 퍼지 매칭 시간을 잰다.
#[test]
fn quick_open_index_and_match_real_tree() {
    let Ok(dir) = std::env::var("KILN_BENCH_DIR") else { return };
    use kiln_editor::fuzzy::{FileIndex, Scorer};
    let t = Instant::now();
    let idx = FileIndex::spawn(dir.into(), || {});
    while !idx.is_done() {
        std::thread::sleep(Duration::from_millis(5));
    }
    let walk = t.elapsed();
    let items: Vec<String> = (0..idx.len()).filter_map(|i| idx.get(i)).collect();
    let boxed: Vec<Box<str>> = items.into_iter().map(String::into_boxed_str).collect();
    let mut s = Scorer::default();
    let t = Instant::now();
    let r1 = s.run(1, &boxed, "s", 100, &|| false).unwrap();
    let one = t.elapsed();
    let t = Instant::now();
    let r2 = s.run(1, &boxed, "srclib", 100, &|| false).unwrap();
    let narrow = t.elapsed();
    let t = Instant::now();
    let r3 = Scorer::default().run(1, &boxed, "parserrs", 100, &|| false).unwrap();
    let fresh = t.elapsed();
    println!("PERF real tree: {} files indexed in {:.0} ms", boxed.len(), ms(walk));
    println!("PERF real tree match: 's' {:.1} ms ({}), narrowed 'srclib' {:.1} ms ({}), fresh 'parserrs' {:.1} ms ({})",
        ms(one), r1.total, ms(narrow), r2.total, ms(fresh), r3.total);
}

#[test]
fn word_wrap_folding_and_multi_cursor_on_20k_line_file() {
    // 긴 줄이 섞인 20k 줄 파일.
    let mut src = common::rust_source(20_000);
    let long = format!("    // {}\n", "lorem ipsum dolor sit amet 한글 주석 ".repeat(8));
    src = src.lines().enumerate().map(|(i, l)| if i % 10 == 0 { long.clone() } else { format!("{l}\n") }).collect();
    let mut ed = Editor::from_text("big.rs", &src);
    ed.set_word_wrap(true);
    let ctx = egui::Context::default();
    common::apply_theme(&ctx);
    let mut size = egui::vec2(1200.0, 800.0);
    let raw = |size: egui::Vec2| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };
    let frame = |ed: &mut Editor, input: egui::RawInput| {
        let t = Instant::now();
        let mut out = ctx.run_ui(input, |ui| {
            ed.ui(ui);
        });
        out.textures_delta.clear();
        t.elapsed()
    };
    let first = frame(&mut ed, raw(size));
    let second = frame(&mut ed, raw(size));
    // 창 너비가 바뀌면 전체 줄 바꿈을 다시 계산한다.
    let mut resize = Duration::ZERO;
    for k in 0..10 {
        size.x = 900.0 + k as f32 * 30.0;
        resize = resize.max(frame(&mut ed, raw(size)));
    }
    ctx.memory_mut(|m| m.request_focus(ed.id()));
    let _ = frame(&mut ed, raw(size));
    ed.goto(10_000, 1);
    let jump = frame(&mut ed, raw(size));
    let mut typing = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for c in "let wrapped = compute(42); ".chars() {
        let mut input = raw(size);
        input.events.push(egui::Event::Text(c.to_string()));
        let d = frame(&mut ed, input);
        typing += d;
        worst = worst.max(d);
    }
    let avg = typing / 27;

    let t = Instant::now();
    let n_ranges = ed.fold_ranges().len();
    let fold_compute = t.elapsed();
    let t = Instant::now();
    ed.fold_all();
    let fold_all = t.elapsed();
    let folded_frame = frame(&mut ed, raw(size));
    let t = Instant::now();
    ed.unfold_all();
    let unfold_all = t.elapsed();

    // Cmd+D 로 같은 단어 200개에 커서를 두고 입력한다.
    ed.goto(1, 1);
    ed.set_selection(kiln_editor::Selection::new(kiln_editor::Pos::new(4, 8), kiln_editor::Pos::new(4, 12)));
    for _ in 0..199 {
        ed.add_next_occurrence();
    }
    let cursors = ed.cursor_count();
    let t = Instant::now();
    ed.insert_text("x");
    let multi_edit = t.elapsed();
    let multi_frame = frame(&mut ed, raw(size));

    println!("PERF wrap: first frame {:.2} ms, second {:.2} ms, worst resize (full rewrap) frame {:.2} ms", ms(first), ms(second), ms(resize));
    println!("PERF wrap: jump to line 10000 {:.2} ms, typing frame avg {:.2} ms worst {:.2} ms", ms(jump), ms(avg), ms(worst));
    println!("PERF fold: compute {n_ranges} ranges {:.2} ms, fold all {:.2} ms, frame folded {:.2} ms, unfold all {:.2} ms", ms(fold_compute), ms(fold_all), ms(folded_frame), ms(unfold_all));
    println!("PERF multi-cursor: {cursors} cursors single keystroke edit {:.2} ms, next frame {:.2} ms", ms(multi_edit), ms(multi_frame));
    assert!(ed.text().contains("let wrapped = compute(42);"));
    assert!(avg < Duration::from_millis(50));
}
