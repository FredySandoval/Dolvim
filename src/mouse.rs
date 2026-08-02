//! Mouse handling. Everything the toolbar, breadcrumb, headers, places panel
//! and status bar draw is clickable, hit-tested against the rects the last
//! render actually produced.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

use crate::app::{App, Drag, Focus, MenuKind, Mode, ViewMode};
use crate::config;
use crate::drag;
use crate::places::Target;
use crate::vim;
use crate::vim::Action;

fn hit(r: Rect, x: u16, y: u16) -> bool {
    r.width > 0 && r.height > 0 && x >= r.x && x < r.right() && y >= r.y && y < r.bottom()
}

/// Double-click window. Dolphin selects on the first click and opens on the
/// second — verified on a stock install with no `SingleClick` key set in
/// kdeglobals. We do the same.
const DOUBLE_MS: u64 = 400;

pub fn handle(app: &mut App, m: MouseEvent) {
    // Modal, for the reason given in `vim::key` — a click can reach Paste
    // through the menu just as a key can, so blocking only the keyboard would
    // leave the orphaned-transfer hole open.
    if app.progress.is_some() {
        return;
    }
    let (x, y) = (m.column, m.row);
    let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);
    let shift = m.modifiers.contains(KeyModifiers::SHIFT);

    match m.kind {
        MouseEventKind::ScrollDown => scroll(app, x, y, 1),
        MouseEventKind::ScrollUp => scroll(app, x, y, -1),
        MouseEventKind::Down(MouseButton::Left) => press(app, x, y, ctrl, shift),
        MouseEventKind::Down(MouseButton::Middle) => middle(app, x, y),
        MouseEventKind::Down(MouseButton::Right) => {
            hit_pane(app, x, y);
            app.mode = Mode::Menu(MenuKind::Hamburger);
            app.menu_sel = 0;
        }
        MouseEventKind::Drag(MouseButton::Left) => dragging(app, x, y),
        MouseEventKind::Up(MouseButton::Left) => release(app, x, y, shift, ctrl),
        MouseEventKind::Moved => hover(app, x, y),
        _ => {}
    }
}

/// Over one of the two toolbar buttons that drop a menu.
fn on_menu_button(app: &App, x: u16, y: u16) -> bool {
    hit(app.hits.view_menu, x, y) || hit(app.hits.menu, x, y)
}

/// The one thing the pointer does without a click: a toolbar button that drops
/// a menu opens it on the way past, so the row behaves like a menu bar. It only
/// ever opens — leaving does not close, or the menu would vanish from under a
/// pointer on its way to the item it wants. Click or Esc dismisses, as before.
///
/// This is the single exception to "no hover state" in `docs/DECISIONS.md`.
/// Nothing else tracks the pointer, and no redraw is added: the loop already
/// draws every tick and this changes no state unless the pointer is over one
/// of two buttons.
fn hover(app: &mut App, x: u16, y: u16) {
    // Never take a mode that is in the middle of something.
    if !matches!(app.mode, Mode::Normal | Mode::Buttons(_) | Mode::Menu(_)) {
        return;
    }
    let h = app.hits.clone();
    // The caret, not the icon beside it: on a split button the icon is the
    // action and only the caret drops the list, which is how clicking works too.
    let want = if hit(h.view_menu, x, y) {
        MenuKind::ViewMode
    } else if hit(h.menu, x, y) {
        MenuKind::Hamburger
    } else {
        return;
    };
    if app.mode != Mode::Menu(want.clone()) {
        app.mode = Mode::Menu(want);
        app.menu_sel = 0;
    }
}

fn scroll(app: &mut App, x: u16, y: u16, dir: isize) {
    if app.places_visible && hit(app.hits.places, x, y) {
        let n = app.places.len();
        let next = (app.places_sel as isize + dir).clamp(0, n as isize - 1) as usize;
        if app.places[next].is_selectable() {
            app.places_sel = next;
        }
        return;
    }
    if let Some(i) = pane_at(app, x, y) {
        let p = app.pane_at_mut(i);
        let step = match p.view {
            ViewMode::Details => 3,
            _ => 1,
        };
        // The selection does not follow the viewport: in Dolphin the wheel
        // moves the view and leaves the current item exactly where it is, off
        // screen if that is where it ends up. Scrolling is looking around, not
        // choosing. The bound is the last screenful, so the wheel cannot run
        // off into blank space.
        let max = p.max_offset() as isize;
        p.offset = (p.offset as isize + dir * step).clamp(0, max) as usize;
    }
}

fn press(app: &mut App, x: u16, y: u16, ctrl: bool, shift: bool) {
    // An open menu swallows the click: inside picks, outside dismisses.
    if let Mode::Menu(kind) = app.mode.clone() {
        let items = vim::menu_items(&kind);
        if let Some(i) = menu_index(app, x, y, items.len()) {
            let a = items[i].1;
            vim::leave_toolbar(app);
            vim::act(app, a, 1);
        } else if !on_menu_button(app, x, y) {
            // Clicking the button the menu hangs from would close it, and the
            // next pointer motion would open it again. Leave it be.
            app.mode = Mode::Normal;
        }
        return;
    }
    if let Mode::CrumbMenu(seg) = app.mode.clone() {
        let items = vim::crumb_siblings(app, seg);
        if let Some(i) = menu_index(app, x, y, items.len()) {
            let d = items[i].clone();
            vim::leave_toolbar(app);
            app.goto(Target::Dir(d), true);
        } else {
            app.mode = Mode::Normal;
        }
        return;
    }
    if matches!(app.mode, Mode::Properties | Mode::Help | Mode::Confirm(_)) {
        app.mode = Mode::Normal;
        return;
    }

    // Toolbar.
    let h = app.hits.clone();
    if hit(h.back, x, y) {
        return app.back();
    }
    if hit(h.forward, x, y) {
        return app.forward();
    }
    if hit(h.view_cycle, x, y) {
        return vim::act(app, Action::CycleView, 1);
    }
    if hit(h.view_menu, x, y) {
        app.mode = Mode::Menu(MenuKind::ViewMode);
        app.menu_sel = 0;
        return;
    }
    if hit(h.split, x, y) {
        return app.toggle_split();
    }
    if hit(h.search, x, y) {
        return vim::act(app, Action::EnterSearch, 1);
    }
    if hit(h.menu, x, y) {
        app.mode = Mode::Menu(MenuKind::Hamburger);
        app.menu_sel = 0;
        return;
    }
    for (r, seg) in &h.crumb_arrows {
        if hit(*r, x, y) {
            app.mode = Mode::CrumbMenu(*seg);
            app.menu_sel = 0;
            return;
        }
    }
    for (r, p) in &h.crumbs {
        if hit(*r, x, y) {
            let p = p.clone();
            return app.goto(Target::Dir(p), true);
        }
    }
    // Clicking the empty part of the location bar edits it, as in Dolphin.
    if hit(h.path_bar, x, y) && app.mode != Mode::PathEdit {
        return vim::act(app, Action::EnterPathEdit, 1);
    }
    for (i, r) in h.tabs.iter().enumerate() {
        if hit(*r, x, y) {
            app.tab = i;
            return;
        }
    }
    // Details column headers sort, and re-sort in reverse on a second click.
    for (r, key) in &h.headers {
        if hit(*r, x, y) {
            let k = *key;
            return app.set_sort(k);
        }
    }
    if app.places_visible && hit(h.places, x, y) {
        let i = (y - h.places.y) as usize;
        if let Some(row) = app.places.get(i) {
            if let Some(t) = row.target().cloned() {
                app.places_sel = i;
                app.focus = Focus::Places;
                app.goto(t, true);
                app.focus = Focus::View;
            }
        }
        return;
    }

    // The file view.
    let Some((pane_idx, vis)) = hit_item(app, x, y) else {
        if let Some(i) = pane_at(app, x, y) {
            app.tabs[app.tab].active = i;
            app.focus = Focus::View;
            if !ctrl && !shift {
                app.pane_at_mut(i).selected.clear();
            }
        }
        return;
    };
    app.tabs[app.tab].active = pane_idx;
    app.focus = Focus::View;

    let path = app.pane().at(vis).map(|e| e.path.clone());
    let Some(path) = path else { return };

    // The tree arrow opens a folder as a drawer in place, on one click. The
    // rest of the row is untouched, so double-clicking the name still enters
    // the folder — same split Dolphin makes.
    if !ctrl && !shift && on_expand_arrow(app, x, vis) {
        app.pane_mut().cursor = vis;
        app.last_click = None;
        app.toggle_expand();
        return;
    }

    if ctrl {
        let p = app.pane_mut();
        if !p.selected.remove(&path) {
            p.selected.insert(path.clone());
        }
        p.cursor = vis;
        p.anchor = vis;
    } else if shift {
        let anchor = app.pane().anchor;
        let (a, b) = (anchor.min(vis), anchor.max(vis));
        let range: Vec<PathBuf> = (a..=b)
            .filter_map(|i| app.pane().at(i).map(|e| e.path.clone()))
            .collect();
        let p = app.pane_mut();
        p.selected.clear();
        p.selected.extend(range);
        p.cursor = vis;
    } else {
        let was_selected = app.pane().selected.contains(&path);
        let double = app
            .last_click
            .is_some_and(|(t, v)| v == vis && t.elapsed() < Duration::from_millis(DOUBLE_MS));
        if double {
            app.pane_mut().cursor = vis;
            app.last_click = None;
            app.activate();
            return;
        }
        let p = app.pane_mut();
        p.cursor = vis;
        p.anchor = vis;
        if !was_selected {
            p.selected.clear();
        }
        // Arm a drag: it only becomes one once the pointer actually moves.
        let paths = if p.selected.is_empty() {
            vec![path.clone()]
        } else {
            p.selected_paths()
        };
        app.drag = Some(Drag {
            paths,
            at: (x, y),
            origin: (x, y),
            started: false,
        });
    }
    app.last_click = Some((Instant::now(), vis));
}

fn middle(app: &mut App, x: u16, y: u16) {
    // Middle-click a folder to open it in a new tab, as Dolphin does.
    if let Some((pane_idx, vis)) = hit_item(app, x, y) {
        app.tabs[app.tab].active = pane_idx;
        if let Some(e) = app.pane().at(vis).cloned() {
            if e.is_dir() {
                app.new_tab(e.path);
            }
        }
        return;
    }
    if app.places_visible && hit(app.hits.places, x, y) {
        let i = (y - app.hits.places.y) as usize;
        if let Some(Target::Dir(d)) = app.places.get(i).and_then(|r| r.target()).cloned() {
            app.new_tab(d);
        }
    }
}

fn dragging(app: &mut App, x: u16, y: u16) {
    let Some(d) = app.drag.as_mut() else { return };
    d.at = (x, y);
    let moved = x.abs_diff(d.origin.0) + y.abs_diff(d.origin.1);
    if moved > config::DRAG_THRESHOLD {
        d.started = true;
    }
}

fn release(app: &mut App, x: u16, y: u16, shift: bool, ctrl: bool) {
    let Some(d) = app.drag.as_ref() else { return };
    if !d.started {
        app.drag = None;
        return;
    }

    // Dropping on a Places entry moves/copies into that place.
    if app.places_visible && hit(app.hits.places, x, y) {
        let i = (y - app.hits.places.y) as usize;
        if let Some(Target::Dir(dir)) = app.places.get(i).and_then(|r| r.target()).cloned() {
            return drag::drop_internal(app, dir, shift, ctrl);
        }
        app.drag = None;
        return;
    }

    // Dropping on a folder puts the items inside it; anywhere else in a pane
    // drops into that pane's directory — which is how split-view transfer works.
    if let Some((pane_idx, vis)) = hit_item(app, x, y) {
        let target = app
            .pane_at(pane_idx)
            .at(vis)
            .filter(|e| e.is_dir())
            .map(|e| e.path.clone())
            .unwrap_or_else(|| app.pane_at(pane_idx).cwd.clone());
        return drag::drop_internal(app, target, shift, ctrl);
    }
    if let Some(i) = pane_at(app, x, y) {
        let dir = app.pane_at(i).cwd.clone();
        return drag::drop_internal(app, dir, shift, ctrl);
    }
    app.drag = None;
}

// ---------------------------------------------------------------------------
// Hit-testing
// ---------------------------------------------------------------------------

fn pane_at(app: &App, x: u16, y: u16) -> Option<usize> {
    app.tab().panes.iter().position(|p| hit(p.area, x, y))
}

fn hit_pane(app: &mut App, x: u16, y: u16) {
    if let Some(i) = pane_at(app, x, y) {
        app.tabs[app.tab].active = i;
        app.focus = Focus::View;
    }
}

/// Which visible index is under the pointer, using the same arithmetic the
/// renderer used — the geometry is cached on the pane, not recomputed.
fn hit_item(app: &App, x: u16, y: u16) -> Option<(usize, usize)> {
    let idx = pane_at(app, x, y)?;
    let p = app.pane_at(idx);
    let (dx, dy) = (x - p.area.x, y - p.area.y);
    let vis = match p.view {
        ViewMode::Icons => {
            // The grid is centred, so its left edge is not the pane's.
            let c = x.saturating_sub(p.grid_x) / p.cell_w.max(1);
            let r = dy / p.cell_h.max(1);
            // The grid rarely fills the pane exactly: past the last row and
            // right of the last column is empty space, not a phantom item.
            if x < p.grid_x || c >= p.grid_cols || r >= p.grid_rows {
                return None;
            }
            p.offset * p.grid_cols as usize + r as usize * p.grid_cols as usize + c as usize
        }
        ViewMode::Compact => {
            // `cols` truncates, so the leftover columns on the right belong to
            // no cell. Without this the index runs one column past the end and
            // the length check below waves it through. Rows need no such guard:
            // a compact row is one line and the grid is the full pane height.
            let c = dx / p.cell_w.max(1);
            if c >= p.grid_cols {
                return None;
            }
            (p.offset + c as usize) * p.grid_rows as usize + dy as usize
        }
        ViewMode::Details => {
            if dy == 0 {
                return None; // the header row
            }
            p.offset + ((dy - 1) / p.cell_h.max(1)) as usize
        }
    };
    (vis < p.len()).then_some((idx, vis))
}

/// Whether `x` lands on the ▸/▾ glyph of a Details row that draws one. The
/// blank cell to its left counts too: one column is a mean target with a mouse,
/// and nothing else claims that cell.
fn on_expand_arrow(app: &App, x: u16, vis: usize) -> bool {
    let p = app.pane();
    if p.view != ViewMode::Details {
        return false;
    }
    let Some(e) = p.at(vis) else { return false };
    // Mirrors the name column `ui::details_view` writes: one cell of padding,
    // then two per depth level, then the arrow.
    let arrow = p.area.x + 1 + e.depth * 2;
    e.is_dir() && (x == arrow || x + 1 == arrow)
}

/// Row index inside whichever popup is open, or None when the click missed.
fn menu_index(app: &App, x: u16, y: u16, len: usize) -> Option<usize> {
    let r = app.hits.menu_popup;
    if !hit(r, x, y) {
        return None;
    }
    let i = (y - r.y) as usize;
    (i < len).then_some(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    #[test]
    fn hit_rejects_zero_sized_rects() {
        assert!(!hit(Rect::new(0, 0, 0, 0), 0, 0));
        assert!(hit(Rect::new(2, 3, 4, 1), 5, 3));
        assert!(!hit(Rect::new(2, 3, 4, 1), 6, 3));
    }

    #[test]
    fn details_header_row_is_not_an_item() {
        let mut app = App::new(std::env::temp_dir());
        app.pane_mut().view = ViewMode::Details;
        app.pane_mut().area = Rect::new(0, 0, 80, 20);
        app.pane_mut().cell_h = 1;
        assert_eq!(hit_item(&app, 5, 0), None);
    }

    /// The arrow's column is computed twice — here and in the renderer — so it
    /// is worth pinning the indent arithmetic down.
    #[test]
    fn expand_arrow_follows_the_tree_indent() {
        use crate::fs::{Entry, Kind};

        let mk = |depth: u16, kind: Kind| Entry {
            name: "d".into(),
            path: "/d".into(),
            kind,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            depth,
            expanded: false,
        };
        let mut app = App::new(std::env::temp_dir());
        let p = app.pane_mut();
        p.view = ViewMode::Details;
        p.area = Rect::new(4, 0, 60, 20);
        p.entries = vec![mk(0, Kind::Dir), mk(2, Kind::Dir), mk(0, Kind::File)];
        p.visible = vec![0, 1, 2];

        // depth 0: pane edge + one cell of padding.
        assert!(on_expand_arrow(&app, 5, 0));
        assert!(on_expand_arrow(&app, 4, 0)); // the forgiving cell to its left
        assert!(!on_expand_arrow(&app, 6, 0));
        // depth 2: two more cells per level.
        assert!(on_expand_arrow(&app, 9, 1));
        assert!(!on_expand_arrow(&app, 5, 1));
        // Files have no arrow to hit.
        assert!(!on_expand_arrow(&app, 5, 2));
    }

    /// `cols` truncates, so a compact grid leaves dead columns on the right.
    /// Without the bound they read as a further column of items.
    #[test]
    fn compact_right_margin_is_not_an_item() {
        let mut app = App::new(std::env::temp_dir());
        let p = app.pane_mut();
        p.view = ViewMode::Compact;
        p.area = Rect::new(0, 0, 25, 4);
        p.cell_w = 10;
        p.cell_h = 1;
        p.grid_cols = 2; // 25 / 10 — columns 20..24 belong to no cell
        p.grid_rows = 4;
        // More items than the two columns hold, so an out-of-range column
        // still lands inside the listing and the length check waves it
        // through — which is exactly how the bug hid.
        p.visible = (0..12).collect();

        assert_eq!(hit_item(&app, 19, 0), Some((0, 4)));
        assert_eq!(hit_item(&app, 22, 0), None);
    }
}
