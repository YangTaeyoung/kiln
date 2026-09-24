//! 에디터 영역에 열리는 DB 탭: 테이블 뷰와 SQL 콘솔.

mod console;
mod table;
mod viewer;

use crate::{ConnId, DbManager, ResultSet};
use egui::Ui;

pub(crate) use console::ConsoleView;
pub(crate) use table::TableView;

enum Kind {
    Table(Box<TableView>),
    Console(Box<ConsoleView>),
}

/// 테이블 데이터 탭 또는 콘솔 탭.
pub struct DbTab {
    manager: DbManager,
    conn: ConnId,
    kind: Kind,
}

impl DbTab {
    /// 테이블 데이터 탭.
    pub fn table(manager: DbManager, conn: ConnId, schema: Option<String>, table: String) -> DbTab {
        let view = TableView::new(&manager, conn, schema, table);
        DbTab {
            manager,
            conn,
            kind: Kind::Table(Box::new(view)),
        }
    }

    /// SQL 콘솔 탭.
    pub fn console(manager: DbManager, conn: ConnId) -> DbTab {
        let view = ConsoleView::new(&manager, conn);
        DbTab {
            manager,
            conn,
            kind: Kind::Console(Box::new(view)),
        }
    }

    /// 이미 받은 결과 집합을 보여주는 콘솔 탭(렌더링 측정·미리보기용).
    #[doc(hidden)]
    pub fn console_with_result(
        manager: DbManager,
        conn: ConnId,
        sql: &str,
        rs: ResultSet,
    ) -> DbTab {
        let mut view = ConsoleView::new(&manager, conn);
        view.set_text(sql);
        view.push_result(sql, rs);
        DbTab {
            manager,
            conn,
            kind: Kind::Console(Box::new(view)),
        }
    }

    pub fn conn(&self) -> ConnId {
        self.conn
    }

    /// 탭 제목.
    pub fn title(&self) -> String {
        let name = self
            .manager
            .get(self.conn)
            .map(|c| c.display_name())
            .unwrap_or_else(|| "db".into());
        match &self.kind {
            Kind::Table(t) => {
                let dirty = if t.pending_changes() > 0 { " •" } else { "" };
                format!("{}{dirty}", t.table_name())
            }
            Kind::Console(c) => {
                if c.is_running() {
                    format!("{name} console …")
                } else {
                    format!("{name} console")
                }
            }
        }
    }

    /// 제출하지 않은 변경 수(테이블 탭만).
    pub fn pending_changes(&self) -> usize {
        match &self.kind {
            Kind::Table(t) => t.pending_changes(),
            Kind::Console(_) => 0,
        }
    }

    /// 콘솔 편집기 텍스트를 바꾼다(콘솔 탭만).
    pub fn set_console_text(&mut self, sql: &str) {
        if let Kind::Console(c) = &mut self.kind {
            c.set_text(sql);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.manager.set_ctx(ui.ctx());
        match &mut self.kind {
            Kind::Table(t) => t.ui(ui, &self.manager),
            Kind::Console(c) => c.ui(ui, &self.manager),
        }
    }
}
