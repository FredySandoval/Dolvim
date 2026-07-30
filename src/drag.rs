//! Drag and drop.
//!
//! Internal drags (pane→pane, onto a folder, onto a Places entry) are drawn
//! and handled entirely inside the ratatui buffer. Crossing the terminal
//! boundary needs a real X11 client, so we delegate to `ripdrag`/`dragon-drop`
//! and position its window over the cell the drag started from — PLAN.md §Drag.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use crate::app::App;
use crate::ops;

const HELPERS: &[&str] = &["ripdrag", "dragon-drop", "dragon"];

fn helper() -> Option<(&'static str, PathBuf)> {
    HELPERS.iter().find_map(|h| ops::which(h).map(|p| (*h, p)))
}

/// Terminal window origin and per-cell size, so the helper window can be put
/// over the file the user grabbed. X11 only; on Wayland we let the WM place it.
fn cell_origin(col: u16, row: u16) -> Option<(i32, i32)> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        return None;
    }
    let id = Command::new("xdotool")
        .arg("getactivewindow")
        .output()
        .ok()?;
    let id = String::from_utf8_lossy(&id.stdout).trim().to_string();
    if id.is_empty() {
        return None;
    }
    let geo = Command::new("xdotool")
        .args(["getwindowgeometry", "--shell", &id])
        .output()
        .ok()?;
    let txt = String::from_utf8_lossy(&geo.stdout);
    let get = |k: &str| -> Option<i32> {
        txt.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .and_then(|v| v.trim().parse().ok())
    };
    let (x, y, w, h) = (get("X")?, get("Y")?, get("WIDTH")?, get("HEIGHT")?);
    let (cols, rows) = crossterm::terminal::size().ok()?;
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((
        x + (w / cols as i32) * col as i32,
        y + (h / rows as i32) * row as i32,
    ))
}

/// Hand the selection to an external drag helper. `D` in Normal mode, or a
/// mouse drag that leaves the terminal's business and enters the WM's.
pub fn drag_out(app: &mut App) {
    let paths = app.pane().selected_paths();
    if paths.is_empty() {
        return;
    }
    let Some((name, bin)) = helper() else {
        app.error("Drag out needs `ripdrag` or `dragon-drop` on PATH — install one");
        return;
    };
    let mut cmd = Command::new(bin);
    if name == "ripdrag" {
        cmd.arg("--and-exit");
        if let Some((x, y)) = cell_origin(app.pane().area.x, app.pane().area.y) {
            cmd.args(["-x", &x.to_string(), "-y", &y.to_string()]);
        }
    } else {
        cmd.arg("--and-exit");
    }
    cmd.args(&paths).stdout(Stdio::null()).stderr(Stdio::null());
    match cmd.spawn() {
        Ok(_) => app.info(format!("Dragging {} item(s) via {name}", paths.len())),
        Err(e) => app.error(format!("{name} failed: {e}")),
    }
}

/// Receive a drop from another application: the helper prints the dropped
/// paths, and we copy them into the current directory.
pub fn drop_in(app: &mut App) {
    let Some((name, bin)) = helper() else {
        app.error("Drop in needs `ripdrag` or `dragon-drop` on PATH — install one");
        return;
    };
    let mut cmd = Command::new(bin);
    if name == "ripdrag" {
        cmd.args(["--target", "--and-exit", "--print-path"]);
    } else {
        cmd.args(["--target", "--print-path"]);
    }
    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            app.error(format!("{name} failed: {e}"));
            return;
        }
    };
    let paths: Vec<PathBuf> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().trim_start_matches("file://"))
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        app.info("Nothing was dropped");
        return;
    }
    let dest = app.pane().cwd.clone();
    app.progress = Some(ops::start_transfer(paths, dest, false));
}

/// Complete an internal drag onto `dest`. Shift forces a move, Ctrl a copy,
/// and the default is Dolphin's: move within a filesystem, copy across one.
pub fn drop_internal(app: &mut App, dest: PathBuf, shift: bool, ctrl: bool) {
    let Some(d) = app.drag.take() else { return };
    if d.paths.iter().any(|p| dest.starts_with(p)) {
        app.error("Cannot drop a folder into itself");
        return;
    }
    if d.paths.iter().any(|p| p.parent() == Some(dest.as_path())) && !shift {
        app.info("Already there");
        return;
    }
    let move_it = if ctrl {
        false
    } else if shift {
        true
    } else {
        same_device(&d.paths[0], &dest)
    };
    app.progress = Some(ops::start_transfer(d.paths, dest, move_it));
}

fn same_device(a: &std::path::Path, b: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let da = std::fs::metadata(a).map(|m| m.dev()).ok();
    let db = std::fs::metadata(b).map(|m| m.dev()).ok();
    da.is_some() && da == db
}
