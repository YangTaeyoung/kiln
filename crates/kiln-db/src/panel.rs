//! 데이터베이스 탐색기 패널: 연결 목록, 스키마 트리, 연결 편집 대화상자.

use crate::edit::TableRef;
use crate::manager::Job;
use crate::meta::{TableDetails, TableInfo, TableKind};
use crate::ui::{self, TypedConfirm, chevron, dim, icon_button, paint_dot, status_color, tree_row};
use crate::{ConnConfig, ConnId, ConnStatus, DbManager, DbResult, Driver, SslMode};
use egui::{Align2, Color32, FontId, RichText, Ui, pos2, vec2};
use kiln_common::Theme;
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
    OpenConsole {
        conn: ConnId,
    },
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
                *self = Load::Failed("cancelled".into());
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

const ROW_H: f32 = 21.0;
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
            .inner_margin(egui::Margin::symmetric(6, 6))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.header(ui);
                ui.add_space(4.0);
                self.filter_box(ui);
                ui.add_space(4.0);
                if let Some(w) = self.manager.keychain_warning() {
                    ui::banner(ui, &w, true);
                    ui.add_space(4.0);
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
            ui.label(
                RichText::new("DATABASE")
                    .size(11.0)
                    .strong()
                    .color(theme.text_dim),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button(RichText::new("+").size(15.0).color(theme.text_dim), |ui| {
                    ui.set_min_width(180.0);
                    for d in Driver::ALL {
                        if ui.button(format!("{}…", d.label())).clicked() {
                            let cfg = ConnConfig {
                                driver: d,
                                port: d.default_port(),
                                host: if d == Driver::Sqlite {
                                    String::new()
                                } else {
                                    "localhost".into()
                                },
                                ..ConnConfig::default()
                            };
                            self.dialog = Some(ConnDialog::new(cfg, String::new(), true));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Import from URL…").clicked() {
                        let mut d = ConnDialog::new(ConnConfig::default(), String::new(), true);
                        d.url = "postgres://user:password@localhost:5432/db".into();
                        self.dialog = Some(d);
                        ui.close();
                    }
                })
                .response
                .on_hover_text("Add connection");
                if icon_button(ui, "⟳", "Refresh all").clicked() {
                    for (id, n) in self.nodes.iter_mut() {
                        if n.schemas.is_some() {
                            let m = self.manager.clone();
                            let id = *id;
                            n.schemas = Some(load_schemas(&m, id));
                        }
                    }
                }
                if icon_button(ui, "⌨", "New console for selected connection").clicked()
                    && let Some(id) = self.selected_conn()
                {
                    self.events.push(DbEvent::OpenConsole { conn: id });
                }
            });
        });
    }

    fn filter_box(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        egui::Frame::new()
            .fill(theme.bg)
            .stroke(egui::Stroke::new(1.0, theme.border))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("🔍").size(11.0).color(theme.text_faint));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.filter)
                            .id_salt("db-filter")
                            .frame(egui::Frame::NONE)
                            .hint_text("Filter tables")
                            .desired_width(ui.available_width() - 18.0),
                    );
                    if !self.filter.is_empty() && icon_button(ui, "×", "Clear").clicked() {
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
        for n in self.nodes.values_mut() {
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
                                    Some(("DDL copied to clipboard".into(), false, Instant::now()));
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
                                self.toast = Some(("Done".into(), false, Instant::now()));
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

    fn tree(&mut self, ui: &mut Ui) {
        let theme = Theme::current();
        let conns = self.manager.connections();
        if conns.is_empty() {
            ui.add_space(24.0);
            ui.vertical_centered(|ui| {
                ui.label(dim("No connections yet"));
                ui.add_space(6.0);
                if ui::tool_button(ui, "Add connection", true, true).clicked() {
                    self.open_new_connection_dialog();
                }
            });
            return;
        }
        let filter = self.filter.trim().to_lowercase();
        for cfg in conns {
            let id = cfg.id;
            let status = self.manager.status(id);
            let node = self.nodes.entry(id).or_default();
            let sel = self.selected == Some(NodeKey::Conn(id));
            let (rect, resp) = tree_row(ui, ROW_H + 2.0, sel, &cfg.display_name());
            let x0 = rect.min.x + 4.0;
            let cy = rect.center().y;
            if let Some(c) = cfg.color {
                ui.painter().rect_filled(
                    egui::Rect::from_min_size(
                        pos2(rect.min.x, rect.min.y + 3.0),
                        vec2(3.0, rect.height() - 6.0),
                    ),
                    1.0,
                    Color32::from_rgb(c[0], c[1], c[2]),
                );
            }
            chevron(ui, pos2(x0 + 5.0, cy), node.open, true);
            paint_dot(ui, pos2(x0 + 17.0, cy), status_color(&status));
            let name_g = ui.painter().text(
                pos2(x0 + 26.0, cy),
                Align2::LEFT_CENTER,
                cfg.display_name(),
                FontId::proportional(13.0),
                theme.text,
            );
            ui.painter().text(
                pos2(name_g.max.x + 6.0, cy),
                Align2::LEFT_CENTER,
                cfg.driver.label(),
                FontId::proportional(11.0),
                theme.text_faint,
            );
            if matches!(node.schemas, Some(Load::Loading(_))) || status == ConnStatus::Connecting {
                ui.put(
                    egui::Rect::from_center_size(pos2(rect.max.x - 12.0, cy), vec2(12.0, 12.0)),
                    egui::Spinner::new().size(11.0),
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
                    if ui.button("Disconnect").clicked() {
                        self.manager.disconnect(id);
                        node.schemas = None;
                        node.open = false;
                        ui.close();
                    }
                } else if ui.button("Connect").clicked() {
                    node.open = true;
                    node.schemas = Some(load_schemas(&self.manager, id));
                    ui.close();
                }
                if ui.button("New console").clicked() {
                    self.events.push(DbEvent::OpenConsole { conn: id });
                    ui.close();
                }
                if ui.button("Refresh").clicked() {
                    node.schemas = Some(load_schemas(&self.manager, id));
                    node.open = true;
                    ui.close();
                }
                ui.separator();
                if ui.button("Edit…").clicked() {
                    let pw = self.manager.password(id).unwrap_or_default();
                    self.dialog = Some(ConnDialog::new(cfg.clone(), pw, false));
                    ui.close();
                }
                if ui.button("Copy name").clicked() {
                    ui.ctx().copy_text(cfg.display_name());
                    ui.close();
                }
                ui.separator();
                if ui
                    .button(RichText::new("Delete connection…").color(theme.red))
                    .clicked()
                {
                    self.confirm = Some((
                        TypedConfirm {
                            title: "Delete connection".into(),
                            message: format!(
                                "Remove \"{}\" and its stored password?",
                                cfg.display_name()
                            ),
                            expected: cfg.display_name(),
                            input: String::new(),
                            action_label: "Delete".into(),
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
                    info_row(ui, 1, "Connecting…", theme.text_faint);
                }
                Load::Failed(e) => {
                    let e = e.clone();
                    info_row(ui, 1, &format!("⚠ {e}"), theme.red);
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
        let resp = egui::Modal::new(egui::Id::new("db-conn-dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            {
                let w = &mut ui.visuals_mut().widgets;
                w.inactive.bg_fill = theme.bg;
                w.inactive.weak_bg_fill = theme.bg;
                w.inactive.bg_stroke = egui::Stroke::new(1.0, theme.border);
                w.hovered.bg_stroke = egui::Stroke::new(1.0, theme.text_faint);
            }
            ui.label(
                RichText::new(if d.is_new {
                    "New connection"
                } else {
                    "Edit connection"
                })
                .size(15.0)
                .strong(),
            );
            ui.add_space(8.0);
            // URL 가져오기.
            ui.horizontal(|ui| {
                ui.label(dim("URL"));
                let r = ui.add(
                    egui::TextEdit::singleline(&mut d.url)
                        .hint_text("postgres://user:pass@host:5432/db")
                        .desired_width(310.0),
                );
                if ui.button("Import").clicked()
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
                ui.label(RichText::new(e).color(theme.red).size(11.5));
            }
            ui.add_space(6.0);
            egui::Grid::new("db-conn-grid")
                .num_columns(2)
                .spacing(vec2(10.0, 6.0))
                .show(ui, |ui| {
                    ui.label("Name");
                    let hint = d.cfg.display_name();
                    ui.add(
                        egui::TextEdit::singleline(&mut d.cfg.name)
                            .hint_text(hint)
                            .desired_width(300.0),
                    );
                    ui.end_row();
                    ui.label("Driver");
                    egui::ComboBox::from_id_salt("db-driver")
                        .selected_text(d.cfg.driver.label())
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for drv in Driver::ALL {
                                if ui
                                    .selectable_label(d.cfg.driver == drv, drv.label())
                                    .clicked()
                                {
                                    if d.cfg.port == d.cfg.driver.default_port() {
                                        d.cfg.port = drv.default_port();
                                    }
                                    d.cfg.driver = drv;
                                }
                            }
                        });
                    ui.end_row();
                    if d.cfg.driver == Driver::Sqlite {
                        ui.label("File");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut d.cfg.file).desired_width(230.0),
                            );
                            if ui.button("Browse…").clicked()
                                && let Some(p) = rfd::FileDialog::new()
                                    .add_filter("SQLite", &["db", "sqlite", "sqlite3", "db3"])
                                    .add_filter("All files", &["*"])
                                    .pick_file()
                            {
                                d.cfg.file = p.to_string_lossy().into_owned();
                            }
                        });
                        ui.end_row();
                    } else {
                        ui.label("Host");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut d.cfg.host).desired_width(200.0),
                            );
                            ui.label("Port");
                            ui.add(egui::DragValue::new(&mut d.cfg.port).range(1..=65535));
                        });
                        ui.end_row();
                        ui.label("User");
                        ui.add(egui::TextEdit::singleline(&mut d.cfg.user).desired_width(300.0));
                        ui.end_row();
                        ui.label("Password");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut d.password)
                                .password(true)
                                .desired_width(300.0),
                        );
                        if r.changed() {
                            d.password_touched = true;
                        }
                        ui.end_row();
                        ui.label("Database");
                        ui.add(
                            egui::TextEdit::singleline(&mut d.cfg.database).desired_width(300.0),
                        );
                        ui.end_row();
                        ui.label("SSL mode");
                        egui::ComboBox::from_id_salt("db-ssl")
                            .selected_text(d.cfg.ssl_mode.label())
                            .width(140.0)
                            .show_ui(ui, |ui| {
                                for m in SslMode::ALL {
                                    ui.selectable_value(&mut d.cfg.ssl_mode, m, m.label());
                                }
                            });
                        ui.end_row();
                    }
                    ui.label("Timeout");
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut d.cfg.connect_timeout_secs)
                                .range(1..=120)
                                .suffix(" s"),
                        );
                    });
                    ui.end_row();
                    ui.label("Color");
                    ui.horizontal(|ui| {
                        let none_sel = d.cfg.color.is_none();
                        if ui.selectable_label(none_sel, "none").clicked() {
                            d.cfg.color = None;
                        }
                        for c in COLOR_PRESETS {
                            let col = Color32::from_rgb(c[0], c[1], c[2]);
                            let (r, resp) =
                                ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::click());
                            ui.painter().circle_filled(r.center(), 7.0, col);
                            if d.cfg.color == Some(c) {
                                ui.painter().circle_stroke(
                                    r.center(),
                                    8.5,
                                    egui::Stroke::new(1.5, theme.text),
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
                ui.add_space(4.0);
                ui.checkbox(
                    &mut d.cfg.save_password_in_file,
                    "Save password in config file (plain text)",
                );
                if !d.cfg.save_password_in_file {
                    ui.label(dim("Password is stored in the OS keychain."));
                }
            }
            ui.add_space(8.0);
            match (&d.test, &d.test_result) {
                (Some(_), _) => {
                    ui.horizontal(|ui| {
                        ui::spinner(ui);
                        ui.label(dim("Testing connection…"));
                    });
                }
                (None, Some(Ok(v))) => {
                    ui.label(
                        RichText::new(format!("✔ Connected · {}", first_line(v, 90)))
                            .color(theme.green)
                            .size(12.0),
                    );
                }
                (None, Some(Err(e))) => {
                    ui.label(RichText::new(format!("✖ {e}")).color(theme.red).size(12.0));
                }
                _ => {}
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(d.test.is_none(), egui::Button::new("Test connection"))
                    .clicked()
                {
                    let m = self.manager.clone();
                    let cfg = d.cfg.clone();
                    let pw = (!d.password.is_empty()).then(|| d.password.clone());
                    d.test_result = None;
                    d.test = Some(
                        self.manager
                            .spawn(async move { m.test_connection(cfg, pw).await }),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::tool_button(ui, "Save", true, true).clicked() {
                        save = true;
                    }
                    if ui.button("Cancel").clicked() {
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
            .fixed_pos(pos2(rect.min.x + 8.0, rect.max.y - 40.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme.bg_elevated)
                    .stroke(egui::Stroke::new(
                        1.0,
                        if *err { theme.red } else { theme.border },
                    ))
                    .corner_radius(5.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_max_width(rect.width() - 30.0);
                        ui.label(
                            RichText::new(msg.as_str())
                                .color(if *err { theme.red } else { theme.text })
                                .size(12.0),
                        );
                    });
            });
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(250));
    }
}

impl ConnDialog {
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
    ui.painter().text(
        pos2(
            rect.min.x + 8.0 + depth as f32 * INDENT + 12.0,
            rect.center().y,
        ),
        Align2::LEFT_CENTER,
        first_line(text, 80),
        FontId::proportional(12.0),
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
    icon: &str,
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
    p.text(
        pos2(x + 18.0, cy),
        Align2::CENTER_CENTER,
        icon,
        FontId::proportional(12.0),
        icon_color,
    );
    let g = p.text(
        pos2(x + 28.0, cy),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        theme.text,
    );
    if !suffix.is_empty() {
        p.text(
            pos2(g.max.x + 6.0, cy),
            Align2::LEFT_CENTER,
            suffix,
            FontId::proportional(11.0),
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
        Driver::Postgres => "schema",
        Driver::MySql | Driver::MariaDb => "database",
        Driver::Sqlite => "",
    };
    let resp = node_row(
        ui,
        1,
        true,
        sc.open,
        "🗄",
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
        if ui.button("Refresh").clicked() {
            sc.tables = load_tables(m, id, &sc.name);
            ui.close();
        }
        if ui.button("New console").clicked() {
            events.push(DbEvent::OpenConsole { conn: id });
            ui.close();
        }
        if ui.button("Copy name").clicked() {
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
        Load::Loading(_) => info_row(ui, 2, "Loading…", theme.text_faint),
        Load::Failed(e) => {
            let e = e.clone();
            info_row(ui, 2, &format!("⚠ {e}"), theme.red);
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
                    if show { "📂" } else { "📁" },
                    theme.text_faint,
                    if is_view { "Views" } else { "Tables" },
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
        TableKind::Table => ("⊞", theme.blue),
        TableKind::View | TableKind::MaterializedView => ("👁", theme.green),
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
        if ui.button("Open table").clicked() {
            events.push(DbEvent::OpenTable {
                conn: id,
                schema: Some(schema.to_string()),
                table: t.info.name.clone(),
            });
            ui.close();
        }
        if ui.button("New console").clicked() {
            events.push(DbEvent::OpenConsole { conn: id });
            ui.close();
        }
        ui.separator();
        if ui.button("Copy name").clicked() {
            ui.ctx().copy_text(t.info.name.clone());
            ui.close();
        }
        if ui.button("Copy qualified name").clicked() {
            let d = m.driver(id).unwrap_or(Driver::Postgres);
            ui.ctx().copy_text(tref.sql_name(d));
            ui.close();
        }
        if ui.button("Copy DDL").clicked() {
            let m2 = m.clone();
            let tr = tref.clone();
            actions.push(PendingAction::CopyDdl(
                m.spawn(async move { m2.table_ddl(id, &tr).await }),
            ));
            ui.close();
        }
        if ui.button("Refresh").clicked() {
            t.details = load_details(m, id, tref.clone());
            ui.close();
        }
        ui.separator();
        if t.info.kind == TableKind::Table
            && ui
                .button(RichText::new("Truncate…").color(theme.orange))
                .clicked()
        {
            *confirm = Some((
                TypedConfirm {
                    title: "Truncate table".into(),
                    message: format!(
                        "Delete ALL rows from {}? This cannot be undone.",
                        t.info.name
                    ),
                    expected: t.info.name.clone(),
                    input: String::new(),
                    action_label: "Truncate".into(),
                },
                ConfirmKind::Truncate(id, tref.clone()),
            ));
            ui.close();
        }
        if ui.button(RichText::new("Drop…").color(theme.red)).clicked() {
            *confirm = Some((
                TypedConfirm {
                    title: format!(
                        "Drop {}",
                        if t.info.kind == TableKind::Table {
                            "table"
                        } else {
                            "view"
                        }
                    ),
                    message: format!("Permanently drop {}? This cannot be undone.", t.info.name),
                    expected: t.info.name.clone(),
                    input: String::new(),
                    action_label: "Drop".into(),
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
        Load::Loading(_) => info_row(ui, 4, "Loading…", theme.text_faint),
        Load::Failed(e) => {
            let e = e.clone();
            info_row(ui, 4, &format!("⚠ {e}"), theme.red);
        }
        Load::Ready(det) => {
            for c in &det.columns {
                let fk = det.fk_target(&c.name);
                let (icon, col) = if c.is_pk() {
                    ("🔑", theme.yellow)
                } else if fk.is_some() {
                    ("🔗", theme.blue)
                } else {
                    ("•", theme.text_faint)
                };
                let mut suffix = c.data_type.clone();
                if !c.nullable {
                    suffix.push_str(" · not null");
                }
                let r = node_row(ui, 4, false, false, icon, col, &c.name, &suffix, false);
                let mut tip = format!("{} {}", c.name, c.data_type);
                if let Some(d) = &c.default {
                    tip.push_str(&format!("\ndefault {d}"));
                }
                if let Some(f) = fk {
                    tip.push_str(&format!("\nreferences {f}"));
                }
                r.on_hover_text(tip).context_menu(|ui| {
                    if ui.button("Copy name").clicked() {
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
                    "⚡",
                    theme.orange,
                    "Indexes",
                    &det.indexes.len().to_string(),
                    false,
                );
                if r.clicked() {
                    t.indexes_open = !t.indexes_open;
                }
                if t.indexes_open {
                    for ix in &det.indexes {
                        let kind = if ix.primary {
                            "primary"
                        } else if ix.unique {
                            "unique"
                        } else {
                            ""
                        };
                        let suffix = format!("({}) {kind}", ix.columns.join(", "));
                        node_row(
                            ui,
                            5,
                            false,
                            false,
                            "⚡",
                            theme.orange,
                            &ix.name,
                            &suffix,
                            false,
                        );
                    }
                }
            }
            if !det.foreign_keys.is_empty() {
                let r = node_row(
                    ui,
                    4,
                    true,
                    t.fks_open,
                    "🔗",
                    theme.blue,
                    "Foreign keys",
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
                            ui, 5, false, false, "🔗", theme.blue, &fk.name, &suffix, false,
                        );
                    }
                }
            }
        }
    }
}
