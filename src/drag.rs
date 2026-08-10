//! Drag and drop.
//!
//! Internal drags (pane→pane, onto a folder, onto a Places entry) are drawn
//! and handled entirely inside the ratatui buffer. Crossing the terminal
//! boundary needs a real X11 client, so we delegate to `ripdrag`/`dragon-drop`
//! and position its window over the cell the drag started from — PLAN.md §Drag.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::app::{App, RevealIntent};
use crate::ops;

const DRAG_HELPER_BINARIES: &[&str] = &["ripdrag", "dragon-drop", "dragon"];

/// An installed drag helper: the binary's name, and where it lives.
struct DragHelper {
    name: &'static str,
    binary: PathBuf,
}

fn find_drag_helper() -> Option<DragHelper> {
    DRAG_HELPER_BINARIES.iter().find_map(|helper_name| {
        ops::which(helper_name).map(|helper_path| DragHelper {
            name: helper_name,
            binary: helper_path,
        })
    })
}

/// Terminal window origin and per-cell size, so the helper window can be put
/// over the file the user grabbed. X11 only; on Wayland we let the WM place it.
fn cell_origin(col: u16, row: u16) -> Option<(i32, i32)> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return None;
    }
    let xdotool_output = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()?;
    let window_id = String::from_utf8_lossy(&xdotool_output.stdout)
        .trim()
        .to_string();
    if window_id.is_empty() {
        return None;
    }
    let geo = Command::new("xdotool")
        .args(["getwindowgeometry", "--shell", &window_id])
        .output()
        .ok()?;
    let geometry_text = String::from_utf8_lossy(&geo.stdout);
    let geometry_field = |field_name: &str| -> Option<i32> {
        geometry_text
            .lines()
            .find_map(|line| line.strip_prefix(field_name)?.strip_prefix('='))
            .and_then(|value| value.trim().parse().ok())
    };
    let (x, y, window_width_px, window_height_px) = (
        geometry_field("X")?,
        geometry_field("Y")?,
        geometry_field("WIDTH")?,
        geometry_field("HEIGHT")?,
    );
    let (cols, rows) = crossterm::terminal::size().ok()?;
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((
        x + (window_width_px / cols as i32) * col as i32,
        y + (window_height_px / rows as i32) * row as i32,
    ))
}

/// Hand the selection to an external drag helper. `D` in Normal mode, or a
/// mouse drag that leaves the terminal's business and enters the WM's.
pub fn drag_out(app: &mut App) {
    let paths = ops::normalize_operands(app.pane().selected_paths());
    if paths.is_empty() {
        return;
    }
    let Some(helper) = find_drag_helper() else {
        app.error("Drag out needs `ripdrag` or `dragon-drop` on PATH — install one");
        return;
    };
    let mut drag_out_cmd = Command::new(&helper.binary);
    drag_out_cmd.arg("--and-exit");
    if helper.name == "ripdrag" {
        if let Some((x, y)) = cell_origin(app.pane().area.x, app.pane().area.y) {
            drag_out_cmd.args(["-x", &x.to_string(), "-y", &y.to_string()]);
        }
    }
    drag_out_cmd
        .args(&paths)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match drag_out_cmd.spawn() {
        Ok(_) => app.info(format!(
            "Dragging {} item(s) via {}",
            paths.len(),
            helper.name
        )),
        Err(spawn_error) => app.error(format!("{} failed: {spawn_error}", helper.name)),
    }
}

/// Receive a drop from another application: the helper prints the dropped
/// paths, and we copy them into the current directory.
pub fn drop_in(app: &mut App) {
    let Some(helper) = find_drag_helper() else {
        app.error("Drop in needs `ripdrag` or `dragon-drop` on PATH — install one");
        return;
    };
    let mut drop_in_cmd = Command::new(&helper.binary);
    if helper.name == "ripdrag" {
        drop_in_cmd.args(["--target", "--and-exit", "--print-path"]);
    } else {
        drop_in_cmd.args(["--target", "--print-path"]);
    }
    let helper_output = match drop_in_cmd.output() {
        Ok(helper_output) => helper_output,
        Err(spawn_error) => {
            app.error(format!("{} failed: {spawn_error}", helper.name));
            return;
        }
    };
    let paths: Vec<PathBuf> = String::from_utf8_lossy(&helper_output.stdout)
        .lines()
        .map(|line| line.trim().trim_start_matches("file://"))
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        app.info("Nothing was dropped");
        return;
    }
    let dest = app.pane().cwd.clone();
    let progress = ops::start_transfer(paths, dest, ops::TransferKind::Copy);
    app.begin_transfer(progress, None);
}

/// Complete an internal drag onto `dest`. Shift forces a move, Ctrl a copy,
/// and the default is Dolphin's: move within a filesystem, copy across one.
pub fn drop_internal(app: &mut App, dest: PathBuf, shift: bool, ctrl: bool, reveal: RevealIntent) {
    let Some(active_drag) = app.drag.take() else {
        return;
    };
    if active_drag
        .paths
        .iter()
        .any(|source_path| dest.starts_with(source_path))
    {
        app.error("Cannot drop a folder into itself");
        return;
    }
    if active_drag
        .paths
        .iter()
        .any(|source_path| source_path.parent() == Some(dest.as_path()))
        && !shift
    {
        app.info("Already there");
        return;
    }
    let transfer_kind = if !ctrl && (shift || same_device(&active_drag.paths[0], &dest)) {
        ops::TransferKind::Move
    } else {
        ops::TransferKind::Copy
    };
    let progress = ops::start_transfer(active_drag.paths, dest, transfer_kind);
    app.begin_transfer_from(progress, Some(reveal), active_drag.source_pane_id);
}

fn same_device(source: &std::path::Path, dest: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let source_dev = std::fs::metadata(source)
        .map(|metadata| metadata.dev())
        .ok();
    let dest_dev = std::fs::metadata(dest).map(|metadata| metadata.dev()).ok();
    source_dev.is_some() && source_dev == dest_dev
}
