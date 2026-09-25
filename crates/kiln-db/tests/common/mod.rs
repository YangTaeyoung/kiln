//! UI 테스트 공용 도우미.
#![allow(dead_code)]

use kiln_db::{ConnConfig, ConnId, DbManager, Driver};

/// 번들 글꼴(Pretendard, JetBrains Mono)을 컨텍스트마다 한 번 설치한다.
/// 글꼴은 다음 프레임부터 적용되므로, 설치한 첫 프레임에는 `false` 를 돌려준다.
pub fn install_korean_font(ctx: &egui::Context) -> bool {
    let flag = egui::Id::new("kiln-test-fonts");
    if ctx.data(|d| d.get_temp::<bool>(flag)).unwrap_or(false) {
        return true;
    }
    ctx.data_mut(|d| d.insert_temp(flag, true));
    ctx.set_fonts(kiln_common::fonts::definitions(false));
    false
}

/// customers·orders 테이블과 뷰가 든 SQLite 픽스처.
pub fn sqlite_fixture() -> (tempfile::TempDir, DbManager, ConnId) {
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
