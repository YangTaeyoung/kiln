//! External file/folder requests share one routing policy at launch and at runtime.

use super::{Action, KilnApp, Pane, PaneId, PaneKind, ToastKind, normalize_path, tools};
use kiln_common::paths::Canonical;
use std::path::{Path, PathBuf};

struct Target {
    path: PathBuf,
    folder: bool,
}

fn resolve(path: PathBuf) -> Result<Target, String> {
    let metadata = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(path.display().to_string());
    }
    let resolved = path
        .canonical()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Target {
        path: normalize_path(resolved),
        folder: metadata.is_dir(),
    })
}

impl KilnApp {
    /// Process the entire batch even when individual paths fail. Explicit folders
    /// are registered first so file/folder ordering never changes workspace choice.
    pub(super) fn open_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) -> bool {
        let mut success = true;
        let mut targets = Vec::new();
        for path in paths {
            match resolve(path) {
                Ok(target) => targets.push(target),
                Err(error) => {
                    success = false;
                    self.toast(
                        kiln_common::i18n::tr("열 수 없습니다"),
                        error,
                        ToastKind::Error,
                        None,
                    );
                }
            }
        }
        for target in targets.iter().filter(|target| target.folder) {
            self.add_workspace(target.path.clone(), ctx);
        }
        for target in targets {
            if target.folder {
                if self
                    .workspaces
                    .iter()
                    .any(|ws| ws.tools.canonical_root() == target.path)
                {
                    self.add_workspace(target.path, ctx);
                } else {
                    success = false;
                }
            } else {
                success &= self.open_editor_path(target.path, ctx);
            }
        }
        success
    }

    fn open_editor_pane(&self, path: &Path) -> Option<PaneId> {
        self.workspaces
            .iter()
            .flat_map(|ws| ws.all_panes())
            .find(|id| {
                self.panes.get(id).and_then(Pane::tool).is_some_and(|tab| {
                    tab.key().starts_with("file:")
                        && tab.path().is_some_and(|p| {
                            p == path || p.canonical().is_ok_and(|p| normalize_path(p) == path)
                        })
                })
            })
    }

    fn open_editor_path(&mut self, path: PathBuf, ctx: &egui::Context) -> bool {
        // Reuse across ALL workspaces before choosing a root. Repeated Finder
        // requests (including aliases) must never reload or replace unsaved text.
        let mut pane = self.open_editor_pane(&path);
        if pane.is_none() {
            let workspace = self
                .workspaces
                .iter()
                .enumerate()
                .filter_map(|(i, ws)| {
                    let root = normalize_path(ws.tools.canonical_root().to_path_buf());
                    path.starts_with(&root)
                        .then_some((i, root.components().count()))
                })
                .max_by_key(|(i, depth)| (*depth, *i == self.active, std::cmp::Reverse(*i)))
                .map(|(i, _)| i);
            if let Some(workspace) = workspace {
                self.active = workspace;
            } else if let Some(parent) = path.parent() {
                self.add_workspace(parent.to_path_buf(), ctx);
            }
            // A failed/inaccessible parent must not route to an unrelated workspace.
            if !self
                .workspaces
                .get(self.active)
                .is_some_and(|ws| path.starts_with(ws.tools.canonical_root()))
            {
                return false;
            }
            self.apply(
                Action::OpenTab(tools::open_file_factory(path.clone(), None, None)),
                ctx,
            );
            pane = self.open_editor_pane(&path);
        }
        if let Some(pane) = pane {
            self.reveal_pane(pane);
            if let Some(Pane {
                kind: PaneKind::Tool(tab),
                ..
            }) = self.panes.get_mut(&pane)
            {
                tab.request_focus();
            }
            self.focus_terminal = false;
            self.reveal_work_surface(ctx);
            ctx.request_repaint();
            true
        } else {
            false
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn open_native_paths(&mut self, ctx: &egui::Context) {
        for request in super::macos::take_open_requests() {
            let success = self.open_paths(request.paths.clone(), ctx);
            super::macos::finish_open_request(&request, success);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;

    fn isolated(name: &str, check: impl FnOnce(&Path)) {
        if let Some(root) = std::env::var_os("KILN_OPEN_FILES_TEST_ROOT") {
            check(Path::new(&root));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", name, "--nocapture", "--test-threads=1"])
            .env("KILN_OPEN_FILES_TEST_ROOT", dir.path())
            .env("KILN_CONFIG_DIR", dir.path().join("config"))
            .env("KILN_SOCKET", dir.path().join("unserved.sock"))
            .env("KILN_ACCOUNTS_SANDBOX", dir.path().join("accounts"))
            .env("KILN_DB_NO_KEYCHAIN", "1")
            .env("KILN_NO_AUTO_UPGRADE", "1")
            // No GUI helper or session daemon is launched by this regression.
            .env("KILN_EXE", dir.path().join("no-executable"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn startup_file_uses_parent_workspace() {
        isolated(
            "app::open_files::tests::startup_file_uses_parent_workspace",
            |root| {
                let file = root.join("README");
                std::fs::write(&file, "# Extensionless text\n").unwrap();
                let ctx = egui::Context::default();
                let app = KilnApp::new(&ctx, Some(file.clone()));
                assert_eq!(app.workspaces.len(), 1);
                assert_eq!(app.workspaces[app.active].root, root.canonical().unwrap());
                assert!(
                    app.debug_tool_keys()
                        .contains(&format!("file:{}", file.canonical().unwrap().display()))
                );
            },
        );
    }

    #[test]
    fn multiple_files_use_explicit_folder_and_longest_existing_workspace() {
        isolated(
            "app::open_files::tests::multiple_files_use_explicit_folder_and_longest_existing_workspace",
            |root| {
                let project = root.join("project");
                let src = project.join("src");
                std::fs::create_dir_all(&src).unwrap();
                let source = src.join("first.rs");
                let second = src.join("second.rs");
                let third = src.join("third.rs");
                let config = project.join(".env");
                for file in [&source, &second, &third, &config] {
                    std::fs::write(file, "fixture text\n").unwrap();
                }
                let ctx = egui::Context::default();
                // The folder can follow its files in both CLI and native batches.
                let mut app =
                    KilnApp::new_paths(&ctx, vec![source.clone(), config.clone(), project.clone()]);
                assert_eq!(app.workspaces.len(), 1);
                assert_eq!(app.workspaces[0].root, project.canonical().unwrap());
                assert_eq!(app.debug_tool_keys().len(), 2);
                assert!(app.open_paths(vec![src.clone(), second.clone()], &ctx));
                assert_eq!(app.workspaces.len(), 2);
                assert_eq!(app.workspaces[app.active].root, src.canonical().unwrap());
                let second_pane = app.focused_pane().unwrap();
                assert!(!app.focus_terminal, "file requests focus the editor");
                assert!(app.open_paths(vec![root.to_path_buf()], &ctx));
                assert_eq!(app.workspaces.len(), 3);
                assert!(app.open_paths(vec![third], &ctx));
                assert_eq!(
                    app.workspaces[app.active].root,
                    src.canonical().unwrap(),
                    "an unopened file chooses the longest root even when another workspace is active"
                );
                assert!(app.open_paths(vec![second, src, project], &ctx));
                assert_eq!(
                    app.workspaces.len(),
                    3,
                    "folders reuse their exact workspace"
                );
                assert_eq!(app.debug_tool_keys().len(), 4);
                assert!(app.panes.contains_key(&second_pane));
            },
        );
    }

    #[test]
    fn repeated_file_reveals_original_unsaved_editor_across_workspaces() {
        isolated(
            "app::open_files::tests::repeated_file_reveals_original_unsaved_editor_across_workspaces",
            |root| {
                let project = root.join("project");
                let src = project.join("src");
                std::fs::create_dir_all(&src).unwrap();
                let file = src.join("draft.txt");
                std::fs::write(&file, "original").unwrap();
                let ctx = egui::Context::default();
                let mut app = KilnApp::new_paths(&ctx, vec![project.clone(), file.clone()]);
                let original_workspace = app.active;
                let pane = app.focused_pane().unwrap();
                let mut editor = kiln_editor::Editor::open(&file).unwrap();
                editor.select_all();
                editor.insert_text("unsaved 한글");
                let draft = editor.recovery_draft().unwrap();
                let tab = app.workspaces[original_workspace]
                    .tools
                    .restore_tool(
                        &state::ToolP::EditorDraft {
                            path: file.clone(),
                            draft: draft.clone(),
                            language_override: None,
                        },
                        &ctx,
                    )
                    .unwrap();
                app.panes.get_mut(&pane).unwrap().kind = PaneKind::Tool(tab);
                assert!(app.open_paths(vec![src.clone(), root.to_path_buf()], &ctx));
                std::fs::write(&file, "disk changed externally").unwrap();
                let request = {
                    #[cfg(unix)]
                    {
                        let alias = root.join("file alias");
                        std::os::unix::fs::symlink(&file, &alias).unwrap();
                        alias
                    }
                    #[cfg(not(unix))]
                    {
                        src.join("..").join("src").join("draft.txt")
                    }
                };
                assert!(app.open_paths(vec![request, file.clone()], &ctx));
                assert_eq!(
                    app.active, original_workspace,
                    "existing buffer wins over a newly added deeper workspace"
                );
                assert_eq!(app.focused_pane(), Some(pane));
                assert_eq!(app.debug_tool_keys().len(), 1);
                let tab = app.panes[&pane].tool().unwrap();
                assert!(tab.is_dirty());
                let Some(state::ToolP::EditorDraft { draft: after, .. }) = tab.persist() else {
                    panic!("missing unsaved buffer")
                };
                assert_eq!(
                    after, draft,
                    "reopening must not reload, replace, or save the buffer"
                );
                assert_eq!(
                    std::fs::read_to_string(file).unwrap(),
                    "disk changed externally"
                );
            },
        );
    }

    #[test]
    fn bad_paths_do_not_drop_good_files_and_binary_files_cannot_be_saved() {
        isolated(
            "app::open_files::tests::bad_paths_do_not_drop_good_files_and_binary_files_cannot_be_saved",
            |root| {
                let text = root.join("unknown.custom-extension");
                let binary = root.join("binary");
                let bytes = b"\0\x01\x02binary\xff";
                std::fs::write(&text, "ordinary text").unwrap();
                std::fs::write(&binary, bytes).unwrap();
                let ctx = egui::Context::default();
                let mut app = KilnApp::new(&ctx, Some(root.to_path_buf()));
                assert!(!app.open_paths(vec![root.join("missing"), text, binary.clone()], &ctx));
                assert_eq!(app.debug_tool_keys().len(), 2);
                assert_eq!(app.workspaces.len(), 1);
                assert_eq!(app.toasts.len(), 1);
                let mut editor = kiln_editor::Editor::open(&binary).unwrap();
                assert!(editor.is_binary());
                assert!(editor.save().is_err());
                assert_eq!(std::fs::read(&binary).unwrap(), bytes);
                let rejected_folder = root.join("x".repeat(101));
                std::fs::create_dir(&rejected_folder).unwrap();
                assert!(
                    !app.open_paths(vec![rejected_folder], &ctx),
                    "a workspace rejected by validation must not acknowledge native success"
                );
                assert_eq!(app.workspaces.len(), 1);
                #[cfg(unix)]
                {
                    let special = root.join("not-a-regular-file");
                    let _listener = std::os::unix::net::UnixListener::bind(&special).unwrap();
                    assert!(!app.open_paths(vec![special], &ctx));
                    assert_eq!(app.debug_tool_keys().len(), 2);
                }
            },
        );
    }
}
