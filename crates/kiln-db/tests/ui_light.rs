//! 라이트 테마 스냅샷: 패널 트리, 테이블 뷰, 콘솔, 연결 대화상자.
//! 테마는 프로세스 전역이므로 이 파일은 테스트 하나에서 순서대로 그린다.

mod common;

use common::sqlite_fixture;
use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_db::{DbManager, DbPanel, DbTab};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn save_png<S>(h: &mut Harness<'_, S>, name: &str) {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&d).unwrap();
    match h.render() {
        Ok(img) => {
            let p = d.join(format!("{name}.png"));
            img.save(&p).unwrap();
            eprintln!("snapshot: {}", p.display());
        }
        Err(e) => eprintln!("render unavailable ({e}); skipping snapshot {name}"),
    }
}

fn step_until<S>(h: &mut Harness<'_, S>, what: &str, mut cond: impl FnMut(&mut Harness<'_, S>) -> bool) {
    let start = Instant::now();
    loop {
        h.step();
        if cond(h) {
            for _ in 0..3 {
                h.step();
            }
            return;
        }
        assert!(start.elapsed() < Duration::from_secs(15), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn click<S>(h: &mut Harness<'_, S>, p: egui::Pos2) {
    h.hover_at(p);
    for _ in 0..10 {
        h.step();
    }
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        h.step();
    }
}

/// 테마와 글꼴을 적용한다. 글꼴이 아직 준비되지 않은 프레임이면 `false`.
fn themed(ctx: &egui::Context) -> bool {
    kiln_common::Theme::current().apply(ctx);
    common::install_korean_font(ctx)
}

#[test]
fn light_theme_panels_render() {
    kiln_common::Theme::set_current("kiln-light");
    let (_d, m, id) = sqlite_fixture();

    // 패널 트리.
    let mut h = Harness::builder()
        .with_size(vec2(320.0, 480.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, p: &mut DbPanel| {
                if !themed(ui.ctx()) {
                    return;
                }
                let _ = p.ui(ui);
            },
            DbPanel::new(m.clone()),
        );
    h.step();
    h.get_by_label("shop.db").click();
    step_until(&mut h, "tables", |h| h.query_by_label("customers").is_some());
    let r = h.get_by_label("customers").rect();
    click(&mut h, egui::pos2(r.min.x + 50.0, r.center().y));
    step_until(&mut h, "columns", |h| h.query_by_label("email").is_some());
    h.hover_at(egui::pos2(-10.0, -10.0));
    h.step();
    save_png(&mut h, "light_panel_tree");
    drop(h);

    // 빈 패널과 연결 대화상자.
    let mut h = Harness::builder()
        .with_size(vec2(900.0, 640.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, p: &mut DbPanel| {
                if !themed(ui.ctx()) {
                    return;
                }
                egui::Panel::left("left").exact_size(300.0).show(ui, |ui| {
                    let _ = p.ui(ui);
                });
            },
            DbPanel::new(DbManager::in_memory()),
        );
    h.step();
    save_png(&mut h, "light_panel_empty");
    h.get_by_label("PostgreSQL").click();
    for _ in 0..4 {
        h.step();
    }
    save_png(&mut h, "light_connection_dialog");
    drop(h);

    // 테이블 뷰.
    let tab = DbTab::table(m.clone(), id, Some("main".into()), "customers".into());
    let mut h = Harness::builder()
        .with_size(vec2(1100.0, 460.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, t: &mut DbTab| {
                if !themed(ui.ctx()) {
                    return;
                }
                t.ui(ui);
            },
            tab,
        );
    step_until(&mut h, "rows", |h| h.query_by_label_contains("총 12").is_some());
    let hdr = h.get_by_label("email").rect();
    let p = egui::pos2(hdr.center().x, hdr.max.y + 26.0 + 13.0);
    click(&mut h, p);
    h.hover_at(egui::pos2(-10.0, -10.0));
    h.step();
    save_png(&mut h, "light_table_view");
    h.get_by_label("컬럼").click();
    for _ in 0..3 {
        h.step();
    }
    assert!(h.query_by_label("VARCHAR(120)").is_some());
    save_png(&mut h, "light_table_structure");
    h.get_by_label("인덱스").click();
    step_until(&mut h, "indexes", |h| h.query_by_label("idx_customers_name").is_some());
    assert!(h.query_by_label("고유").is_some());
    save_png(&mut h, "light_table_indexes");
    let mut orders = DbTab::table(m.clone(), id, Some("main".into()), "orders".into());
    orders.show_table_section(kiln_db::TableSection::Structure);
    *h.state_mut() = orders;
    step_until(&mut h, "foreign key metadata", |h| h.query_by_label_contains("customers.id").is_some());
    assert!(h.query_by_label("외래 키").is_some());
    save_png(&mut h, "light_table_structure_foreign_keys");
    drop(h);

    // 콘솔.
    let tab = DbTab::console(m.clone(), id);
    let mut h = Harness::builder()
        .with_size(vec2(1100.0, 560.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, t: &mut DbTab| {
                if !themed(ui.ctx()) {
                    return;
                }
                t.ui(ui);
            },
            tab,
        );
    h.step();
    h.get_by_role(egui::accesskit::Role::MultilineTextInput).click();
    h.step();
    h.event(egui::Event::Text(
        "SELECT c.name, c.balance, o.total, o.payload\nFROM customers c JOIN orders o ON o.customer_id = c.id\nORDER BY o.total DESC;".into(),
    ));
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    step_until(&mut h, "result", |h| h.query_by_label_contains("결과 1").is_some());
    h.hover_at(egui::pos2(-5.0, -5.0));
    h.step();
    save_png(&mut h, "light_console");
    kiln_common::Theme::set_current("kiln-dark");
}
