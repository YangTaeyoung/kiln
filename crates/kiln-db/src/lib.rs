//! Kiln 데이터베이스 클라이언트: 연결 관리, 탐색기 패널, 테이블/콘솔 탭.

mod config;
mod driver;
pub mod edit;
pub mod export;
mod manager;
pub mod logo;
pub mod meta;
mod panel;
pub mod sql;
mod tab;
mod ui;
pub mod value;

pub use config::{ConnConfig, ConnId, Driver, SslMode};
pub use driver::{ChangeError, ChangeStmt, ColumnInfo, DbError, DbResult, ResultSet, StmtOutcome};
pub use edit::{ChangeSet, RowInsert, RowUpdate, TableRef};
pub use manager::{ConnStatus, ConsoleSession, DbManager, HistoryEntry, Job};
pub use meta::{ColumnDef, ForeignKeyInfo, IndexInfo, TableDetails, TableInfo, TableKind};
pub use panel::{DbEvent, DbPanel};
pub use tab::DbTab;
pub use value::{TypeClass, Value};
