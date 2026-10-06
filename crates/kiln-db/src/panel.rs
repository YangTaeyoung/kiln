//! 데이터베이스 탐색기 패널: 연결 목록, 스키마 트리, 연결 편집 대화상자.

use crate::edit::TableRef;
use crate::schema::{SchemaAction, ColumnSpec, IndexSpec};
use crate::TableSection;
use crate::manager::Job;
use crate::meta::{TableDetails, TableInfo, TableKind};
use crate::ui::{self, Glyph, TypedConfirm, chevron, dim, faint, icon_button, status_color, tree_row};
use crate::{ConnConfig, ConnId, ConnStatus, DbManager, DbResult, Driver, SslMode};
use egui::{Align2, Color32, RichText, Sense, Ui, pos2, vec2};
use kiln_common::icons::Icon;
use kiln_common::widgets::{self, ButtonKind};
use kiln_common::{Theme, fonts};
use std::collections::HashMap;
use std::time::Instant;

/// 패널이 앱에 요청하는 동작.
#[derive(Clone, Debug, PartialEq)]
pub enum DbEvent {
    OpenTable {
        conn: ConnId,
        schema: Option<String>,
        table: String,
    },
    OpenConsole { conn: ConnId },
    OpenTableSection { conn: ConnId, schema: Option<String>, table: String, section: TableSection },
    SchemaAction { conn: ConnId, schema: Option<String>, table: String, action: SchemaAction },
}

enum Load<T> {
    Idle,
    Loading(Job<DbResult<T>>),
    Ready(T),
    Failed(String),
}

impl<T> Load<T> {
    fn poll(&mut self) {
        if let Load::Loading(job) = self {
            if let Some(r) = job.poll() {
                *self = match r {
                    Ok(v) => Load::Ready(v),
                    Err(e) => Load::Failed(e.to_string()),
                };
            } else if !job.is_running() {
                *self = Load::Failed(kiln_common::i18n::tr("취소됨").into());
            }
        }
    }

    fn is_loading(&self) -> bool {
        matches!(self, Load::Loading(_))
    }
}

struct TableNode {
    info: TableInfo,
    open: bool,
    details: Load<TableDetails>,
    indexes_open: bool,
    fks_open: bool,
}

struct SchemaNode {
    name: String,
    open: bool,
    tables_open: bool,
    views_open: bool,
    tables: Load<Vec<TableNode>>,
}

#[derive(Default)]
struct ConnNode {
    open: bool,
    schemas: Option<Load<Vec<SchemaNode>>>,
    epoch: Option<u64>,
    schema_expansion: HashMap<String, (bool, bool, bool)>,
    table_expansion: HashMap<(String, String), (bool, bool, bool)>,
}

impl ConnNode {
    fn remember_expansion(&mut self) {
        if let Some(Load::Ready(schemas)) = &self.schemas {
            for schema in schemas {
                self.schema_expansion.insert(schema.name.clone(), (schema.open, schema.tables_open, schema.views_open));
                if let Load::Ready(tables) = &schema.tables {
                    for table in tables {
                        self.table_expansion.insert((schema.name.clone(), table.info.name.clone()), (table.open, table.indexes_open, table.fks_open));
                    }
                }
            }
        }
    }
    fn cancel_loads(&mut self) {
        match &mut self.schemas {
            Some(Load::Loading(job)) => job.abort(),
            Some(Load::Ready(schemas)) => for schema in schemas {
                match &mut schema.tables {
                    Load::Loading(job) => job.abort(),
                    Load::Ready(tables) => for table in tables {
                        if let Load::Loading(job) = &mut table.details { job.abort(); }
                    },
                    _ => {}
                }
            },
            _ => {}
        }
    }
    fn reload(&mut self, manager: &DbManager, id: ConnId) {
        self.remember_expansion();
        self.cancel_loads();
        self.schemas = if self.schemas.is_some() && manager.status(id) == ConnStatus::Connected { Some(load_schemas(manager, id)) } else { None };
        self.epoch = Some(manager.connection_epoch(id));
    }
    fn restore_expansion(&mut self) {
        if let Some(Load::Ready(schemas)) = &mut self.schemas {
            for schema in schemas {
                if let Some((open, tables, views)) = self.schema_expansion.remove(&schema.name) {
                    schema.open = open; schema.tables_open = tables; schema.views_open = views;
                }
                if let Load::Ready(tables) = &mut schema.tables {
                    for table in tables {
                        if let Some((open, indexes, fks)) = self.table_expansion.remove(&(schema.name.clone(), table.info.name.clone())) {
                            table.open = open; table.indexes_open = indexes; table.fks_open = fks;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NodeKey {
    Conn(ConnId),
    Schema(ConnId, String),
    Table(ConnId, String, String),
}

/// 연결 추가/편집 대화상자 상태.
struct ConnDialog {
    cfg: ConnConfig,
    password: String,
    password_touched: bool,
    is_new: bool,
    url: String,
    url_error: Option<String>,
    test: Option<Job<DbResult<String>>>,
    test_result: Option<Result<String, String>>,
    tested_settings: Option<(ConnConfig,String)>,
    test_stale: bool,
}

enum PendingAction {
    CopyDdl(Job<DbResult<String>>),
    Dangerous {
        job: Job<DbResult<()>>,
        conn: ConnId,
        schema: String,
    },
}

enum ConfirmKind {
    Truncate(ConnId, TableRef),
    Drop(ConnId, TableRef, TableKind),
    DeleteConn(ConnId),
}

/// 탐색기 패널.
pub struct DbPanel {
    manager: DbManager,
    filter: String,
    nodes: HashMap<ConnId, ConnNode>,
    selected: Option<NodeKey>,
    dialog: Option<ConnDialog>,
    confirm: Option<(TypedConfirm, ConfirmKind)>,
    actions: Vec<PendingAction>,
    toast: Option<(String, bool, Instant)>,
    events: Vec<DbEvent>,
}

const ROW_H: f32 = 26.0;
const INDENT: f32 = 14.0;

const COLOR_PRESETS: [[u8; 3]; 6] = [
    [0x6c, 0x9e, 0xff],
    [0x7e, 0xc6, 0x7a],
    [0xe5, 0xc0, 0x7b],
    [0xe0, 0x96, 0x5c],
    [0xf0, 0x6c, 0x75],
    [0xc6, 0x78, 0xdd],
];

impl DbPanel {
    pub fn new(manager: DbManager) -> DbPanel {
        DbPanel {
            manager,
            filter: String::new(),
            nodes: HashMap::new(),
            selected: None,
            dialog: None,
            confirm: None,
            actions: Vec::new(),
            toast: None,
            events: Vec::new(),
        }
    }

    pub fn manager(&self) -> &DbManager {
        &self.manager
    }

    /// 새 연결 대화상자를 연다.
    pub fn open_new_connection_dialog(&mut self) {
        self.dialog = Some(ConnDialog::new(ConnConfig::default(), String::new(), true));
    }

    /// 패널을 그리고 발생한 이벤트를 돌려준다.
    pub fn ui(&mut self, ui: &mut Ui) -> Vec<DbEvent> {
        self.manager.set_ctx(ui.ctx());
        let theme = Theme::current();
        self.poll_jobs(ui.ctx());

        egui::Frame::new()
            .fill(theme.bg_panel)
            .inner_margin(egui::Margin::symmetric(8, 8))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.header(ui);
                ui.add_space(6.0);
                self.filter_box(ui);
                ui.add_space(8.0);
                if let Some(w) = self.manager.keychain_warning() {
                    ui::banner(ui, &w, true);
                    ui.add_space(6.0);
                }
                egui::ScrollArea::vertical()
                    .id_salt("db-tree")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        self.tree(ui);
                    });
            });

        self.dialog_ui(ui.ctx());
        self.confirm_ui(ui.ctx());
        self.toast_ui(ui);
        if self
            .nodes
            .values()
            .any(|n| n.schemas.as_ref().is_some_and(|s| s.is_loading()))
            || !self.actions.is_empty()
        {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(100));
        }
        std::mem::take(&mut self.events)
    }

    fn header(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        ui.horizontal(|ui| {
            ui.set_min_height(26.0);
            ui.add_space(4.0);
            ui.label(RichText::new(kiln_common::i18n::tr("데이터베이스")).font(fonts::semibold(13.0)).color(theme.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let add = icon_button(ui, Icon::Plus, kiln_common::i18n::tr("새 연결"));
                egui::Popup::menu(&add).gap(4.0).show(|ui| {
                    ui.set_min_width(190.0);
                    for d in Driver::ALL {
                        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width().max(190.0), 32.0), Sense::click());
                        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, kiln_common::trf!("{} 연결", d.label())));
                        if resp.hovered() {
                            ui.painter().rect_filled(r, 6.0, theme.bg_hover);
                        }
                        crate::logo::paint(ui, egui::Rect::from_center_size(pos2(r.left() + 16.0, r.center().y), vec2(20.0, 20.0)), d);
                        ui.painter().text(pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, d.label(), fonts::medium(13.0), theme.text);
                        if resp.clicked() {
                            self.start_new_connection(d);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button(kiln_common::i18n::tr("URL에서 가져오기…")).clicked() {
                        let mut d = ConnDialog::new(ConnConfig::default(), String::new(), true);
                        d.url = "postgres://user:password@localhost:5432/db".into();
                        self.dialog = Some(d);
                        ui.close();
                    }
                });
                if icon_button(ui, Icon::Refresh, kiln_common::i18n::tr("모두 새로 고침")).clicked() {
                    for (id, n) in self.nodes.iter_mut() {
                        if n.schemas.is_some() {
                            let m = self.manager.clone();
                            let id = *id;
                            n.schemas = Some(load_schemas(&m, id));
                        }
                    }
                }
                if icon_button(ui, Icon::Terminal, kiln_common::i18n::tr("선택한 연결에 새 콘솔 열기")).clicked()
                    && let Some(id) = self.selected_conn()
                {
                    self.events.push(DbEvent::OpenConsole { conn: id });
                }
            });
        });
    }

    fn filter_box(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let id = egui::Id::new("db-filter");
        let focused = ui.memory(|m| m.has_focus(id));
        widgets::input_frame(focused, false)
            .inner_margin(egui::Margin { left: 8, right: 4, top: 2, bottom: 2 })
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui::glyph_label(ui, Icon::Search, theme.text_faint, 14.0);
                    let clear_w = if self.filter.is_empty() { 0.0 } else { 26.0 };
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .id(id)
                            .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(0, 4)))
                            .hint_text(RichText::new(kiln_common::i18n::tr("테이블 필터")).color(theme.text_faint))
                            .desired_width(ui.available_width() - clear_w),
                    );
                    if !self.filter.is_empty() && icon_button(ui, Icon::Close, kiln_common::i18n::tr("지우기")).clicked() {
                        self.filter.clear();
                    }
                });
            });
    }

    fn selected_conn(&self) -> Option<ConnId> {
        match &self.selected {
            Some(NodeKey::Conn(id))
            | Some(NodeKey::Schema(id, _))
            | Some(NodeKey::Table(id, _, _)) => Some(*id),
            None => self.manager.connections().first().map(|c| c.id),
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        for (id, n) in &mut self.nodes {
            let epoch = self.manager.connection_epoch(*id);
            if n.epoch.is_some_and(|previous| previous != epoch) { n.reload(&self.manager, *id); }
            n.epoch = Some(epoch);
            if let Some(s) = &mut n.schemas {
                s.poll();
                if let Load::Ready(schemas) = s {
                    for sc in schemas.iter_mut() {
                        sc.tables.poll();
                        if let Load::Ready(ts) = &mut sc.tables {
                            for t in ts.iter_mut() {
                                t.details.poll();
                            }
                        }
                    }
                }
            }
        }
        for n in self.nodes.values_mut() { n.restore_expansion(); }
        let mut done = Vec::new();
        let mut reload: Vec<(ConnId, String)> = Vec::new();
        for (i, a) in self.actions.iter_mut().enumerate() {
            match a {
                PendingAction::CopyDdl(job) => {
                    if let Some(r) = job.poll() {
                        match r {
                            Ok(ddl) => {
                                ctx.copy_text(ddl);
                                self.toast =
                                    Some((kiln_common::i18n::tr("DDL을 클립보드에 복사했습니다").into(), false, Instant::now()));
                            }
                            Err(e) => self.toast = Some((e.to_string(), true, Instant::now())),
                        }
                        done.push(i);
                    }
                }
                PendingAction::Dangerous { job, conn, schema } => {
                    if let Some(r) = job.poll() {
                        match r {
                            Ok(()) => {
                                self.toast = Some((kiln_common::i18n::tr("완료").into(), false, Instant::now()));
                                reload.push((*conn, schema.clone()));
                            }
                            Err(e) => self.toast = Some((e.to_string(), true, Instant::now())),
                        }
                        done.push(i);
                    }
                }
            }
        }
        for i in done.into_iter().rev() {
            self.actions.remove(i);
        }
        let m = self.manager.clone();
        for (c, s) in reload {
            if let Some(sc) = self.schema_node_mut(c, &s) {
                sc.tables = load_tables(&m, c, &s);
            }
        }
    }

    fn schema_node_mut(&mut self, conn: ConnId, schema: &str) -> Option<&mut SchemaNode> {
        match self.nodes.get_mut(&conn)?.schemas.as_mut()? {
            Load::Ready(v) => v.iter_mut().find(|s| s.name == schema),
            _ => None,
        }
    }

    fn start_new_connection(&mut self, d: Driver) {
        let cfg = ConnConfig {
            driver: d,
            port: d.default_port(),
            host: if d == Driver::Sqlite { String::new() } else { "localhost".into() },
            ..ConnConfig::default()
        };
        self.dialog = Some(ConnDialog::new(cfg, String::new(), true));
    }

    fn tree(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let conns = self.manager.connections();
        if conns.is_empty() {
            ui.add_space(28.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(kiln_common::i18n::tr("데이터베이스에 연결하세요")).font(fonts::semibold(15.0)).color(theme.text));
                ui.add_space(4.0);
                ui.label(RichText::new(kiln_common::i18n::tr("연결할 데이터베이스 종류를 고르세요")).size(12.5).color(theme.text_faint));
            });
            ui.add_space(16.0);
            if let Some(d) = driver_cards(ui, None) {
                self.start_new_connection(d);
            }
            ui.add_space(12.0);
            ui.vertical_centered(|ui| {
                if widgets::button_with(ui, None, kiln_common::i18n::tr("URL에서 가져오기…"), widgets::ButtonKind::Ghost, true).clicked() {
                    let mut d = ConnDialog::new(ConnConfig::default(), String::new(), true);
                    d.url = "postgres://user:password@localhost:5432/db".into();
                    self.dialog = Some(d);
                }
            });
            return;
        }
        let filter = self.filter.trim().to_lowercase();
        for cfg in conns {
            let id = cfg.id;
            let status = self.manager.status(id);
            let node = self.nodes.entry(id).or_default();
            node.epoch.get_or_insert_with(|| self.manager.connection_epoch(id));
            if node.open && node.schemas.is_none() && status == ConnStatus::Connected {
                node.schemas = Some(load_schemas(&self.manager, id));
            }
            let sel = self.selected == Some(NodeKey::Conn(id));
            let (rect, resp) = tree_row(ui, ROW_H + 4.0, sel, &cfg.display_name());
            let x0 = rect.min.x + 4.0;
            let cy = rect.center().y;
            chevron(ui, pos2(x0 + 5.0, cy), node.open, true);
            let tint = cfg.color.map(|c| Color32::from_rgb(c[0], c[1], c[2]));
            let ir = egui::Rect::from_center_size(pos2(x0 + 22.0, cy), vec2(20.0, 20.0));
            crate::logo::paint(ui, ir, cfg.driver);
            if let Some(c) = tint {
                // 사용자가 고른 연결 색은 행 왼쪽 막대로 표시한다.
                ui.painter().rect_filled(egui::Rect::from_min_size(pos2(rect.min.x, rect.min.y + 6.0), vec2(3.0, rect.height() - 12.0)), 2.0, c);
            }
            let dot = ir.right_bottom() - vec2(1.0, 1.0);
            ui.painter().circle_filled(dot, 4.0, theme.bg_panel);
            ui.painter().circle_filled(dot, 2.8, status_color(&status));
            let name_g = ui.painter().text(
                pos2(x0 + 38.0, cy),
                Align2::LEFT_CENTER,
                cfg.display_name(),
                fonts::medium(13.0),
                theme.text,
            );
            ui.painter().text(
                pos2(name_g.max.x + 7.0, cy + 0.5),
                Align2::LEFT_CENTER,
                cfg.driver.label(),
                fonts::regular(11.5),
                theme.text_faint,
            );
            if matches!(node.schemas, Some(Load::Loading(_))) || status == ConnStatus::Connecting {
                ui.put(
                    egui::Rect::from_center_size(pos2(rect.max.x - 14.0, cy), vec2(12.0, 12.0)),
                    egui::Spinner::new().size(11.0).color(theme.text_dim),
                );
            }
            let hover = match &status {
                ConnStatus::Failed(e) => format!("{}\n\n{e}", cfg.summary()),
                _ => cfg.summary(),
            };
            let resp = resp.on_hover_text(hover);
            if resp.clicked() {
                self.selected = Some(NodeKey::Conn(id));
                node.open = !node.open;
                if node.open && node.schemas.is_none() {
                    node.schemas = Some(load_schemas(&self.manager, id));
                }
            }
            if ui::double_clicked(ui, &resp) {
                self.events.push(DbEvent::OpenConsole { conn: id });
            }
            resp.context_menu(|ui| {
                ui.set_min_width(180.0);
                if status == ConnStatus::Connected {
                    if ui.button(kiln_common::i18n::tr("연결 끊기")).clicked() {
                        self.manager.disconnect(id);
                        node.schemas = None;
                        node.open = false;
                        ui.close();
                    }
                } else if ui.button(kiln_common::i18n::tr("연결")).clicked() {
                    node.open = true;
                    node.schemas = Some(load_schemas(&self.manager, id));
                    ui.close();
                }
                if ui.button(kiln_common::i18n::tr("새 콘솔")).clicked() {
                    self.events.push(DbEvent::OpenConsole { conn: id });
                    ui.close();
                }
                if ui.button(kiln_common::i18n::tr("새로 고침")).clicked() {
                    node.schemas = Some(load_schemas(&self.manager, id));
                    node.open = true;
                    ui.close();
                }
                ui.separator();
                if ui.button(kiln_common::i18n::tr("편집…")).clicked() {
                    let pw = self.manager.password(id).unwrap_or_default();
                    self.dialog = Some(ConnDialog::new(cfg.clone(), pw, false));
                    ui.close();
                }
                if ui.button(kiln_common::i18n::tr("이름 복사")).clicked() {
                    ui.ctx().copy_text(cfg.display_name());
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(RichText::new(kiln_common::i18n::tr("연결 삭제…")).color(theme.red))
                    .clicked()
                {
                    self.confirm = Some((
                        TypedConfirm {
                            title: kiln_common::i18n::tr("연결 삭제").into(),
                            message: kiln_common::trf!(
                                "\"{}\" 연결 설정과 저장된 비밀번호를 삭제할까요? 데이터베이스의 데이터는 유지됩니다.",
                                cfg.display_name()
                            ),
                            expected: cfg.display_name(),
                            input: String::new(),
                            action_label: kiln_common::i18n::tr("연결 삭제").into(),
                        },
                        ConfirmKind::DeleteConn(id),
                    ));
                    ui.close();
                }
            });
            if !node.open && filter.is_empty() {
                continue;
            }
            if !node.open && !filter.is_empty() && node.schemas.is_none() {
                continue;
            }
            let Some(schemas) = &mut node.schemas else {
                continue;
            };
            match schemas {
                Load::Idle => {}
                Load::Loading(_) => {
                    info_row(ui, 1, kiln_common::i18n::tr("연결 중…"), theme.text_faint);
                }
                Load::Failed(e) => {
                    let e = e.clone();
                    info_row(ui, 1, &e, theme.red);
                }
                Load::Ready(list) => {
                    let single = list.len() == 1;
                    for sc in list.iter_mut() {
                        schema_ui(
                            ui,
                            &self.manager,
                            id,
                            cfg.driver,
                            sc,
                            single,
                            &filter,
                            &mut self.selected,
                            &mut self.events,
                            &mut self.actions,
                            &mut self.confirm,
                        );
                    }
                }
            }
        }
    }

    fn dialog_ui(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.dialog else {
            return;
        };
        if let Some(job) = &mut d.test
            && let Some(r) = job.poll()
        {
            d.test_result = Some(r.map_err(|e| e.to_string()));
            d.test = None;
        }
        let theme = Theme::current();
        let mut close = false;
        let mut save = false;
        let fid = |k: &str| egui::Id::new(("db-conn-field", k));
        let resp = egui::Modal::new(egui::Id::new("db-conn-dialog")).frame(ui::modal_frame()).show(ctx, |ui| {
            let width=460.0f32.min(ctx.content_rect().width()-64.0).max(260.0);
            ui.set_width(width);
            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
            ui.spacing_mut().scroll.floating=false;
            ui.spacing_mut().scroll.dormant_handle_opacity=0.65;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), egui::Sense::hover());
                crate::logo::paint(ui, r, d.cfg.driver);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(
                        RichText::new(if d.is_new { kiln_common::i18n::tr("새 연결") } else { kiln_common::i18n::tr("연결 편집") })
                            .font(fonts::semibold(15.0))
                            .color(theme.text),
                    );
                    ui.label(faint(d.cfg.driver.label()));
                });
            });
            ui.add_space(4.0);
            let status_budget=if d.test.is_some() || d.test_result.is_some() || d.test_stale {210.0}else{160.0};
            egui::ScrollArea::vertical().id_salt("db-connection-fields").max_height((ctx.content_rect().height()-status_budget).max(60.0)).auto_shrink([false,true]).show(ui,|ui| {
            // URL 가져오기.
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(72.0, 28.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(72.0);
                        ui.label(dim("URL"))
                    },
                );
                let r = ui::text_field(
                    ui,
                    egui::TextEdit::singleline(&mut d.url)
                        .hint_text(RichText::new("postgres://user:pass@host:5432/db").color(theme.text_faint)),
                    fid("url"),
                    (width-170.0).max(100.0),
                );
                if ui::secondary_button(ui, None, kiln_common::i18n::tr("가져오기"), true).clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    match ConnConfig::from_url(&d.url) {
                        Ok((mut cfg, pw)) => {
                            cfg.id = d.cfg.id;
                            cfg.color = d.cfg.color;
                            d.cfg = cfg;
                            if let Some(p) = pw {
                                d.password = p;
                                d.password_touched = true;
                            }
                            d.url_error = None;
                        }
                        Err(e) => d.url_error = Some(e),
                    }
                }
            });
            if let Some(e) = &d.url_error {
                ui.horizontal(|ui| {
                    ui.add_space(80.0);
                    ui.label(RichText::new(e).color(theme.red).size(11.5));
                });
            }
            let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 0.0, theme.border);
            let full = (ui.available_width()-88.0).max(150.0);
            egui::Grid::new("db-conn-grid")
                .num_columns(2)
                .min_col_width(72.0)
                .spacing(vec2(8.0, 8.0))
                .show(ui, |ui| {
                    let label = |ui: &mut Ui, s: &str| {
                        ui.allocate_ui_with_layout(
                            vec2(72.0, 28.0),
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| ui.label(dim(s)),
                        );
                    };
                    label(ui, kiln_common::i18n::tr("연결 이름"));
                    let hint = d.cfg.display_name();
                    ui::text_field(
                        ui,
                        egui::TextEdit::singleline(&mut d.cfg.name).hint_text(RichText::new(hint).color(theme.text_faint)),
                        fid("name"),
                        full,
                    );
                    ui.end_row();
                    label(ui, kiln_common::i18n::tr("DB 종류"));
                    if let Some(drv) = driver_chips(ui, d.cfg.driver, full) {
                        if d.cfg.port == d.cfg.driver.default_port() {
                            d.cfg.port = drv.default_port();
                        }
                        d.cfg.driver = drv;
                    }
                    ui.end_row();
                    if d.cfg.driver == Driver::Sqlite {
                        label(ui, kiln_common::i18n::tr("파일"));
                        ui.horizontal(|ui| {
                            ui::text_field(ui, egui::TextEdit::singleline(&mut d.cfg.file), fid("file"), full - 96.0);
                            if ui::secondary_button(ui, None, kiln_common::i18n::tr("찾아보기…"), true).clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .add_filter("SQLite", &["db", "sqlite", "sqlite3", "db3"])
                                    .add_filter(kiln_common::i18n::tr("모든 파일"), &["*"])
                                    .pick_file()
                            {
                                d.cfg.file = p.to_string_lossy().into_owned();
                            }
                        });
                        ui.end_row();
                    } else {
                        label(ui, kiln_common::i18n::tr("호스트"));
                        ui.horizontal(|ui| {
                            ui::text_field(ui, egui::TextEdit::singleline(&mut d.cfg.host), fid("host"), full - 124.0);
                            ui.label(dim(kiln_common::i18n::tr("포트")));
                            ui.add_sized(vec2(78.0, 28.0), egui::DragValue::new(&mut d.cfg.port).range(1..=65535));
                        });
                        ui.end_row();
                        label(ui, kiln_common::i18n::tr("사용자"));
                        ui::text_field(ui, egui::TextEdit::singleline(&mut d.cfg.user), fid("user"), full);
                        ui.end_row();
                        label(ui, kiln_common::i18n::tr("비밀번호"));
                        let r = ui::text_field(
                            ui,
                            egui::TextEdit::singleline(&mut d.password).password(true),
                            fid("password"),
                            full,
                        );
                        if r.changed() {
                            d.password_touched = true;
                        }
                        ui.end_row();
                        label(ui, kiln_common::i18n::tr("데이터베이스"));
                        ui::text_field(ui, egui::TextEdit::singleline(&mut d.cfg.database), fid("database"), full);
                        ui.end_row();
                        label(ui, kiln_common::i18n::tr("SSL 모드"));
                        egui::ComboBox::from_id_salt("db-ssl")
                            .selected_text(d.cfg.ssl_mode.label())
                            .width(160.0)
                            .show_ui(ui, |ui| {
                                for m in SslMode::ALL {
                                    ui.selectable_value(&mut d.cfg.ssl_mode, m, m.label());
                                }
                            });
                        ui.end_row();
                    }
                    label(ui, kiln_common::i18n::tr("타임아웃"));
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            vec2(78.0, 28.0),
                            egui::DragValue::new(&mut d.cfg.connect_timeout_secs)
                                .range(1..=120)
                                .suffix(" s"),
                        );
                    });
                    ui.end_row();
                    label(ui, kiln_common::i18n::tr("색상"));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        let (r, resp) = ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::click());
                        let none_sel = d.cfg.color.is_none();
                        ui.painter().circle_stroke(r.center(), 7.0, egui::Stroke::new(1.2, theme.text_faint));
                        ui.painter().line_segment(
                            [r.center() + vec2(-4.5, 4.5), r.center() + vec2(4.5, -4.5)],
                            egui::Stroke::new(1.2, theme.text_faint),
                        );
                        if none_sel {
                            ui.painter().circle_stroke(r.center(), 10.0, egui::Stroke::new(1.5, theme.accent));
                        }
                        if resp.on_hover_text(kiln_common::i18n::tr("없음")).clicked() {
                            d.cfg.color = None;
                        }
                        for c in COLOR_PRESETS {
                            let col = Color32::from_rgb(c[0], c[1], c[2]);
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(22.0, 22.0), egui::Sense::click());
                            ui.painter().circle_filled(r.center(), 7.5, col);
                            if d.cfg.color == Some(c) {
                                ui.painter().circle_stroke(
                                    r.center(),
                                    10.0,
                                    egui::Stroke::new(1.5, theme.accent),
                                );
                            }
                            if resp.clicked() {
                                d.cfg.color = Some(c);
                            }
                        }
                    });
                    ui.end_row();
                });
            if d.cfg.driver != Driver::Sqlite {
                ui.horizontal(|ui| {
                    ui.add_space(80.0);
                    widgets::toggle(ui, &mut d.cfg.save_password_in_file);
                    ui.label(RichText::new(kiln_common::i18n::tr("설정 파일에 비밀번호 저장 (평문)")).size(12.5).color(theme.text));
                });
                if !d.cfg.save_password_in_file {
                    ui.horizontal(|ui| {
                        ui.add_space(80.0);
                        ui.label(faint(kiln_common::i18n::tr("비밀번호는 OS 키체인에 저장됩니다.")));
                    });
                }
            }
            });
            d.invalidate_changed_test();
            egui::ScrollArea::vertical().id_salt("db-connection-result").max_height(48.0).show(ui,|ui| {
            if d.test_stale {ui.label(dim(kiln_common::i18n::tr("설정이 변경되었습니다. 다시 테스트하세요.")));}
            match (&d.test, &d.test_result) {
                (Some(_), _) => {
                    ui.horizontal(|ui| {
                        ui::spinner(ui);
                        ui.label(dim(kiln_common::i18n::tr("연결 테스트 중…")));
                    });
                }
                (None, Some(Ok(v))) => {
                    ui.horizontal(|ui| {
                        ui::glyph_label(ui, Icon::Check, theme.green, 14.0);
                        ui.label(RichText::new(kiln_common::trf!("연결 테스트 성공 · {}", first_line(v, 90))).color(theme.green).size(12.5));
                    });
                }
                (None, Some(Err(e))) => {
                    ui::banner(ui, e, true);
                }
                _ => {}
            }
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui::secondary_button(ui, Some(Icon::Plug), kiln_common::i18n::tr("연결 테스트"), d.test.is_none()).clicked() {
                    let m = self.manager.clone();
                    let cfg = d.cfg.clone();
                    let pw = (!d.password.is_empty()).then(|| d.password.clone());
                    d.test_result = None;
                    d.test_stale = false;
                    d.tested_settings = Some((d.cfg.clone(),d.password.clone()));
                    d.test = Some(
                        self.manager
                            .spawn(async move { m.test_connection(cfg, pw).await }),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::button(ui, kiln_common::i18n::tr("저장"), ButtonKind::Primary).clicked() {
                        save = true;
                    }
                    if widgets::button(ui, kiln_common::i18n::tr("취소"), ButtonKind::Ghost).clicked() {
                        close = true;
                    }
                });
            });
        });
        if resp.should_close() {
            close = true;
        }
        if save {
            let d = self.dialog.take().expect("dialog");
            let pw = if d.password_touched || d.is_new {
                Some(d.password.clone())
            } else {
                None
            };
            if d.is_new {
                let id = self.manager.add(d.cfg, pw);
                self.selected = Some(NodeKey::Conn(id));
            } else {
                let id = d.cfg.id;
                self.manager.update(d.cfg, pw);
                self.nodes.remove(&id);
            }
        } else if close {
            self.dialog = None;
        }
    }

    fn confirm_ui(&mut self, ctx: &egui::Context) {
        let Some((c, kind)) = &mut self.confirm else {
            return;
        };
        let Some(ok) = c.show(ctx, egui::Id::new("db-confirm")) else {
            return;
        };
        if ok {
            let m = self.manager.clone();
            match kind {
                ConfirmKind::Truncate(id, t) => {
                    let (id, t) = (*id, t.clone());
                    let schema = t.schema.clone().unwrap_or_default();
                    let job = self
                        .manager
                        .spawn(async move { m.truncate_table(id, &t).await });
                    self.actions.push(PendingAction::Dangerous {
                        job,
                        conn: id,
                        schema,
                    });
                }
                ConfirmKind::Drop(id, t, k) => {
                    let (id, t, k) = (*id, t.clone(), *k);
                    let schema = t.schema.clone().unwrap_or_default();
                    let job = self
                        .manager
                        .spawn(async move { m.drop_table(id, &t, k).await });
                    self.actions.push(PendingAction::Dangerous {
                        job,
                        conn: id,
                        schema,
                    });
                }
                ConfirmKind::DeleteConn(id) => {
                    self.manager.remove(*id);
                    self.nodes.remove(id);
                }
            }
        }
        self.confirm = None;
    }

    fn toast_ui(&mut self, ui: &mut Ui) {
        let Some((msg, err, at)) = &self.toast else {
            return;
        };
        if at.elapsed().as_secs_f32() > 4.0 {
            self.toast = None;
            return;
        }
        let theme = Theme::current();
        let rect = ui.max_rect();
        egui::Area::new(egui::Id::new("db-panel-toast"))
            .fixed_pos(pos2(rect.min.x + 10.0, rect.max.y - 48.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme.bg_elevated)
                    .stroke(egui::Stroke::new(1.0, theme.border_strong))
                    .corner_radius(10.0)
                    .shadow(theme.shadow())
                    .inner_margin(egui::Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(rect.width() - 30.0);
                        ui.horizontal(|ui| {
                            let (icon, c) = if *err { (Icon::Warning, theme.red) } else { (Icon::Check, theme.green) };
                            ui::glyph_label(ui, icon, c, 14.0);
                            ui.add(egui::Label::new(RichText::new(msg.as_str()).color(theme.text).size(12.5)).wrap());
                        });
                    });
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

impl ConnDialog {
    fn invalidate_changed_test(&mut self) {
        if self.tested_settings.as_ref().is_some_and(|(cfg,password)|cfg!=&self.cfg || password!=&self.password) {
            self.test=None; self.test_result=None; self.tested_settings=None; self.test_stale=true;
        }
    }

    fn new(cfg: ConnConfig, password: String, is_new: bool) -> ConnDialog {
        ConnDialog {
            cfg,
            password,
            password_touched: false,
            is_new,
            url: String::new(),
            url_error: None,
            test: None,
            test_result: None,
            tested_settings: None,
            test_stale: false,
        }
    }
}

fn first_line(s: &str, max: usize) -> String {
    crate::value::one_line(s.lines().next().unwrap_or(""), max)
}

fn load_schemas(m: &DbManager, id: ConnId) -> Load<Vec<SchemaNode>> {
    let m2 = m.clone();
    Load::Loading(m.spawn(async move {
        let names = m2.list_schemas(id).await?;
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(i, name)| SchemaNode {
                open: i == 0,
                tables_open: true,
                views_open: false,
                tables: Load::Idle,
                name,
            })
            .collect())
    }))
}

fn load_tables(m: &DbManager, id: ConnId, schema: &str) -> Load<Vec<TableNode>> {
    let m2 = m.clone();
    let schema = schema.to_string();
    Load::Loading(m.spawn(async move {
        Ok(m2
            .list_tables(id, &schema)
            .await?
            .into_iter()
            .map(|info| TableNode {
                info,
                open: false,
                details: Load::Idle,
                indexes_open: false,
                fks_open: false,
            })
            .collect())
    }))
}

fn load_details(m: &DbManager, id: ConnId, t: TableRef) -> Load<TableDetails> {
    let m2 = m.clone();
    Load::Loading(m.spawn(async move { m2.table_details(id, &t).await }))
}

fn info_row(ui: &mut Ui, depth: usize, text: &str, color: Color32) {
    let (rect, _) = tree_row(ui, ROW_H, false, text);
    let x = rect.min.x + 4.0 + depth as f32 * INDENT + 12.0;
    if color == Theme::current().red {
        ui::paint_glyph(
            ui.painter(),
            egui::Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(13.0, 13.0)),
            Glyph::Common(Icon::Warning),
            color,
        );
    } else {
        ui.put(
            egui::Rect::from_center_size(pos2(x + 6.0, rect.center().y), vec2(11.0, 11.0)),
            egui::Spinner::new().size(10.0).color(color),
        );
    }
    ui.painter().text(
        pos2(x + 18.0, rect.center().y),
        Align2::LEFT_CENTER,
        first_line(text, 80),
        fonts::regular(12.5),
        color,
    );
}

/// 트리 한 줄: 들여쓰기, 화살표, 아이콘, 이름, 보조 텍스트.
#[allow(clippy::too_many_arguments)]
fn node_row(
    ui: &mut Ui,
    depth: usize,
    expandable: bool,
    open: bool,
    icon: Glyph,
    icon_color: Color32,
    label: &str,
    suffix: &str,
    selected: bool,
) -> egui::Response {
    let theme = Theme::current();
    let (rect, resp) = tree_row(ui, ROW_H, selected, label);
    let x = rect.min.x + 4.0 + depth as f32 * INDENT;
    let cy = rect.center().y;
    chevron(ui, pos2(x + 5.0, cy), open, expandable);
    let p = ui.painter();
    ui::paint_glyph(p, egui::Rect::from_center_size(pos2(x + 19.0, cy), vec2(13.0, 13.0)), icon, icon_color);
    let g = p.text(
        pos2(x + 30.0, cy),
        Align2::LEFT_CENTER,
        label,
        fonts::regular(13.0),
        theme.text,
    );
    if !suffix.is_empty() {
        p.text(
            pos2(g.max.x + 7.0, cy + 0.5),
            Align2::LEFT_CENTER,
            suffix,
            fonts::regular(11.5),
            theme.text_faint,
        );
    }
    resp
}

#[allow(clippy::too_many_arguments)]
fn schema_ui(
    ui: &mut Ui,
    m: &DbManager,
    id: ConnId,
    driver: Driver,
    sc: &mut SchemaNode,
    single: bool,
    filter: &str,
    selected: &mut Option<NodeKey>,
    events: &mut Vec<DbEvent>,
    actions: &mut Vec<PendingAction>,
    confirm: &mut Option<(TypedConfirm, ConfirmKind)>,
) {
    let theme = Theme::current();
    let key = NodeKey::Schema(id, sc.name.clone());
    let label = match driver {
        Driver::Postgres => kiln_common::i18n::tr("스키마"),
        Driver::MySql | Driver::MariaDb => kiln_common::i18n::tr("데이터베이스"),
        Driver::Sqlite => "",
    };
    let resp = node_row(
        ui,
        1,
        true,
        sc.open,
        Glyph::Common(Icon::Database),
        theme.purple,
        &sc.name,
        if single { label } else { "" },
        *selected == Some(key.clone()),
    );
    if resp.clicked() {
        *selected = Some(key);
        sc.open = !sc.open;
    }
    resp.context_menu(|ui| {
        if ui.button(kiln_common::i18n::tr("새로 고침")).clicked() {
            sc.tables = load_tables(m, id, &sc.name);
            ui.close();
        }
        if ui.button(kiln_common::i18n::tr("새 콘솔")).clicked() {
            events.push(DbEvent::OpenConsole { conn: id });
            ui.close();
        }
        if ui.button(kiln_common::i18n::tr("이름 복사")).clicked() {
            ui.ctx().copy_text(sc.name.clone());
            ui.close();
        }
    });
    if !sc.open && filter.is_empty() {
        return;
    }
    if matches!(sc.tables, Load::Idle) {
        sc.tables = load_tables(m, id, &sc.name);
    }
    match &mut sc.tables {
        Load::Idle => {}
        Load::Loading(_) => info_row(ui, 2, kiln_common::i18n::tr("불러오는 중…"), theme.text_faint),
        Load::Failed(e) => {
            let e = e.clone();
            info_row(ui, 2, &e, theme.red);
        }
        Load::Ready(tables) => {
            let schema = sc.name.clone();
            for (is_view, open) in [(false, &mut sc.tables_open), (true, &mut sc.views_open)] {
                let matching: Vec<usize> = tables
                    .iter()
                    .enumerate()
                    .filter(|(_, t)| (t.info.kind != TableKind::Table) == is_view)
                    .filter(|(_, t)| {
                        filter.is_empty() || t.info.name.to_lowercase().contains(filter)
                    })
                    .map(|(i, _)| i)
                    .collect();
                if matching.is_empty() && (is_view || !filter.is_empty()) {
                    continue;
                }
                let show = *open || !filter.is_empty();
                let resp = node_row(
                    ui,
                    2,
                    true,
                    show,
                    Glyph::Common(Icon::Folder),
                    theme.text_faint,
                    if is_view { kiln_common::i18n::tr("뷰") } else { kiln_common::i18n::tr("테이블") },
                    &matching.len().to_string(),
                    false,
                );
                if resp.clicked() {
                    *open = !*open;
                }
                if !show {
                    continue;
                }
                for i in matching {
                    table_ui(
                        ui,
                        m,
                        id,
                        &schema,
                        &mut tables[i],
                        selected,
                        events,
                        actions,
                        confirm,
                    );
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn table_ui(
    ui: &mut Ui,
    m: &DbManager,
    id: ConnId,
    schema: &str,
    t: &mut TableNode,
    selected: &mut Option<NodeKey>,
    events: &mut Vec<DbEvent>,
    actions: &mut Vec<PendingAction>,
    confirm: &mut Option<(TypedConfirm, ConfirmKind)>,
) {
    let theme = Theme::current();
    let key = NodeKey::Table(id, schema.to_string(), t.info.name.clone());
    let (icon, color) = match t.info.kind {
        TableKind::Table => (Glyph::Common(Icon::Table), theme.blue),
        TableKind::View | TableKind::MaterializedView => (Glyph::Common(Icon::Eye), theme.green),
    };
    let suffix = t
        .info
        .row_estimate
        .filter(|n| *n > 0)
        .map(|n| format!("~{}", ui::thousands(n)))
        .unwrap_or_default();
    let resp = node_row(
        ui,
        3,
        true,
        t.open,
        icon,
        color,
        &t.info.name,
        &suffix,
        *selected == Some(key.clone()),
    );
    let tref = TableRef::new(Some(schema.to_string()), t.info.name.clone());
    let resp = if t.info.comment.is_empty() {
        resp
    } else {
        resp.on_hover_text(&t.info.comment)
    };
    if resp.clicked() {
        *selected = Some(key.clone());
        let pos = resp.interact_pointer_pos().unwrap_or_default();
        // 화살표 영역 클릭 시 펼침, 나머지는 선택만.
        if pos.x < resp.rect.min.x + 4.0 + 3.0 * INDENT + 12.0 {
            t.open = !t.open;
        }
    }
    if ui::double_clicked(ui, &resp) {
        events.push(DbEvent::OpenTable {
            conn: id,
            schema: Some(schema.to_string()),
            table: t.info.name.clone(),
        });
    }
    resp.context_menu(|ui| {
        ui.set_min_width(180.0);
        if ui.button(kiln_common::i18n::tr("테이블 열기")).clicked() {
            events.push(DbEvent::OpenTable {
                conn: id,
                schema: Some(schema.to_string()),
                table: t.info.name.clone(),
            });
            ui.close();
        }
        for (label, section) in [("컬럼 보기", TableSection::Structure), ("인덱스 보기", TableSection::Indexes), ("DDL 보기", TableSection::Ddl)] {
            if ui.button(kiln_common::i18n::tr(label)).clicked() {
                events.push(DbEvent::OpenTableSection { conn: id, schema: Some(schema.into()), table: t.info.name.clone(), section });
                ui.close();
            }
        }
        if t.info.kind == TableKind::Table {
            for (label, action) in [
                ("컬럼 추가…", SchemaAction::AddColumn(ColumnSpec { name: String::new(), data_type: "TEXT".into(), nullable: true, default: None })),
                ("인덱스 추가…", SchemaAction::AddIndex(IndexSpec { name: String::new(), columns: Vec::new(), unique: false })),
                ("테이블 이름 변경…", SchemaAction::RenameTable { name: t.info.name.clone() }),
            ] {
                if ui.button(kiln_common::i18n::tr(label)).clicked() {
                    events.push(DbEvent::SchemaAction { conn: id, schema: Some(schema.into()), table: t.info.name.clone(), action });
                    ui.close();
                }
            }
        }
        if ui.button(kiln_common::i18n::tr("새 콘솔")).clicked() {
            events.push(DbEvent::OpenConsole { conn: id });
            ui.close();
        }
        ui.separator();
        if ui.button(kiln_common::i18n::tr("이름 복사")).clicked() {
            ui.ctx().copy_text(t.info.name.clone());
            ui.close();
        }
        if ui.button(kiln_common::i18n::tr("전체 이름 복사")).clicked() {
            let d = m.driver(id).unwrap_or(Driver::Postgres);
            ui.ctx().copy_text(tref.sql_name(d));
            ui.close();
        }
        if ui.button(kiln_common::i18n::tr("DDL 복사")).clicked() {
            let m2 = m.clone();
            let tr = tref.clone();
            actions.push(PendingAction::CopyDdl(
                m.spawn(async move { m2.table_ddl(id, &tr).await }),
            ));
            ui.close();
        }
        if ui.button(kiln_common::i18n::tr("새로 고침")).clicked() {
            t.details = load_details(m, id, tref.clone());
            ui.close();
        }
        ui.separator();
        if t.info.kind == TableKind::Table
            && ui
                .button(RichText::new(kiln_common::i18n::tr("모든 행 삭제…")).color(theme.orange))
                .clicked()
        {
            *confirm = Some((
                TypedConfirm {
                    title: kiln_common::i18n::tr("모든 행 삭제").into(),
                    message: kiln_common::trf!(
                        "{}의 모든 행을 삭제할까요? 이 작업은 되돌릴 수 없습니다.",
                        t.info.name
                    ),
                    expected: t.info.name.clone(),
                    input: String::new(),
                    action_label: kiln_common::i18n::tr("모든 행 삭제").into(),
                },
                ConfirmKind::Truncate(id, tref.clone()),
            ));
            ui.close();
        }
        if ui.button(RichText::new(if t.info.kind==TableKind::Table {kiln_common::i18n::tr("테이블 삭제…")}else{kiln_common::i18n::tr("뷰 삭제…")}).color(theme.red)).clicked() {
            if t.info.kind == TableKind::Table {
                events.push(DbEvent::SchemaAction { conn: id, schema: Some(schema.into()), table: t.info.name.clone(), action: SchemaAction::DropTable });
                ui.close();
                return;
            }
            *confirm = Some((
                TypedConfirm {
                    title: kiln_common::trf!(
                        "{} 삭제",
                        if t.info.kind == TableKind::Table {
                            kiln_common::i18n::tr("테이블")
                        } else {
                            kiln_common::i18n::tr("뷰")
                        }
                    ),
                    message: kiln_common::trf!("다음 객체를 영구 삭제할까요? 이 작업은 되돌릴 수 없습니다.\n{}", t.info.name),
                    expected: t.info.name.clone(),
                    input: String::new(),
                    action_label: if t.info.kind==TableKind::Table {kiln_common::i18n::tr("테이블 삭제").into()}else{kiln_common::i18n::tr("뷰 삭제").into()},
                },
                ConfirmKind::Drop(id, tref.clone(), t.info.kind),
            ));
            ui.close();
        }
    });
    if !t.open {
        return;
    }
    if matches!(t.details, Load::Idle) {
        t.details = load_details(m, id, tref);
    }
    match &mut t.details {
        Load::Idle => {}
        Load::Loading(_) => info_row(ui, 4, kiln_common::i18n::tr("불러오는 중…"), theme.text_faint),
        Load::Failed(e) => {
            let e = e.clone();
            info_row(ui, 4, &e, theme.red);
        }
        Load::Ready(det) => {
            for c in &det.columns {
                let fk = det.fk_target(&c.name);
                let (icon, col) = if c.is_pk() {
                    (Glyph::Common(Icon::Key), theme.yellow)
                } else if fk.is_some() {
                    (Glyph::Link, theme.blue)
                } else {
                    (Glyph::Dot, theme.text_faint)
                };
                let mut suffix = c.data_type.clone();
                if !c.nullable {
                    suffix.push_str(" · NOT NULL");
                }
                let r = node_row(ui, 4, false, false, icon, col, &c.name, &suffix, false);
                let mut tip = format!("{} {}", c.name, c.data_type);
                if let Some(d) = &c.default {
                    tip.push_str(&kiln_common::trf!("\n기본값 {d}"));
                }
                if let Some(f) = fk {
                    tip.push_str(&kiln_common::trf!("\n참조 {f}"));
                }
                r.on_hover_text(tip).context_menu(|ui| {
                    if t.info.kind == TableKind::Table {
                        for (label, action) in [
                            ("컬럼 수정…", SchemaAction::AlterColumn { column: c.name.clone(), spec: ColumnSpec::from(c) }),
                            ("컬럼 이름 변경…", SchemaAction::RenameColumn { column: c.name.clone(), name: c.name.clone() }),
                            ("컬럼 삭제…", SchemaAction::DropColumn { column: c.name.clone() }),
                        ] {
                            if ui.button(kiln_common::i18n::tr(label)).clicked() {
                                events.push(DbEvent::SchemaAction { conn: id, schema: Some(schema.into()), table: t.info.name.clone(), action });
                                ui.close();
                            }
                        }
                        ui.separator();
                    }
                    if ui.button(kiln_common::i18n::tr("이름 복사")).clicked() {
                        ui.ctx().copy_text(c.name.clone());
                        ui.close();
                    }
                });
            }
            if !det.indexes.is_empty() {
                let r = node_row(
                    ui,
                    4,
                    true,
                    t.indexes_open,
                    Glyph::Bolt,
                    theme.orange,
                    kiln_common::i18n::tr("인덱스"),
                    &det.indexes.len().to_string(),
                    false,
                );
                if r.clicked() {
                    t.indexes_open = !t.indexes_open;
                }
                if t.indexes_open {
                    for ix in &det.indexes {
                        let kind = if ix.primary {
                            kiln_common::i18n::tr("기본 키")
                        } else if ix.unique {
                            kiln_common::i18n::tr("고유")
                        } else {
                            ""
                        };
                        let suffix = format!("({}) {kind}", ix.columns.join(", "));
                        node_row(
                            ui,
                            5,
                            false,
                            false,
                            Glyph::Bolt,
                            theme.orange,
                            &ix.name,
                            &suffix,
                            false,
                        ).context_menu(|ui| {
                            if ui.button(kiln_common::i18n::tr("인덱스 보기")).clicked() {
                                events.push(DbEvent::OpenTableSection { conn: id, schema: Some(schema.into()), table: t.info.name.clone(), section: TableSection::Indexes });
                                ui.close();
                            }
                            if ui.add_enabled(t.info.kind == TableKind::Table && !ix.primary && !ix.constraint, egui::Button::new(kiln_common::i18n::tr("인덱스 삭제…"))).clicked() {
                                events.push(DbEvent::SchemaAction { conn: id, schema: Some(schema.into()), table: t.info.name.clone(), action: SchemaAction::DropIndex { name: ix.name.clone() } });
                                ui.close();
                            }
                        });
                    }
                }
            }
            if !det.foreign_keys.is_empty() {
                let r = node_row(
                    ui,
                    4,
                    true,
                    t.fks_open,
                    Glyph::Link,
                    theme.blue,
                    kiln_common::i18n::tr("외래 키"),
                    &det.foreign_keys.len().to_string(),
                    false,
                );
                if r.clicked() {
                    t.fks_open = !t.fks_open;
                }
                if t.fks_open {
                    for fk in &det.foreign_keys {
                        let suffix = format!(
                            "({}) → {}({})",
                            fk.columns.join(", "),
                            fk.ref_table,
                            fk.ref_columns.join(", ")
                        );
                        node_row(
                            ui, 5, false, false, Glyph::Link, theme.blue, &fk.name, &suffix, false,
                        );
                    }
                }
            }
        }
    }
}

/// 드라이버를 로고 카드로 고른다. 누른 드라이버를 돌려준다.
fn driver_cards(ui: &mut Ui, selected: Option<Driver>) -> Option<Driver> {
    let theme = Theme::current();
    let mut picked = None;
    let avail = ui.available_width();
    let cols = if avail > 420.0 { 4 } else { 2 };
    let gap = 8.0;
    let w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).min(150.0);
    let rows: Vec<&[Driver]> = Driver::ALL.chunks(cols).collect();
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for d in row {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 74.0), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, d.label()));
                let sel = selected == Some(*d);
                let fill = if sel { theme.accent_soft(if theme.dark { 30 } else { 22 }) } else if resp.hovered() { theme.bg_hover } else { theme.bg_elevated };
                ui.painter().rect_filled(r, 10.0, fill);
                let stroke = if sel { egui::Stroke::new(1.5, theme.accent) } else { egui::Stroke::new(1.0, theme.border) };
                ui.painter().rect_stroke(r, 10.0, stroke, egui::StrokeKind::Inside);
                crate::logo::paint(ui, egui::Rect::from_center_size(pos2(r.center().x, r.top() + 27.0), vec2(30.0, 30.0)), *d);
                ui.painter().text(pos2(r.center().x, r.bottom() - 15.0), Align2::CENTER_CENTER, d.label(), fonts::medium(12.5), if sel { theme.text } else { theme.text_dim });
                if resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if resp.clicked() {
                    picked = Some(*d);
                }
            }
        });
    }
    picked
}

/// 대화상자용 한 줄 드라이버 선택(로고 + 이름).
fn driver_chips(ui: &mut Ui, selected: Driver, width: f32) -> Option<Driver> {
    let theme = Theme::current();
    let mut picked = None;
    let gap = 6.0;
    let n = Driver::ALL.len() as f32;
    let w = ((width - gap * (n - 1.0)) / n).max(70.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for d in Driver::ALL {
            let (r, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, d.label()));
            let sel = selected == d;
            let fill = if sel { theme.accent_soft(if theme.dark { 30 } else { 22 }) } else if resp.hovered() { theme.bg_hover } else { theme.bg_input };
            ui.painter().rect_filled(r, 8.0, fill);
            let stroke = if sel { egui::Stroke::new(1.5, theme.accent) } else { egui::Stroke::new(1.0, theme.border) };
            ui.painter().rect_stroke(r, 8.0, stroke, egui::StrokeKind::Inside);
            crate::logo::paint(ui, egui::Rect::from_center_size(pos2(r.left() + 17.0, r.center().y), vec2(20.0, 20.0)), d);
            let short = if d == Driver::Postgres { "Postgres" } else { d.label() };
            ui.painter().with_clip_rect(r.shrink(2.0)).text(pos2(r.left() + 32.0, r.center().y), Align2::LEFT_CENTER, short, fonts::medium(12.0), if sel { theme.text } else { theme.text_dim });
            if resp.clicked() {
                picked = Some(d);
            }
        }
    });
    picked
}

#[cfg(test)]
mod dialog_regressions {
    use super::*;
    use egui_kittest::{Harness,kittest::Queryable};
    fn expanded_schemas() -> Vec<SchemaNode> {
        vec![SchemaNode { name: "main".into(), open: true, tables_open: true, views_open: true,
            tables: Load::Ready(vec![TableNode {
                info: TableInfo { schema: Some("main".into()), name: "customers".into(), kind: TableKind::Table, row_estimate: None, comment: String::new() },
                open: true, details: Load::Idle, indexes_open: true, fks_open: true,
            }]),
        }]
    }

    #[test]
    fn explorer_reload_preserves_expansion_without_reconnecting() {
        let manager = DbManager::in_memory();
        let id = ConnId(999);
        let mut node = ConnNode { open: true, schemas: Some(Load::Ready(expanded_schemas())), ..Default::default() };
        node.reload(&manager, id);
        assert!(node.open && node.schemas.is_none());
        let mut fresh = expanded_schemas();
        fresh[0].open = false;
        fresh[0].views_open = false;
        if let Load::Ready(tables) = &mut fresh[0].tables { tables[0].open = false; tables[0].indexes_open = false; }
        node.schemas = Some(Load::Ready(fresh));
        node.restore_expansion();
        let Some(Load::Ready(schemas)) = &mut node.schemas else { panic!() };
        assert!(schemas[0].open && schemas[0].views_open);
        let Load::Ready(tables) = &schemas[0].tables else { panic!() };
        assert!(tables[0].open && tables[0].indexes_open && tables[0].fks_open);
        schemas[0].open = false;
        node.restore_expansion();
        let Some(Load::Ready(schemas)) = &node.schemas else { panic!() };
        assert!(!schemas[0].open, "restoration must not override later user choices");
    }

    #[test]
    fn explorer_reload_discards_old_pending_metadata() {
        let manager = DbManager::in_memory();
        let stale = manager.spawn(async {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            Ok(expanded_schemas())
        });
        let id = ConnId(999);
        let old_epoch = manager.connection_epoch(id).wrapping_add(1);
        let node = ConnNode { open: true, schemas: Some(Load::Loading(stale)), epoch: Some(old_epoch), ..Default::default() };
        let mut panel = DbPanel::new(manager);
        panel.nodes.insert(id, node);
        panel.poll_jobs(&egui::Context::default());
        assert!(panel.nodes[&id].schemas.is_none(), "pending old connection results must never return to the tree");
        assert_eq!(panel.nodes[&id].epoch, Some(panel.manager.connection_epoch(id)));
    }

    #[test]
    fn changed_connection_settings_invalidate_previous_test() {
        let mut d=ConnDialog::new(ConnConfig::default(),"old".into(),true);
        d.tested_settings=Some((d.cfg.clone(),d.password.clone()));
        d.test_result=Some(Ok("server A".into()));
        d.invalidate_changed_test(); assert!(d.test_result.is_some());
        d.cfg.host="server-b.invalid".into();d.invalidate_changed_test();
        assert!(d.test_result.is_none() && d.test_stale);
        d.tested_settings=Some((d.cfg.clone(),d.password.clone()));d.test_result=Some(Err("auth error".into()));
        d.password="new".into();d.invalidate_changed_test();assert!(d.test_result.is_none());
    }
    #[test]
    fn minimum_connection_dialog_keeps_actions_visible_with_long_error() {
        for theme in ["kiln-dark","kiln-light"] {
            Theme::set_current(theme);
            let mut panel=DbPanel::new(DbManager::in_memory());
            let mut d=ConnDialog::new(ConnConfig::default(),String::new(),true);
            d.test_result=Some(Err("연결하지 못했습니다. 입력한 호스트와 인증 정보를 확인하세요. ".repeat(8)));
            panel.dialog=Some(d);
            let mut initialized=false;
            let mut h=Harness::builder().with_size([720.0/1.3,440.0/1.3]).wgpu().build_ui_state(|ui,p:&mut DbPanel| {
                if !initialized {fonts::install(ui.ctx());Theme::current().apply(ui.ctx());initialized=true;return;}
                p.dialog_ui(ui.ctx());
            },panel);
            h.run_steps(5);
            for label in ["연결 테스트","취소","저장"] {
                assert!(h.ctx.content_rect().contains_rect(h.get_by_label(label).rect()),"{theme} {label}");
            }
            h.render().unwrap().save(format!("/tmp/kiln-db-connection-minimum-{theme}.png")).unwrap();
        }
        Theme::set_current("kiln-dark");
    }
}
