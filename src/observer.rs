//! Append-only behavioral observations for real PTY tests.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use serde::Serialize;
use serde_json::{json, Value};

use crate::app::{App, Mode, ViewMode};
use crate::ops::UnnamedRegister;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum ObservedPath {
    Local(String),
    External { external: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PaneSnapshot {
    id: u64,
    cwd: ObservedPath,
    cursor_path: Option<ObservedPath>,
    selected_paths: Vec<ObservedPath>,
    expanded_paths: Vec<ObservedPath>,
    entry_count: usize,
    loading: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RegisterSnapshot {
    Empty,
    Live {
        operation: &'static str,
        paths: Vec<ObservedPath>,
    },
    Deleted {
        item_count: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    mode: &'static str,
    view: &'static str,
    active_tab: usize,
    active_pane: usize,
    panes: Vec<PaneSnapshot>,
    register: RegisterSnapshot,
    status: String,
    status_is_error: bool,
}

pub struct Observer {
    output: BufWriter<File>,
    root: PathBuf,
    seq: u64,
    revision: u64,
    rendered_revision: u64,
    last_snapshot: Option<Snapshot>,
    last_rendered: Option<(u64, u16, u16)>,
    idle: bool,
}

impl Observer {
    pub fn open(path: &Path, root: PathBuf) -> io::Result<Self> {
        let output = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            output: BufWriter::new(output),
            root,
            seq: 0,
            revision: 0,
            rendered_revision: 0,
            last_snapshot: None,
            last_rendered: None,
            idle: false,
        })
    }

    fn emit(&mut self, kind: &str, extra: Value) -> io::Result<()> {
        self.seq += 1;
        let mut record = serde_json::Map::new();
        record.insert("schema".into(), json!(1));
        record.insert("seq".into(), json!(self.seq));
        record.insert("kind".into(), json!(kind));
        record.insert("revision".into(), json!(self.revision));
        if let Value::Object(fields) = extra {
            record.extend(fields);
        }
        serde_json::to_writer(&mut self.output, &record)?;
        self.output.write_all(b"\n")?;
        self.output.flush()
    }

    pub fn started(&mut self, columns: u16, rows: u16) -> io::Result<()> {
        self.emit(
            "started",
            json!({"start_root": ".", "columns": columns, "rows": rows}),
        )
    }

    pub fn observe_state(&mut self, app: &App) -> io::Result<bool> {
        let snapshot = Snapshot::from_app(app, &self.root);
        if self.last_snapshot.as_ref() == Some(&snapshot) {
            return Ok(false);
        }
        self.revision += 1;
        self.last_snapshot = Some(snapshot.clone());
        self.idle = false;
        self.emit("state", serde_json::to_value(snapshot)?)?;
        Ok(true)
    }

    pub fn rendered(&mut self, columns: u16, rows: u16) -> io::Result<()> {
        self.rendered_revision = self.revision;
        let rendered = (self.revision, columns, rows);
        if self.last_rendered == Some(rendered) {
            return Ok(());
        }
        self.last_rendered = Some(rendered);
        self.emit("rendered", json!({"columns": columns, "rows": rows}))
    }

    pub fn input_key(&mut self, key: KeyEvent) -> io::Result<()> {
        self.idle = false;
        self.emit(
            "input",
            json!({"device": "keyboard", "key": normalized_key(key)}),
        )
    }

    pub fn input_mouse(&mut self, mouse: MouseEvent) -> io::Result<()> {
        self.idle = false;
        self.emit("input", json!({"device":"mouse", "mouse_kind": format!("{:?}", mouse.kind).to_lowercase(),
            "column":mouse.column, "row":mouse.row, "modifiers":normalized_modifiers(mouse.modifiers)}))
    }

    pub fn paste_command(
        &mut self,
        app: &App,
        sources: &[PathBuf],
        destination: &Path,
    ) -> io::Result<()> {
        let pane = app.pane();
        self.emit(
            "command",
            json!({"action":"paste", "pane_cwd":self.path(&pane.cwd),
            "cursor_path":pane.current().map(|entry| self.path(&entry.path)),
            "source_paths":sources.iter().map(|p| self.path(p)).collect::<Vec<_>>(),
            "resolved_destination":self.path(destination)}),
        )
    }

    pub fn operation_started(
        &mut self,
        id: u64,
        action: &str,
        destination: &Path,
        item_count: usize,
    ) -> io::Result<()> {
        self.idle = false;
        self.emit(
            "operation_started",
            json!({"operation_id":id,"action":action,
            "destination":self.path(destination),"item_count":item_count}),
        )
    }

    pub fn operation_finished(
        &mut self,
        id: u64,
        committed: usize,
        failed: usize,
        cancelled: bool,
    ) -> io::Result<()> {
        self.emit(
            "operation_finished",
            json!({"operation_id":id,"committed":committed,"failed":failed,"cancelled":cancelled}),
        )
    }

    pub fn maybe_idle(&mut self, app: &App, input_queued: bool) -> io::Result<()> {
        let pending_listings = app.pending_listings();
        let pending_refreshes = app.pending_refreshes();
        let pending_operations = usize::from(app.active_transfer.is_some());
        let quiescent = pending_listings == 0
            && pending_refreshes == 0
            && pending_operations == 0
            && self.rendered_revision == self.revision
            && !input_queued;
        if quiescent && !self.idle {
            self.emit(
                "idle",
                json!({"rendered_revision":self.rendered_revision,
                "pending_operations":0,"pending_listings":0,"pending_refreshes":0}),
            )?;
            self.idle = true;
        } else if !quiescent {
            self.idle = false;
        }
        Ok(())
    }

    pub fn exiting(&mut self, reason: &str) -> io::Result<()> {
        self.emit("exiting", json!({"reason":reason}))
    }

    fn path(&self, path: &Path) -> ObservedPath {
        observed_path(&self.root, path)
    }
}

impl Snapshot {
    fn from_app(app: &App, root: &Path) -> Self {
        let tab = app.tab();
        let panes = tab
            .panes
            .iter()
            .map(|pane| {
                let mut selected_paths = pane
                    .selected
                    .iter()
                    .map(|p| observed_path(root, p))
                    .collect::<Vec<_>>();
                selected_paths.sort_by_key(|p| serde_json::to_string(p).unwrap_or_default());
                let mut expanded_paths = pane
                    .expanded
                    .iter()
                    .map(|p| observed_path(root, p))
                    .collect::<Vec<_>>();
                expanded_paths.sort_by_key(|p| serde_json::to_string(p).unwrap_or_default());
                PaneSnapshot {
                    id: pane.id,
                    cwd: observed_path(root, &pane.cwd),
                    cursor_path: pane.current().map(|e| observed_path(root, &e.path)),
                    selected_paths,
                    expanded_paths,
                    entry_count: pane.visible.len(),
                    loading: pane.loading,
                }
            })
            .collect();
        let register = match &app.register {
            UnnamedRegister::Empty => RegisterSnapshot::Empty,
            UnnamedRegister::Live { paths, cut } => RegisterSnapshot::Live {
                operation: if *cut { "move" } else { "copy" },
                paths: paths.iter().map(|p| observed_path(root, p)).collect(),
            },
            UnnamedRegister::Deleted { items } => RegisterSnapshot::Deleted {
                item_count: items.len(),
            },
        };
        Self {
            mode: mode_name(&app.mode),
            view: view_name(app.pane().view),
            active_tab: app.active_tab,
            active_pane: tab.active,
            panes,
            register,
            status: app.status.clone(),
            status_is_error: app.status_is_error,
        }
    }
}

pub fn observed_path(root: &Path, path: &Path) -> ObservedPath {
    match path.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ObservedPath::Local(".".into()),
        Ok(relative) => ObservedPath::Local(relative.to_string_lossy().replace('\\', "/")),
        Err(_) => ObservedPath::External {
            external: path.to_string_lossy().into_owned(),
        },
    }
}

fn mode_name(mode: &Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Visual => "visual",
        Mode::VisualLine => "visual_line",
        Mode::VisualBlock => "visual_block",
        Mode::Command => "command",
        Mode::Search => "search",
        Mode::Filter => "filter",
        Mode::PathEdit => "path_edit",
        Mode::Rename(_) => "rename",
        Mode::BatchRename => "batch_rename",
        Mode::Confirm(_) => "confirm",
        Mode::NewFolder(_) => "new_folder",
        Mode::NewFile(_) => "new_file",
        Mode::Properties => "properties",
        Mode::Help => "help",
        Mode::CrumbMenu(_) => "crumb_menu",
        Mode::Buttons(_) => "buttons",
        Mode::Menu(_) => "menu",
    }
}
fn view_name(view: ViewMode) -> &'static str {
    match view {
        ViewMode::Icons => "icons",
        ViewMode::Compact => "compact",
        ViewMode::Details => "details",
    }
}
fn normalized_modifiers(m: KeyModifiers) -> Vec<&'static str> {
    let mut out = Vec::new();
    if m.contains(KeyModifiers::CONTROL) {
        out.push("Ctrl")
    }
    if m.contains(KeyModifiers::ALT) {
        out.push("Alt")
    }
    if m.contains(KeyModifiers::SHIFT) {
        out.push("Shift")
    }
    if m.contains(KeyModifiers::SUPER) {
        out.push("Super")
    }
    out
}
fn normalized_key(key: KeyEvent) -> String {
    let mut parts = normalized_modifiers(key.modifiers)
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let code = match key.code {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Backspace => "Backspace".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "BackTab".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::Delete => "Delete".into(),
        KeyCode::Insert => "Insert".into(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    parts.push(code);
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_relative_or_honestly_external() {
        let root = Path::new("/tmp/run/root");
        assert_eq!(
            observed_path(root, Path::new("/tmp/run/root/a")),
            ObservedPath::Local("a".into())
        );
        assert!(matches!(
            observed_path(root, Path::new("/etc/passwd")),
            ObservedPath::External { .. }
        ));
    }

    #[test]
    fn records_are_valid_sequenced_and_suppress_duplicate_state_and_render() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("dolvim-observer-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let output = root.join("events.jsonl");
        let mut app = App::new(root.clone());
        app.pane_mut().loading = false;
        let mut observer = Observer::open(&output, root.clone()).unwrap();
        observer.started(80, 24).unwrap();
        assert!(observer.observe_state(&app).unwrap());
        assert!(!observer.observe_state(&app).unwrap());
        observer.rendered(80, 24).unwrap();
        observer.rendered(80, 24).unwrap();
        observer.rendered(81, 24).unwrap();
        observer.maybe_idle(&app, false).unwrap();
        drop(observer);
        let rows = std::fs::read_to_string(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            rows.iter()
                .map(|row| row["seq"].as_u64().unwrap())
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(rows.iter().filter(|row| row["kind"] == "state").count(), 1);
        assert_eq!(
            rows.iter().filter(|row| row["kind"] == "rendered").count(),
            2
        );
        assert_eq!(rows[1]["revision"], 1);
        assert_eq!(rows[2]["kind"], "rendered");
        assert_eq!(rows[3]["columns"], 81);
        assert_eq!(rows[4]["kind"], "idle");
        std::fs::remove_dir_all(root).unwrap();
    }
}
