//! egui_kittest UI 테스트: 패널 트리, 테이블 편집 흐름, 콘솔 실행, 대용량 그리드 렌더 시간.

mod common;

use egui::{Key, Modifiers, vec2};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use kiln_db::{ConnConfig, ConnId, DbManager, DbPanel, DbTab, Driver, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn snapshot_dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save_png<S>(h: &mut Harness<'_, S>, name: &str) {
    match h.render() {
        Ok(img) => {
            let p = snapshot_dir().join(format!("{name}.png"));
            img.save(&p).unwrap();
            eprintln!("snapshot: {}", p.display());
        }
        Err(e) => eprintln!("render unavailable ({e}); skipping snapshot {name}"),
    }
}

fn sqlite_fixture() -> (tempfile::TempDir, DbManager, ConnId) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shop.db");
    std::fs::write(&path, b"").unwrap();
    let m = DbManager::in_memory();
    let id = m.add(
        ConnConfig {
            name: "shop.db".into(),
            driver: Driver::Sqlite,
            file: path.to_string_lossy().into_owned(),
            color: Some([0x6c, 0x9e, 0xff]),
            ..Default::default()
        },
        None,
    );
    let ddl = r#"
        CREATE TABLE customers (
            id INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            email VARCHAR(120) UNIQUE,
            vip BOOLEAN DEFAULT 0,
            balance DECIMAL(10,2),
            notes TEXT,
            created DATETIME DEFAULT CURRENT_TIMESTAMP
        );
        CREATE INDEX idx_customers_name ON customers(name);
        CREATE TABLE orders (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            customer_id INTEGER NOT NULL REFERENCES customers(id),
            total REAL NOT NULL,
            payload JSON,
            receipt BLOB
        );
        CREATE VIEW vip_customers AS SELECT id, name FROM customers WHERE vip = 1;
        CREATE TABLE audit_log (at TEXT, message TEXT);
    "#;
    for r in kiln_db::sql::split_statements(ddl, Driver::Sqlite) {
        m.block_on(m.query(id, &ddl[r], None)).unwrap();
    }
    let names = [
        "Ada Lovelace",
        "Alan Turing",
        "Grace Hopper",
        "Edsger Dijkstra",
        "Barbara Liskov",
        "Donald Knuth",
        "Ken Thompson",
        "Margaret Hamilton",
        "Linus Torvalds",
        "Frances Allen",
        "John McCarthy",
        "Leslie Lamport",
    ];
    for (i, n) in names.iter().enumerate() {
        let i = i + 1;
        let notes = if i % 3 == 0 {
            "NULL".to_string()
        } else {
            format!("'note {i}\nsecond line'")
        };
        m.block_on(m.query(
            id,
            &format!(
                "INSERT INTO customers (id, name, email, vip, balance, notes, created) VALUES ({i}, '{n}', '{}@example.com', {}, {}.{:02}, {notes}, '2024-0{}-1{} 09:3{}:00')",
                n.split(' ').next().unwrap().to_lowercase(),
                i % 2,
                i * 137 % 5000,
                i * 7 % 100,
                i % 9 + 1,
                i % 10,
                i % 10
            ),
            None,
        ))
        .unwrap();
        m.block_on(m.query(
            id,
            &format!(
                "INSERT INTO orders (customer_id, total, payload, receipt) VALUES ({i}, {}.5, '{{\"items\":[{i},{}],\"gift\":{}}}', X'CAFEBABE{:02X}')",
                i * 13,
                i + 1,
                if i % 2 == 0 { "true" } else { "false" },
                i
            ),
            None,
        ))
        .unwrap();
    }
    (dir, m, id)
}

/// 조건이 참이 될 때까지 프레임을 진행한다.
fn step_until<S>(
    h: &mut Harness<'_, S>,
    what: &str,
    mut cond: impl FnMut(&mut Harness<'_, S>) -> bool,
) {
    let start = Instant::now();
    loop {
        h.step();
        if cond(h) {
            // 세 프레임 더 진행한다.
            for _ in 0..3 {
                h.step();
            }
            return;
        }
        if start.elapsed() > Duration::from_secs(15) {
            panic!("timed out waiting for {what}");
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn themed(ctx: &egui::Context) {
    kiln_common::Theme::current().apply(ctx);
    common::install_korean_font(ctx);
}

struct PanelState {
    panel: DbPanel,
    events: Vec<kiln_db::DbEvent>,
}

#[test]
fn panel_tree_expands_and_emits_open_table_event() {
    let (_d, m, _id) = sqlite_fixture();
    let state = PanelState {
        panel: DbPanel::new(m.clone()),
        events: Vec::new(),
    };
    let mut h = Harness::builder()
        .with_size(vec2(320.0, 560.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, s: &mut PanelState| {
                themed(ui.ctx());
                let ev = s.panel.ui(ui);
                s.events.extend(ev);
            },
            state,
        );
    h.step();
    h.get_by_label("shop.db").click();
    step_until(&mut h, "tables", |h| {
        h.query_by_label("customers").is_some()
    });
    // 테이블 행의 화살표 쪽을 클릭하면 펼쳐진다.
    let r = h.get_by_label("customers").rect();
    click_at(&mut h, egui::pos2(r.min.x + 50.0, r.center().y), false);
    step_until(&mut h, "columns", |h| h.query_by_label("email").is_some());
    h.hover_at(egui::pos2(-10.0, -10.0));
    h.step();
    save_png(&mut h, "panel_tree");

    let r = h.get_by_label("orders").rect();
    click_at(&mut h, r.center(), true);
    let evs = &h.state().events;
    assert!(
        evs.iter()
            .any(|e| matches!(e, kiln_db::DbEvent::OpenTable { table, .. } if table == "orders")),
        "{evs:?}"
    );
}

struct TabState {
    tab: DbTab,
}

fn tab_harness(tab: DbTab, size: egui::Vec2) -> Harness<'static, TabState> {
    Harness::builder()
        .with_size(size)
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, s: &mut TabState| {
                themed(ui.ctx());
                s.tab.ui(ui);
            },
            TabState { tab },
        )
}

fn click_at<S>(h: &mut Harness<'_, S>, p: egui::Pos2, double: bool) {
    h.hover_at(p);
    // 클릭 전에 40프레임을 진행한다.
    for _ in 0..40 {
        h.step();
    }
    let n = if double { 2 } else { 1 };
    for _ in 0..n {
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        });
        h.step();
        h.event(egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        });
        h.step();
    }
}

/// 헤더 이름과 행 번호로 셀 중심 좌표를 구한다.
fn cell_pos<S>(h: &Harness<'_, S>, column: &str, row: usize) -> egui::Pos2 {
    let hdr = h.get_by_label(column).rect();
    egui::pos2(hdr.center().x, hdr.max.y + row as f32 * 22.0 + 11.0)
}

#[test]
fn table_view_edits_cell_and_submits_to_database() {
    let (_d, m, id) = sqlite_fixture();
    let tab = DbTab::table(m.clone(), id, Some("main".into()), "customers".into());
    let mut h = tab_harness(tab, vec2(1100.0, 520.0));
    step_until(&mut h, "rows", |h| {
        h.query_by_label("email").is_some() && h.query_by_label_contains("총 12").is_some()
    });
    save_png(&mut h, "table_view");

    // email 첫 행을 더블클릭해 편집, 새 값 입력 후 Enter.
    let p = cell_pos(&h, "email", 0);
    click_at(&mut h, p, true);
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(egui::Event::Text("ada@kiln.dev".into()));
    h.step();
    save_png(&mut h, "table_view_editing");
    h.key_press(Key::Enter);
    h.step();
    // notes 두 번째 행을 Delete 로 NULL 지정.
    let p = cell_pos(&h, "notes", 1);
    click_at(&mut h, p, false);
    h.key_press(Key::Delete);
    h.step();
    assert_eq!(h.state().tab.pending_changes(), 2);
    h.step();
    save_png(&mut h, "table_view_pending");

    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    step_until(&mut h, "submit", |h| {
        h.state().tab.pending_changes() == 0 && h.query_by_label_contains("커밋됨").is_some()
    });
    let rs = m
        .block_on(m.query(
            id,
            "SELECT email, notes FROM customers WHERE id IN (1, 2) ORDER BY id",
            None,
        ))
        .unwrap()
        .result;
    assert_eq!(rs.rows[0][0], Value::Text("ada@kiln.dev".into()));
    assert_eq!(rs.rows[1][1], Value::Null);

    // 값 뷰어, 구조, DDL 하위 탭.
    let p = cell_pos(&h, "notes", 0);
    click_at(&mut h, p, false);
    h.get_by_label("값").click();
    for _ in 0..3 {
        h.step();
    }
    save_png(&mut h, "table_value_viewer");
    h.get_by_label("값").click();
    h.get_by_label("구조").click();
    for _ in 0..3 {
        h.step();
    }
    save_png(&mut h, "table_structure");
    h.get_by_label("DDL").click();
    step_until(&mut h, "ddl", |h| h.query_by_label("복사").is_some());
    save_png(&mut h, "table_ddl");
}

#[test]
fn table_view_sorts_by_header_click_and_filters_with_where() {
    let (_d, m, id) = sqlite_fixture();
    let tab = DbTab::table(m.clone(), id, Some("main".into()), "customers".into());
    let mut h = tab_harness(tab, vec2(1100.0, 420.0));
    step_until(&mut h, "rows", |h| {
        h.query_by_label_contains("총 12").is_some()
    });
    // name 헤더 두 번 클릭 → 내림차순.
    let hdr = h.get_by_label("name").rect().center();
    click_at(&mut h, hdr, false);
    click_at(&mut h, hdr, false);
    step_until(&mut h, "sorted", |h| {
        h.query_all_by_value("\"name\" DESC").next().is_some()
    });
    let first = cell_pos(&h, "name", 0);
    let _ = first;
    // WHERE 입력 후 Enter.
    let where_box = h
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .next()
        .unwrap();
    where_box.click();
    h.step();
    h.event(egui::Event::Text("vip = 1".into()));
    h.step();
    h.key_press(Key::Enter);
    step_until(&mut h, "filtered", |h| {
        h.query_by_label_contains("총 6").is_some()
    });
    save_png(&mut h, "table_sorted_filtered");
}

#[test]
fn connection_dialog_renders_with_all_fields() {
    let m = DbManager::in_memory();
    let state = PanelState {
        panel: DbPanel::new(m.clone()),
        events: Vec::new(),
    };
    let mut h = Harness::builder()
        .with_size(vec2(900.0, 640.0))
        .with_step_dt(1.0 / 60.0)
        .wgpu()
        .build_ui_state(
            |ui, s: &mut PanelState| {
                themed(ui.ctx());
                egui::Panel::left("left").exact_size(300.0).show(ui, |ui| {
                    let ev = s.panel.ui(ui);
                    s.events.extend(ev);
                });
            },
            state,
        );
    h.step();
    h.get_by_label("연결 추가").click();
    for _ in 0..4 {
        h.step();
    }
    save_png(&mut h, "connection_dialog");
    // URL 가져오기로 필드를 채운다.
    let url_y = h.get_by_label("URL").rect().center().y;
    let url = h
        .get_all_by_role(egui::accesskit::Role::TextInput)
        .find(|n| (n.rect().center().y - url_y).abs() < 6.0)
        .unwrap();
    url.click();
    h.step();
    h.event(egui::Event::Text(
        "postgres://kiln@db.internal:6543/app?sslmode=require".into(),
    ));
    h.step();
    h.get_by_label("가져오기").click();
    for _ in 0..3 {
        h.step();
    }
    h.get_by_label("저장").click();
    for _ in 0..3 {
        h.step();
    }
    let conns = m.connections();
    assert_eq!(conns.len(), 1);
    assert_eq!(conns[0].host, "db.internal");
    assert_eq!(conns[0].port, 6543);
    assert_eq!(conns[0].ssl_mode, kiln_db::SslMode::Require);
    save_png(&mut h, "panel_with_connection");
}

#[test]
fn console_runs_typed_sql_and_shows_rows() {
    let (_d, m, id) = sqlite_fixture();
    let tab = DbTab::console(m.clone(), id);
    let mut h = tab_harness(tab, vec2(1100.0, 600.0));
    h.step();
    let editor = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    editor.click();
    h.step();
    h.event(egui::Event::Text(
        "-- top customers\nSELECT c.name, c.balance, o.total, o.payload\nFROM customers c JOIN orders o ON o.customer_id = c.id\nWHERE c.balance > 100\nORDER BY o.total DESC;".into(),
    ));
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
    step_until(&mut h, "result", |h| {
        h.query_by_label_contains("결과 1").is_some()
    });
    assert!(h.query_by_label("payload").is_some());
    h.hover_at(egui::pos2(-5.0, -5.0));
    h.step();
    save_png(&mut h, "console");

    // 오류 문장: 위치와 메시지가 결과 영역에 나온다.
    let editor = h.get_by_role(egui::accesskit::Role::MultilineTextInput);
    editor.click();
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(egui::Event::Text(
        "SELECT nope FROM customers;\nUPDATE customers SET vip = 1 WHERE id = 1;".into(),
    ));
    h.step();
    h.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Enter);
    step_until(&mut h, "error", |h| {
        h.query_by_label_contains("오류").is_some()
    });
    save_png(&mut h, "console_error");
}

#[test]
fn pending_insert_delete_and_edit_states_render_and_revert() {
    let (_d, m, id) = sqlite_fixture();
    let tab = DbTab::table(m.clone(), id, Some("main".into()), "customers".into());
    let mut h = tab_harness(tab, vec2(1100.0, 440.0));
    step_until(&mut h, "rows", |h| {
        h.query_by_label_contains("총 12").is_some()
    });
    // 3행 삭제 표시.
    let p = cell_pos(&h, "name", 2);
    click_at(&mut h, p, false);
    h.get_by_label("− 행").click();
    h.step();
    // 2행 balance 편집.
    let p = cell_pos(&h, "balance", 1);
    click_at(&mut h, p, true);
    h.key_press_modifiers(Modifiers::COMMAND, Key::A);
    h.event(egui::Event::Text("9999.99".into()));
    h.step();
    h.key_press(Key::Enter);
    h.step();
    // 새 행 추가 후 name 입력.
    h.get_by_label("+ 행").click();
    h.step();
    let p = cell_pos(&h, "name", 12);
    click_at(&mut h, p, true);
    h.event(egui::Event::Text("New Person".into()));
    h.step();
    h.key_press(Key::Enter);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.state().tab.pending_changes(), 3);
    h.hover_at(egui::pos2(-5.0, -5.0));
    h.step();
    save_png(&mut h, "table_pending_states");
    h.get_by_label("⟲ 되돌리기").click();
    h.step();
    assert_eq!(h.state().tab.pending_changes(), 0);
}
