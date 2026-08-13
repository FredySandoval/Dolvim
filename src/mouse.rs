//! Mouse handling. Everything the toolbar, breadcrumb, headers, places panel
//! and status bar draw is clickable, hit-tested against the rects the last
//! render actually produced.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, CellPos, Drag, Focus, MenuKind, Mode, ViewMode};
use crate::config;
use crate::drag;
use crate::places::Target;
use crate::vim;
use crate::vim::Action;

/// Which Places row the pointer is on. The panel is one row per entry, so the
/// mapping is the offset from its top edge.
fn places_row_at(app: &App, y: u16) -> usize {
    (y - app.hits.places.y) as usize
}

fn rect_contains(rect: Rect, x: u16, y: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && x >= rect.x
        && x < rect.right()
        && y >= rect.y
        && y < rect.bottom()
}

pub fn handle_mouse_event(app: &mut App, m: MouseEvent) {
    // Modal, for the reason given in `vim::handle_key_event` — a click can
    // reach Paste through the menu just as a key can, so blocking only the
    // keyboard would leave the orphaned-transfer hole open.
    if app.active_transfer.is_some() {
        return;
    }
    let (x, y) = (m.column, m.row);
    let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);
    let shift = m.modifiers.contains(KeyModifiers::SHIFT);

    match m.kind {
        MouseEventKind::ScrollDown => handle_scroll(app, x, y, 1),
        MouseEventKind::ScrollUp => handle_scroll(app, x, y, -1),
        MouseEventKind::Down(MouseButton::Left) => handle_left_press(app, x, y, ctrl, shift),
        MouseEventKind::Down(MouseButton::Middle) => handle_middle_press(app, x, y),
        MouseEventKind::Down(MouseButton::Right) => {
            hit_pane(app, x, y);
            vim::open_menu(app, MenuKind::Hamburger);
        }
        MouseEventKind::Drag(MouseButton::Left) => handle_left_drag(app, x, y),
        MouseEventKind::Up(MouseButton::Left) => handle_left_release(app, x, y, shift, ctrl),
        MouseEventKind::Moved => handle_pointer_move(app, x, y),
        _ => {}
    }
}

/// Over one of the two toolbar buttons that drop a menu.
fn on_menu_button(app: &App, x: u16, y: u16) -> bool {
    rect_contains(app.hits.view_menu, x, y) || rect_contains(app.hits.menu, x, y)
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
fn handle_pointer_move(app: &mut App, x: u16, y: u16) {
    // Never take a mode that is in the middle of something.
    if !matches!(app.mode, Mode::Normal | Mode::Buttons(_) | Mode::Menu(_)) {
        return;
    }
    let hitboxes = app.hits.clone();
    // The caret, not the icon beside it: on a split button the icon is the
    // action and only the caret drops the list, which is how clicking works too.
    let menu_to_open = if rect_contains(hitboxes.view_menu, x, y) {
        MenuKind::ViewMode
    } else if rect_contains(hitboxes.menu, x, y) {
        MenuKind::Hamburger
    } else {
        return;
    };
    if app.mode != Mode::Menu(menu_to_open.clone()) {
        vim::open_menu(app, menu_to_open);
    }
}

fn handle_scroll(app: &mut App, x: u16, y: u16, scroll_delta: isize) {
    if app.places_visible && rect_contains(app.hits.places, x, y) {
        let n = app.places.len();
        let next = (app.places_cursor as isize + scroll_delta).clamp(0, n as isize - 1) as usize;
        if app.places[next].is_selectable() {
            app.places_cursor = next;
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
        p.offset = (p.offset as isize + scroll_delta * step).clamp(0, max) as usize;
    }
}

fn handle_left_press(app: &mut App, x: u16, y: u16, ctrl: bool, shift: bool) {
    // An open menu swallows the click: inside picks, outside dismisses.
    if let Mode::Menu(kind) = app.mode.clone() {
        let items = vim::menu_items(&kind);
        if let Some(i) = menu_index(app, x, y, items.len()) {
            let action = items[i].action;
            vim::leave_toolbar(app);
            vim::run_action(app, action, 1);
        } else if !on_menu_button(app, x, y) {
            // Clicking the button the menu hangs from would close it, and the
            // next pointer motion would open it again. Leave it be.
            app.mode = Mode::Normal;
        }
        return;
    }
    if let Mode::CrumbMenu(segment_index) = app.mode.clone() {
        let items = vim::crumb_siblings(app, segment_index);
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
    let hitboxes = app.hits.clone();
    if rect_contains(hitboxes.back, x, y) {
        return app.back();
    }
    if rect_contains(hitboxes.forward, x, y) {
        return app.forward();
    }
    if rect_contains(hitboxes.view_cycle, x, y) {
        return vim::run_action(app, Action::CycleView, 1);
    }
    if rect_contains(hitboxes.view_menu, x, y) {
        return vim::open_menu(app, MenuKind::ViewMode);
    }
    if rect_contains(hitboxes.split, x, y) {
        return app.toggle_split();
    }
    if rect_contains(hitboxes.search, x, y) {
        return vim::run_action(app, Action::EnterSearch, 1);
    }
    if rect_contains(hitboxes.menu, x, y) {
        return vim::open_menu(app, MenuKind::Hamburger);
    }
    for (rect, segment_index) in &hitboxes.crumb_arrows {
        if rect_contains(*rect, x, y) {
            return vim::open_crumb(app, *segment_index);
        }
    }
    for (rect, crumb_path) in &hitboxes.crumbs {
        if rect_contains(*rect, x, y) {
            return app.open_breadcrumb(crumb_path.clone());
        }
    }
    // Clicking the empty part of the location bar edits it, as in Dolphin.
    if rect_contains(hitboxes.path_bar, x, y) && app.mode != Mode::PathEdit {
        return vim::run_action(app, Action::EnterPathEdit, 1);
    }
    for (tab_index, rect) in hitboxes.tabs.iter().enumerate() {
        if rect_contains(*rect, x, y) {
            app.active_tab = tab_index;
            app.focus = Focus::Tabs;
            return;
        }
    }
    // Details column headers sort, and re-sort in reverse on a second click.
    for (rect, sort_key) in &hitboxes.headers {
        if rect_contains(*rect, x, y) {
            return app.set_sort(*sort_key);
        }
    }
    if app.places_visible && rect_contains(hitboxes.places, x, y) {
        let places_row_index = places_row_at(app, y);
        if let Some(row) = app.places.get(places_row_index) {
            if matches!(row, crate::places::Row::Item { eject: true, .. })
                && x >= hitboxes
                    .places
                    .right()
                    .saturating_sub(config::glyph::EJECT.width().max(1) as u16 + 1)
            {
                app.eject_place_index(places_row_index);
            } else if row.is_selectable() {
                app.places_cursor = places_row_index;
                app.focus = Focus::Places;
                app.open_place_index(places_row_index);
                app.focus = Focus::View;
            }
        }
        return;
    }

    // The file view.
    let Some(ItemHit {
        pane_index: pane_idx,
        visible_index: vis,
    }) = hit_item(app, x, y)
    else {
        if let Some(i) = pane_at(app, x, y) {
            app.tabs[app.active_tab].active = i;
            app.focus = Focus::View;
            if !ctrl && !shift {
                app.pane_at_mut(i).selected.clear();
            }
        }
        return;
    };
    app.tabs[app.active_tab].active = pane_idx;
    app.focus = Focus::View;

    let entry_identity = app
        .pane()
        .entry_at(vis)
        .map(|entry| (entry.path.clone(), entry.selection_key()));
    let Some((path, selection_key)) = entry_identity else {
        return;
    };

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
        if !p.selected.remove(&selection_key) {
            p.selected.insert(selection_key.clone());
        }
        p.cursor = vis;
        p.anchor = vis;
    } else if shift {
        let anchor = app.pane().anchor;
        let (a, b) = (anchor.min(vis), anchor.max(vis));
        let range: Vec<PathBuf> = (a..=b)
            .filter_map(|i| app.pane().entry_at(i).map(|e| e.selection_key()))
            .collect();
        let p = app.pane_mut();
        p.selected.clear();
        p.selected.extend(range);
        p.cursor = vis;
    } else {
        let was_selected = app.pane().selected.contains(&selection_key);
        let is_double_click = app.last_click.is_some_and(|last_click| {
            last_click.visible_index == vis
                && last_click.at.elapsed() < Duration::from_millis(config::DOUBLE_CLICK_MS)
        });
        if is_double_click {
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
            source_pane_id: app.pane().id,
            position: CellPos { x, y },
            origin: CellPos { x, y },
            started: false,
        });
    }
    app.last_click = Some(crate::app::LastClick {
        at: Instant::now(),
        visible_index: vis,
    });
}

fn handle_middle_press(app: &mut App, x: u16, y: u16) {
    // Middle-click a folder to open it in a new tab, as Dolphin does.
    if let Some(ItemHit {
        pane_index: pane_idx,
        visible_index: vis,
    }) = hit_item(app, x, y)
    {
        app.tabs[app.active_tab].active = pane_idx;
        if let Some(e) = app.pane().entry_at(vis).cloned() {
            if e.is_dir() {
                app.new_tab(e.path);
            }
        }
        return;
    }
    if app.places_visible && rect_contains(app.hits.places, x, y) {
        let places_row_index = places_row_at(app, y);
        if let Some(Target::Dir(d)) = app
            .places
            .get(places_row_index)
            .and_then(|r| r.target())
            .cloned()
        {
            app.new_tab(d);
        }
    }
}

fn handle_left_drag(app: &mut App, x: u16, y: u16) {
    let Some(active_drag) = app.drag.as_mut() else {
        return;
    };
    active_drag.position = CellPos { x, y };
    let moved_cells = x.abs_diff(active_drag.origin.x) + y.abs_diff(active_drag.origin.y);
    if moved_cells > config::DRAG_THRESHOLD {
        active_drag.started = true;
    }
}

fn handle_left_release(app: &mut App, x: u16, y: u16, shift: bool, ctrl: bool) {
    let Some(active_drag) = app.drag.as_ref() else {
        return;
    };
    if !active_drag.started {
        app.drag = None;
        return;
    }

    // Dropping on a Places entry moves/copies into that place.
    if app.places_visible && rect_contains(app.hits.places, x, y) {
        let places_row_index = places_row_at(app, y);
        if let Some(Target::Dir(dir)) = app
            .places
            .get(places_row_index)
            .and_then(|r| r.target())
            .cloned()
        {
            let reveal = app.reveal_intent_for_pane(app.tab().active, dir.clone());
            return drag::drop_internal(app, dir, shift, ctrl, reveal);
        }
        app.drag = None;
        return;
    }

    // Dropping on a folder puts the items inside it; anywhere else in a pane
    // drops into that pane's directory — which is how split-view transfer works.
    if let Some(ItemHit {
        pane_index: pane_idx,
        visible_index: vis,
    }) = hit_item(app, x, y)
    {
        let target = app
            .pane_at(pane_idx)
            .entry_at(vis)
            .filter(|e| e.is_dir())
            .map(|e| e.path.clone())
            .unwrap_or_else(|| app.pane_at(pane_idx).cwd.clone());
        let reveal = app.reveal_intent_for_pane(pane_idx, target.clone());
        return drag::drop_internal(app, target, shift, ctrl, reveal);
    }
    if let Some(i) = pane_at(app, x, y) {
        let dir = app.pane_at(i).cwd.clone();
        let reveal = app.reveal_intent_for_pane(i, dir.clone());
        return drag::drop_internal(app, dir, shift, ctrl, reveal);
    }
    app.drag = None;
}

// ---------------------------------------------------------------------------
// Hit-testing
// ---------------------------------------------------------------------------

fn pane_at(app: &App, x: u16, y: u16) -> Option<usize> {
    app.tab()
        .panes
        .iter()
        .position(|p| rect_contains(p.area, x, y))
}

fn hit_pane(app: &mut App, x: u16, y: u16) {
    if let Some(i) = pane_at(app, x, y) {
        app.tabs[app.active_tab].active = i;
        app.focus = Focus::View;
    }
}

/// Which visible index is under the pointer, using the same arithmetic the
/// renderer used — the geometry is cached on the pane, not recomputed.
#[derive(Debug, PartialEq, Eq)]
struct ItemHit {
    pane_index: usize,
    visible_index: usize,
}

fn hit_item(app: &App, x: u16, y: u16) -> Option<ItemHit> {
    let idx = pane_at(app, x, y)?;
    let pane = app.pane_at(idx);
    let row_offset_in_pane = y - pane.area.y;
    let visible_index = match pane.view {
        ViewMode::Icons => {
            // The grid is centred, so its left edge is not the pane's.
            let column = x.saturating_sub(pane.grid_x) / pane.cell_width.max(1);
            let row = row_offset_in_pane / pane.cell_height.max(1);
            // The grid rarely fills the pane exactly: past the last row and
            // right of the last column is empty space, not a phantom item.
            if x < pane.grid_x || column >= pane.grid_cols || row >= pane.grid_rows {
                return None;
            }
            (pane.offset + row as usize) * pane.grid_cols as usize + column as usize
        }
        ViewMode::Compact => {
            // Columns are individually sized, so the hit is found by walking
            // them. Running off either end means margin, which belongs to no
            // cell — without that the index would run past the last column and
            // the length check below would wave it through. Rows need no such
            // guard: a compact row is one line and the grid is the full pane
            // height.
            if x < pane.grid_x {
                return None;
            }
            let x_offset_in_grid = x - pane.grid_x;
            let mut edge_x = 0;
            let column = pane.column_widths.iter().position(|column_width| {
                edge_x += column_width;
                x_offset_in_grid < edge_x
            })?;
            (pane.offset + column) * pane.grid_rows as usize + row_offset_in_pane as usize
        }
        ViewMode::Details => {
            if row_offset_in_pane == 0 {
                return None; // the header row
            }
            pane.offset + ((row_offset_in_pane - 1) / pane.cell_height.max(1)) as usize
        }
    };
    (visible_index < pane.len()).then_some(ItemHit {
        pane_index: idx,
        visible_index,
    })
}

/// Whether `x` lands on the ▸/▾ glyph of a Details row that draws one. The
/// blank cell to its left counts too: one column is a mean target with a mouse,
/// and nothing else claims that cell.
fn on_expand_arrow(app: &App, x: u16, vis: usize) -> bool {
    let p = app.pane();
    if p.view != ViewMode::Details {
        return false;
    }
    let Some(e) = p.entry_at(vis) else {
        return false;
    };
    // Mirrors the name column `ui::details_view` writes: one cell of padding,
    // then two per depth level, then the arrow.
    let arrow = p.area.x + 1 + e.depth * 2;
    e.is_dir() && (x == arrow || x + 1 == arrow)
}

/// Row index inside whichever popup is open, or None when the click missed.
fn menu_index(app: &App, x: u16, y: u16, len: usize) -> Option<usize> {
    let r = app.hits.menu_popup;
    if !rect_contains(r, x, y) {
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
        assert!(!rect_contains(Rect::new(0, 0, 0, 0), 0, 0));
        assert!(rect_contains(Rect::new(2, 3, 4, 1), 5, 3));
        assert!(!rect_contains(Rect::new(2, 3, 4, 1), 6, 3));
    }

    #[test]
    fn details_header_row_is_not_an_item() {
        let mut app = App::new(std::env::temp_dir());
        app.pane_mut().view = ViewMode::Details;
        app.pane_mut().area = Rect::new(0, 0, 80, 20);
        app.pane_mut().cell_height = 1;
        assert_eq!(hit_item(&app, 5, 0), None);
    }

    /// The arrow's column is computed twice — here and in the renderer — so it
    /// is worth pinning the indent arithmetic down.
    #[test]
    fn expand_arrow_follows_the_tree_indent() {
        use crate::fs::{Entry, Kind};

        let make_entry = |depth: u16, kind: Kind| Entry {
            name: "d".into(),
            path: "/d".into(),
            backing_path: None,
            link_target: None,
            kind,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            trash_id: None,
            depth,
            expanded: false,
        };
        let mut app = App::new(std::env::temp_dir());
        let pane = app.pane_mut();
        pane.view = ViewMode::Details;
        pane.area = Rect::new(4, 0, 60, 20);
        pane.entries = vec![
            make_entry(0, Kind::Dir),
            make_entry(2, Kind::Dir),
            make_entry(0, Kind::File),
        ];
        pane.visible = vec![0, 1, 2];

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

    #[test]
    fn split_drag_keeps_source_cleanup_and_folder_reveal_owners_distinct() {
        use crate::fs::{Entry, Kind};
        use std::sync::atomic::Ordering;

        let unique = format!(
            "dolvim-split-drag-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let source_dir = root.join("source");
        let target_dir = root.join("target");
        let hovered_folder = target_dir.join("hovered");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&hovered_folder).unwrap();
        let source = source_dir.join("item.txt");
        std::fs::write(&source, b"payload").unwrap();

        let mut app = App::new(source_dir);
        app.toggle_split();
        app.tab_mut().active = 0;
        let source_pane_id = app.pane_at(0).id;
        let target_pane_id = app.pane_at(1).id;
        {
            let pane = app.pane_at_mut(0);
            pane.area = Rect::new(0, 0, 20, 10);
        }
        {
            let pane = app.pane_at_mut(1);
            pane.cwd = target_dir;
            pane.target = Target::Dir(pane.cwd.clone());
            pane.view = ViewMode::Details;
            pane.area = Rect::new(20, 0, 20, 10);
            pane.cell_height = 1;
            pane.entries = vec![Entry {
                name: "hovered".into(),
                path: hovered_folder.clone(),
                backing_path: None,
                link_target: None,
                kind: Kind::Dir,
                size: 0,
                mtime: 0,
                mode: 0,
                readable: true,
                hidden: false,
                trash_id: None,
                depth: 0,
                expanded: false,
            }];
            pane.visible = vec![0];
        }
        app.drag = Some(Drag {
            paths: vec![source.clone()],
            source_pane_id,
            position: CellPos { x: 21, y: 1 },
            origin: CellPos { x: 1, y: 1 },
            started: true,
        });

        // Details row zero is at y=1 (below its header). Ctrl makes this a copy
        // so the fixture also verifies the worker's concrete destination.
        handle_left_release(&mut app, 21, 1, false, true);
        let transfer = app.active_transfer.as_ref().unwrap();
        assert_eq!(transfer.selection_pane_id, source_pane_id);
        let reveal = transfer.reveal.as_ref().unwrap();
        assert_eq!(reveal.pane_id, target_pane_id);
        assert_eq!(reveal.directory, hovered_folder);

        while !transfer.progress.finished.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let outcome = transfer.progress.outcome.lock().unwrap();
        let effect = outcome.as_ref().unwrap().committed.first().unwrap();
        assert_eq!(effect.target, hovered_folder.join("item.txt"));
        drop(outcome);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// `cols` truncates, so a compact grid leaves dead columns on the right.
    /// Without the bound they read as a further column of items.
    #[test]
    fn compact_right_margin_is_not_an_item() {
        let mut app = App::new(std::env::temp_dir());
        let pane = app.pane_mut();
        pane.view = ViewMode::Compact;
        pane.area = Rect::new(0, 0, 25, 4);
        pane.cell_width = 10;
        pane.cell_height = 1;
        pane.grid_x = 1; // VIEW_MARGIN: columns start one in from the pane edge
        pane.column_widths = vec![10, 10]; // 21..24 is leftover margin, no cell
        pane.grid_cols = 2;
        pane.grid_rows = 4;
        // More items than the two columns hold, so an out-of-range column
        // still lands inside the listing and the length check waves it
        // through — which is exactly how the bug hid.
        pane.visible = (0..12).collect();

        assert_eq!(
            hit_item(&app, 19, 0),
            Some(ItemHit {
                pane_index: 0,
                visible_index: 4
            })
        );
        assert_eq!(hit_item(&app, 0, 0), None); // the left margin
        assert_eq!(hit_item(&app, 22, 0), None);
    }

    /// Compact columns are sized to their own contents, so the column under a
    /// pointer is found by walking the widths. Dividing by any single one of
    /// them lands in the wrong column the moment they differ.
    #[test]
    fn compact_columns_may_differ_in_width() {
        let mut app = App::new(std::env::temp_dir());
        let pane = app.pane_mut();
        pane.view = ViewMode::Compact;
        pane.area = Rect::new(0, 0, 25, 4);
        pane.cell_height = 1;
        pane.grid_x = 1; // VIEW_MARGIN
        pane.column_widths = vec![12, 8];
        pane.grid_cols = 2;
        pane.grid_rows = 4;
        pane.visible = (0..12).collect();

        assert_eq!(
            hit_item(&app, 12, 0),
            Some(ItemHit {
                pane_index: 0,
                visible_index: 0
            })
        ); // last cell of col 0
        assert_eq!(
            hit_item(&app, 13, 0),
            Some(ItemHit {
                pane_index: 0,
                visible_index: 4
            })
        ); // first of col 1
        assert_eq!(
            hit_item(&app, 20, 2),
            Some(ItemHit {
                pane_index: 0,
                visible_index: 6
            })
        );
        assert_eq!(hit_item(&app, 21, 0), None); // past both columns
    }
}
