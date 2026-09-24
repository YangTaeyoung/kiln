//! 연결 설정 저장: 평문 비밀번호는 체크했을 때만 파일에 쓴다. 기록은 연결별로 저장된다.

use kiln_db::{ConnConfig, DbManager, Driver, HistoryEntry};

#[test]
fn password_is_written_to_file_only_when_opted_in_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("db_connections.json");
    let m = DbManager::with_store(Some(path.clone()), false);
    let a = m.add(
        ConnConfig {
            name: "secret".into(),
            driver: Driver::Postgres,
            user: "u".into(),
            ..Default::default()
        },
        Some("hunter2".into()),
    );
    let b = m.add(
        ConnConfig {
            name: "plain".into(),
            driver: Driver::MySql,
            save_password_in_file: true,
            ..Default::default()
        },
        Some("open-pw".into()),
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("hunter2"), "{text}");
    assert!(text.contains("open-pw"), "{text}");
    assert_eq!(m.password(a).as_deref(), Some("hunter2"));

    m.push_history(
        a,
        HistoryEntry {
            sql: "SELECT 1".into(),
            at: 1,
            elapsed_ms: 2,
            ok: true,
        },
    );
    drop(m);

    let m2 = DbManager::with_store(Some(path.clone()), false);
    let names: Vec<_> = m2.connections().iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, vec!["secret", "plain"]);
    assert_eq!(m2.password(b).as_deref(), Some("open-pw"));
    assert_eq!(
        m2.password(a),
        None,
        "memory-only secret does not survive restart without keychain"
    );
    assert_eq!(m2.history(a)[0].sql, "SELECT 1");

    // 편집: 비밀번호 인자가 None 이면 기존 비밀번호를 유지한다.
    let mut cfg = m2.get(b).unwrap();
    cfg.host = "db2".into();
    m2.update(cfg, None);
    assert_eq!(m2.password(b).as_deref(), Some("open-pw"));
    m2.remove(a);
    assert_eq!(m2.connections().len(), 1);
}
