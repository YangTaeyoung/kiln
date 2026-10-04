//! 에디터 영역에 열리는 DB 탭: 테이블 뷰와 SQL 콘솔.

mod console;
mod table;
mod viewer;

use crate::{ConnId, DbManager, ResultSet};
use egui::Ui;

pub(crate) use console::ConsoleView;
pub use console::ConsoleDocument;
pub(crate) use table::TableView;
pub use table::TableDraft;

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

    /// 이 탭이 쓰는 연결의 드라이버.
    pub fn driver(&self) -> Option<crate::Driver> {
        self.manager.driver(self.conn)
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
                    format!("{name} 콘솔 …")
                } else {
                    format!("{}{}", c.document_name().unwrap_or_else(||format!("{name} 콘솔")), if c.has_draft() { " •" } else { "" })
                }
            }
        }
    }

    /// Closing requires confirmation for pending table edits and SQL drafts.
    pub fn has_unsaved_changes(&self) -> bool {
        match &self.kind {
            Kind::Table(t) => t.pending_changes() > 0,
            Kind::Console(c) => c.has_draft(),
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

    pub fn table_draft(&self) -> Option<TableDraft> {
        match &self.kind { Kind::Table(t) => t.recovery_draft(), _ => None }
    }
    pub fn restore_table_draft(&mut self, draft: &TableDraft) {
        if let Kind::Table(t) = &mut self.kind { t.restore_draft(draft); }
    }
    pub fn console_document(&self)->Option<ConsoleDocument>{match &self.kind{Kind::Console(c)=>Some(c.document()),_=>None}}
    pub fn restore_console_document(&mut self,document:&ConsoleDocument){if let Kind::Console(c)=&mut self.kind{c.restore_document(document);}}
    pub fn console_text(&self) -> Option<&str> {
        match &self.kind { Kind::Console(c) => Some(c.text()), _ => None }
    }
    pub fn request_focus(&mut self) {
        if let Kind::Console(c) = &mut self.kind { c.request_focus(); }
    }
    pub fn ui(&mut self, ui: &mut Ui) {
        self.manager.set_ctx(ui.ctx());
        match &mut self.kind {
            Kind::Table(t) => t.ui(ui, &self.manager),
            Kind::Console(c) => c.ui(ui, &self.manager),
        }
    }
}


#[cfg(test)]
mod close_guard_tests {
    use super::*;

    #[test]
    fn console_draft_requires_discard_even_without_pending_table_edits() {
        let mut tab = DbTab::console(DbManager::in_memory(), ConnId(1));
        assert!(!tab.has_unsaved_changes());
        tab.set_console_text("select * from important_work;");
        assert!(tab.has_unsaved_changes());
        assert_eq!(tab.pending_changes(), 0);
        assert!(tab.title().ends_with(" •"));
        tab.set_console_text("  \n");
        assert!(!tab.has_unsaved_changes());
        assert!(!tab.title().ends_with(" •"));
    }
}
