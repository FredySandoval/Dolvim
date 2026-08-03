//! The modal input engine, and the single place where an `Action` turns into
//! a state change.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, Confirm, Focus, MenuKind, Mode, ViewMode};
use crate::config;
use crate::fs::SortKey;
use crate::ops;
use crate::places::{self, Target};

pub fn handle_key_event(app: &mut App, key_event: KeyEvent) {
    // The transfer popup is modal, as Dolphin's is. Letting keys through means
    // you can navigate away from a live copy, or start a second one on top of
    // `app.progress` and orphan the first thread with no way to see or stop it.
    if let Some(active_transfer) = &app.progress {
        if key_event.code == KeyCode::Esc {
            active_transfer
                .cancel_requested
                .store(true, Ordering::Relaxed);
        }
        return;
    }

    match app.mode.clone() {
        Mode::Normal | Mode::Visual | Mode::VisualLine => normal(app, key_event),
        Mode::Confirm(c) => confirm(app, key_event, c),
        // Any key dismisses an information overlay.
        Mode::Properties | Mode::Help => app.mode = Mode::Normal,
        Mode::Menu(kind) => menu(app, key_event, kind),
        Mode::CrumbMenu(i) => crumb_menu(app, key_event, i),
        Mode::Buttons(i) => buttons(app, key_event, i),
        _ => text(app, key_event),
    }
}

// ---------------------------------------------------------------------------
// Normal / Visual
// ---------------------------------------------------------------------------

fn normal(app: &mut App, k: KeyEvent) {
    if k.code == KeyCode::Esc {
        app.count.clear();
        app.pending = None;
        app.pending_delete = None;
        // Esc ends the visual and takes the range with it: the selection
        // belonged to the drag, not to the pane. Outside a visual it is the
        // same key that clears a selection built with Space.
        app.mode = Mode::Normal;
        app.pane_mut().selected.clear();
        return;
    }

    // A pending chord leader owns the next key, whatever it is.
    if let Some(lead) = app.pending.take() {
        if let KeyCode::Char(c) = k.code {
            if let Some((_, _, a)) = config::CHORDS
                .iter()
                .find(|(l, f, _)| *l == lead && *f == c)
            {
                let n = take_count(app);
                act(app, *a, n);
                return;
            }
        }
        return;
    }

    // Count prefix. A bare `0` is a motion, not a count.
    if let KeyCode::Char(c @ '0'..='9') = k.code {
        if k.modifiers.is_empty() && !(c == '0' && app.count.is_empty()) {
            app.count.push(c);
            return;
        }
    }

    // A `d` owns the next motion. After the count block, so `d5j` can collect
    // its 5 the same way `5j` does.
    if let Some(pre) = app.pending_delete {
        return delete_motion(app, k, pre);
    }

    if app.focus == Focus::Places && places_key(app, k) {
        return;
    }

    if let KeyCode::Char(c) = k.code {
        if k.modifiers.is_empty() && config::CHORD_LEADERS.contains(&c) {
            // `c` is only a leader when a follower can complete it; `cw` is the
            // one chord it starts, so a lone `c` simply waits.
            app.pending = Some(c);
            return;
        }
    }

    if let Some(a) = lookup(k) {
        let n = take_count(app);
        act(app, a, n);
        return;
    }

    // Anything printable and unbound is Dolphin's type-ahead.
    if let KeyCode::Char(c) = k.code {
        if k.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
            app.typeahead(c);
        }
    }
}

/// Resolve `d{motion}` into a linewise range and trash it.
///
/// `pre` is the count typed before the `d`; vim multiplies it by the one typed
/// after, so `2d3j` is six lines. Anything that is not a motion cancels, which
/// is what vim does with a key the operator cannot use.
fn delete_motion(app: &mut App, k: KeyEvent, pre: usize) {
    app.pending_delete = None;
    // Ctrl+d is half-page down, not `dd`. Only a bare motion counts.
    if !k.modifiers.difference(KeyModifiers::SHIFT).is_empty() {
        return;
    }
    let n = pre * take_count(app);
    let c = app.pane().cursor;
    let last = app.pane().len().saturating_sub(1);
    let (a, b) = match k.code {
        // `dd` is this line and the n-1 below it; `dj` is this line *and* the
        // n below, one more, exactly as in vim.
        KeyCode::Char('d') => (c, c + n - 1),
        KeyCode::Char('j') | KeyCode::Down => (c, c + n),
        KeyCode::Char('k') | KeyCode::Up => (c.saturating_sub(n), c),
        KeyCode::Char('G') => (c, last),
        _ => return,
    };
    let v = app.pane().paths_in(a, b);
    trash(app, v);
}

/// What an operation acts on: the selection when there is one, otherwise `n`
/// rows from the cursor down. Every destructive action follows this rule.
fn targets(app: &App, n: usize) -> Vec<PathBuf> {
    if app.pane().selected.is_empty() {
        let c = app.pane().cursor;
        app.pane().paths_in(c, c + n - 1)
    } else {
        app.pane().selected_paths()
    }
}

/// The one path to the Trash. Every delete key ends here so that the undo
/// entry, the message and the refresh cannot drift apart.
fn trash(app: &mut App, v: Vec<PathBuf>) {
    if v.is_empty() {
        return;
    }
    // In the Trash there is no further "away" to move something to, so `x`
    // means purge, as it does in Dolphin. It goes behind the Shift+Del
    // confirmation: this is the one place the key is not undoable.
    if app.pane().target == Target::Trash {
        app.mode = Mode::Confirm(Confirm::PurgeFromTrash(v));
        return;
    }
    match ops::trash(&v) {
        Ok(op) => {
            app.undo.push(op);
            app.info(format!("Moved {} item(s) to Trash", v.len()));
            app.pane_mut().selected.clear();
            app.refresh_in_place();
        }
        Err(e) => app.error(e),
    }
    // Deleting the visual range ends the visual, as `d` does in vim.
    if app.mode.is_visual() {
        app.mode = Mode::Normal;
    }
}

/// `v` and `V` start a range at the cursor. Pressing the key you are already in
/// leaves visual, as in vim; pressing the other switches between charwise and
/// linewise without disturbing the anchor.
fn enter_visual(app: &mut App, want: Mode) {
    if app.mode == want {
        app.mode = Mode::Normal;
        return;
    }
    if !app.mode.is_visual() {
        app.pane_mut().anchor = app.pane().cursor;
        app.pane_mut().selected.clear();
    }
    app.mode = want;
    // Redraw the range under the new rule: `V` has to reach the row edges even
    // though the cursor has not moved yet.
    app.move_cursor(0, true);
}

fn lookup(k: KeyEvent) -> Option<Action> {
    config::VIM_KEYS
        .iter()
        .chain(config::DOLPHIN_KEYS.iter())
        .find(|b| b.code == k.code && b.mods == k.modifiers)
        .map(|b| b.action)
}

fn take_count(app: &mut App) -> usize {
    let n = app.count.parse().unwrap_or(1);
    app.count.clear();
    n.max(1)
}

/// Keys the Places panel consumes while it has focus. Returns true when handled.
fn places_key(app: &mut App, k: KeyEvent) -> bool {
    // A control chord is never Places navigation: `Ctrl+h` must move focus, not
    // be read as the bare `h` that leaves the panel.
    if k.modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    let step = |app: &mut App, d: isize| {
        let n = app.places.len() as isize;
        let mut i = app.places_sel as isize;
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if app.places[i as usize].is_selectable() {
                break;
            }
        }
        app.places_sel = i as usize;
    };
    match k.code {
        KeyCode::Char('j') | KeyCode::Down => step(app, 1),
        KeyCode::Char('k') | KeyCode::Up => step(app, -1),
        // `l` points the view at the place and stays, so the panel can be walked
        // while the view follows. `Enter` means "this one, take me there" and so
        // hands focus over as well. A bare motion never leaves its pane; an
        // accept key may.
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if let Some(t) = app.places[app.places_sel].target().cloned() {
                app.goto(t, true);
                if k.code == KeyCode::Enter {
                    app.focus = Focus::View;
                }
            }
        }
        // The panel is one column deep, so there is nothing to the left of a
        // place. Swallowed rather than ignored, or it would reach the view's
        // cursor, which the user cannot see moving.
        KeyCode::Char('h') | KeyCode::Left => {}
        KeyCode::Tab => app.focus = Focus::View,
        KeyCode::F(9) => {
            app.places_visible = false;
            app.focus = Focus::View;
        }
        _ => return false,
    }
    true
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/// Everything the user can ask for: the program's vocabulary. The keymap
/// tables in `config.rs` name these; `act` below is where each one means
/// something. Adding a variant is writing a feature, not configuring one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /* navigation */
    Back,
    Forward,
    GoUp,
    GoHome,
    Open,
    OpenInNewTab,
    Refresh,
    /* cursor */
    MoveDown,
    MoveUp,
    MoveLeft,
    MoveRight,
    Top,
    Bottom,
    HalfPageDown,
    HalfPageUp,
    PageDown,
    PageUp,
    RowStart,
    RowEnd,
    /* selection */
    ToggleSelect,
    SelectAll,
    InvertSelect,
    EnterVisual,
    EnterVisualLine,
    /* file operations */
    Copy,
    Cut,
    /// `d`: trash a range, once a motion says which.
    DeleteOp,
    Paste,
    Trash,
    DeletePerm,
    Rename,
    NewFolder,
    NewFile,
    Undo,
    Properties,
    Compress,
    Extract,
    EmptyTrash,
    Restore,
    /* view */
    ViewIcons,
    ViewCompact,
    ViewDetails,
    CycleView,
    ToggleHidden,
    ToggleSplit,
    SwapPane,
    FocusLeft,
    FocusRight,
    /// `Ctrl+k`: up into the breadcrumb, with its menu open.
    EnterCrumbs,
    TogglePlaces,
    ToggleInfo,
    ToggleFilterBar,
    ToggleExpand,
    /* sorting */
    SortName,
    SortSize,
    SortDate,
    SortType,
    ToggleSortOrder,
    ToggleDirsFirst,
    /* tabs */
    NewTab,
    CloseTab,
    NextTab,
    PrevTab,
    /* modes */
    EnterCommand,
    EnterSearch,
    SearchNext,
    SearchPrev,
    EnterPathEdit,
    /* misc */
    TerminalPanel,
    TerminalHere,
    DragOut,
    DropIn,
    OpenMenu,
    OpenViewMenu,
    OpenSortMenu,
    Help,
    QuitAll,
}

/// One row of a keymap table.
pub struct Bind {
    pub code: KeyCode,
    pub mods: KeyModifiers,
    pub action: Action,
}

/// Terse constructor so a table row reads as one line.
pub const fn b(code: KeyCode, mods: KeyModifiers, action: Action) -> Bind {
    Bind { code, mods, action }
}

pub fn act(app: &mut App, a: Action, n: usize) {
    let extend = app.mode.is_visual();
    let stride = app.pane().stride() as isize;
    let page = app.pane().page() as isize;
    match a {
        // navigation
        Action::Back => app.back(),
        Action::Forward => app.forward(),
        Action::GoUp => app.go_up(),
        Action::GoHome => app.goto(Target::Dir(places::home()), true),
        Action::Open => app.activate(),
        Action::OpenInNewTab => {
            if let Some(e) = app.pane().current().cloned() {
                if e.is_dir() {
                    app.new_tab(e.path);
                }
            }
        }
        Action::Refresh => app.reload(),

        // cursor
        //
        // A line is `stride` consecutive items. Which key walks it and which
        // crosses it depends on how the view flows: Icons run left to right, so
        // a line is a row and `h`/`l` walk it; Compact runs down its columns, so
        // a line is a column and `j`/`k` walk it. Details has stride 1 and no
        // horizontal axis at all, so `j`/`k` simply cross.
        Action::MoveDown | Action::MoveUp => {
            let d = if a == Action::MoveUp {
                -(n as isize)
            } else {
                n as isize
            };
            if app.pane().view == ViewMode::Compact {
                app.step_along(d, extend);
            } else {
                app.step_across(d, extend);
            }
        }
        Action::MoveLeft | Action::MoveRight => {
            let left = a == Action::MoveLeft;
            let d = if left { -(n as isize) } else { n as isize };
            match app.pane().view {
                // `l` opens in Details, `h` goes up. There is nothing sideways.
                ViewMode::Details if left => app.go_up(),
                ViewMode::Details => app.activate(),
                ViewMode::Compact => app.step_across(d, extend),
                ViewMode::Icons => app.step_along(d, extend),
            }
        }
        // `5gg` goes to item 5, like vim's line numbers. `n` is 1 when no count
        // was typed, so a bare `gg` lands on the first item.
        Action::Top => app.goto_index(n - 1, extend),
        Action::Bottom => {
            let last = app.pane().len().saturating_sub(1);
            app.goto_index(last, extend);
        }
        Action::HalfPageDown => app.move_cursor(page / 2 * n as isize, extend),
        Action::HalfPageUp => app.move_cursor(-page / 2 * n as isize, extend),
        Action::PageDown => app.move_cursor(page * n as isize, extend),
        Action::PageUp => app.move_cursor(-page * n as isize, extend),
        Action::RowStart => {
            let c = app.pane().cursor as isize;
            let back = c % stride.max(1);
            app.move_cursor(-back, extend);
        }
        Action::RowEnd => {
            let c = app.pane().cursor as isize;
            let s = stride.max(1);
            app.move_cursor(s - 1 - (c % s), extend);
        }

        // selection
        Action::ToggleSelect => {
            app.toggle_select();
            app.move_cursor(stride, false);
        }
        Action::SelectAll => app.select_all(),
        Action::InvertSelect => app.invert_selection(),
        Action::EnterVisual => enter_visual(app, Mode::Visual),
        Action::EnterVisualLine => enter_visual(app, Mode::VisualLine),

        // file operations
        Action::Copy | Action::Cut => {
            let cut = a == Action::Cut;
            let v = app.pane().selected_paths();
            let n = v.len();
            app.clipboard.set(v, cut);
            app.info(format!(
                "{} {n} item(s)",
                if cut { "Cut" } else { "Copied" }
            ));
            app.mode = Mode::Normal;
        }
        Action::Paste => paste(app),
        Action::Trash => {
            // `x` on the cursor, `5x` on five rows, the selection if there is
            // one. A selection is an explicit range, so it beats the count
            // rather than being sliced by it.
            let v = targets(app, n);
            trash(app, v);
        }
        Action::DeleteOp => {
            // A selection already is the range, so there is nothing to wait for.
            if app.pane().selected.is_empty() {
                app.pending_delete = Some(n);
            } else {
                let v = app.pane().selected_paths();
                trash(app, v);
            }
        }
        Action::DeletePerm => {
            let v = targets(app, n);
            if !v.is_empty() {
                app.mode = Mode::Confirm(if app.pane().target == Target::Trash {
                    Confirm::PurgeFromTrash(v)
                } else {
                    Confirm::DeletePermanently(v)
                });
            }
        }
        Action::Rename => start_rename(app),
        Action::NewFolder => enter_text(app, Mode::NewFolder, String::new()),
        Action::NewFile => enter_text(app, Mode::NewFile, String::new()),
        Action::Undo => match app.undo.pop() {
            None => app.info("Nothing to undo"),
            Some(op) => match ops::undo(&op) {
                Ok(msg) => {
                    app.info(msg);
                    app.refresh_in_place();
                }
                Err(e) => {
                    app.error(format!("Undo failed: {e}"));
                    app.undo.push(op);
                }
            },
        },
        Action::Properties => {
            if app.pane().current().is_some() {
                app.mode = Mode::Properties;
            }
        }
        Action::Compress => {
            let v = app.pane().selected_paths();
            if v.is_empty() {
                return;
            }
            let dest = app.pane().cwd.join(format!("{}.tar.gz", ops::name(&v[0])));
            match ops::compress(&v, &dest) {
                Ok(m) => {
                    app.info(m);
                    app.refresh_in_place();
                }
                Err(e) => app.error(e),
            }
        }
        Action::Extract => {
            let Some(e) = app.pane().current().cloned() else {
                return;
            };
            if !e.is_archive() {
                app.error(format!("{} is not an archive", e.name));
                return;
            }
            let into = app.pane().cwd.clone();
            match ops::extract(&e.path, &into) {
                Ok(m) => {
                    app.info(m);
                    app.refresh_in_place();
                }
                Err(err) => app.error(err),
            }
        }
        Action::EmptyTrash => app.mode = Mode::Confirm(Confirm::EmptyTrash),
        Action::Restore => {
            let v = targets(app, n);
            match ops::restore_from_trash(&v) {
                Ok(n) => {
                    app.info(format!("Restored {n} item(s)"));
                    app.reload();
                }
                Err(e) => app.error(e),
            }
        }

        // view
        Action::ViewIcons => app.set_view(ViewMode::Icons),
        Action::ViewCompact => app.set_view(ViewMode::Compact),
        Action::ViewDetails => app.set_view(ViewMode::Details),
        Action::CycleView => {
            let v = match app.pane().view {
                ViewMode::Icons => ViewMode::Compact,
                ViewMode::Compact => ViewMode::Details,
                ViewMode::Details => ViewMode::Icons,
            };
            app.set_view(v);
        }
        Action::ToggleHidden => app.toggle_hidden(),
        Action::ToggleSplit => app.toggle_split(),
        Action::FocusLeft => app.focus_left(),
        Action::FocusRight => app.focus_right(),
        // Straight up, into whatever is overhead. The nav group sits over the
        // Places panel and the trail starts where the file view does, so which
        // one that is depends on the pane you left.
        Action::EnterCrumbs => {
            if app.focus == Focus::Places {
                focus_button(app, 0);
            } else {
                enter_crumbs(app, config::NAV_BTNS - 1);
            }
        }
        Action::SwapPane => app.other_pane(),
        Action::TogglePlaces => {
            app.places_visible = !app.places_visible;
            if !app.places_visible {
                app.focus = Focus::View;
            }
        }
        Action::ToggleInfo => app.info_visible = !app.info_visible,
        Action::ToggleFilterBar => {
            app.filter_bar = !app.filter_bar;
            if app.filter_bar {
                app.mode = Mode::Filter;
                app.input = app.pane().filter.clone();
                app.input_cursor = app.input.chars().count();
            } else {
                app.pane_mut().filter.clear();
                app.pane_mut().refilter();
            }
        }
        Action::ToggleExpand => app.toggle_expand(),

        // sorting
        Action::SortName => app.set_sort(SortKey::Name),
        Action::SortSize => app.set_sort(SortKey::Size),
        Action::SortDate => app.set_sort(SortKey::Date),
        Action::SortType => app.set_sort(SortKey::Type),
        Action::ToggleSortOrder => {
            let k = app.pane().sort.key;
            app.set_sort(k);
        }
        Action::ToggleDirsFirst => {
            let p = app.pane_mut();
            p.sort.dirs_first = !p.sort.dirs_first;
            p.refilter();
        }

        // tabs
        Action::NewTab => {
            let d = app.pane().cwd.clone();
            app.new_tab(d);
        }
        Action::CloseTab => app.close_tab(),
        Action::NextTab => app.cycle_tab(n as isize),
        Action::PrevTab => app.cycle_tab(-(n as isize)),

        // modes
        Action::EnterCommand => enter_text(app, Mode::Command, String::new()),
        Action::EnterSearch => enter_text(app, Mode::Search, String::new()),
        Action::SearchNext => search_step(app, 1),
        Action::SearchPrev => search_step(app, -1),
        Action::EnterPathEdit => {
            let p = app.pane().cwd.to_string_lossy().into_owned();
            enter_text(app, Mode::PathEdit, p);
        }

        // misc
        Action::TerminalPanel | Action::TerminalHere => {
            app.suspend = Some(crate::app::Suspend::Shell(app.pane().cwd.clone()));
        }
        Action::DragOut => crate::drag::drag_out(app),
        Action::DropIn => crate::drag::drop_in(app),
        Action::OpenMenu => open_menu(app, MenuKind::Hamburger),
        Action::OpenViewMenu => open_menu(app, MenuKind::ViewMode),
        Action::OpenSortMenu => open_menu(app, MenuKind::Sort),
        Action::Help => app.mode = Mode::Help,
        Action::QuitAll => app.quit = true,
    }
}

fn enter_text(app: &mut App, mode: Mode, seed: String) {
    app.mode = mode;
    app.input = seed;
    app.input_cursor = app.input.chars().count();
}

fn start_rename(app: &mut App) {
    let sel = app.pane().selected_paths();
    match sel.len() {
        0 => {}
        1 => {
            let p = sel[0].clone();
            enter_text(app, Mode::Rename(p.clone()), ops::name(&p));
        }
        _ => enter_text(app, Mode::BatchRename, "Item #".into()),
    }
}

fn paste(app: &mut App) {
    // Prefer our own clipboard: it knows cut-vs-copy, which uri-list cannot say.
    let (mut paths, cut) = (app.clipboard.paths.clone(), app.clipboard.cut);
    if paths.is_empty() {
        paths = ops::import_uris();
    }
    if paths.is_empty() {
        app.info("Clipboard is empty");
        return;
    }
    let dest = app.pane().cwd.clone();
    app.progress = Some(ops::start_transfer(paths, dest, cut));
    if cut {
        app.clipboard.paths.clear();
    }
}

fn search_step(app: &mut App, dir: isize) {
    if app.search_last.is_empty() {
        return;
    }
    let needle = app.search_last.to_lowercase();
    let n = app.pane().len();
    if n == 0 {
        return;
    }
    let start = app.pane().cursor as isize;
    for k in 1..=n as isize {
        let i = (start + dir * k).rem_euclid(n as isize) as usize;
        if app
            .pane()
            .entry_at(i)
            .is_some_and(|e| e.name.to_lowercase().contains(&needle))
        {
            app.pane_mut().cursor = i;
            return;
        }
    }
    app.info(format!("Pattern not found: {}", app.search_last));
}

// ---------------------------------------------------------------------------
// Text entry
// ---------------------------------------------------------------------------

fn text(app: &mut App, k: KeyEvent) {
    match k.code {
        KeyCode::Esc => {
            if app.mode == Mode::Filter {
                app.pane_mut().filter.clear();
                app.pane_mut().refilter();
                app.filter_bar = false;
            }
            app.mode = Mode::Normal;
            app.input.clear();
        }
        KeyCode::Enter => commit(app),
        KeyCode::Backspace => {
            if app.input_cursor > 0 {
                let i = byte_at(&app.input, app.input_cursor - 1);
                app.input.remove(i);
                app.input_cursor -= 1;
                live_update(app);
            }
        }
        KeyCode::Delete => {
            if app.input_cursor < app.input.chars().count() {
                let i = byte_at(&app.input, app.input_cursor);
                app.input.remove(i);
                live_update(app);
            }
        }
        KeyCode::Left => app.input_cursor = app.input_cursor.saturating_sub(1),
        KeyCode::Right => app.input_cursor = (app.input_cursor + 1).min(app.input.chars().count()),
        KeyCode::Home => app.input_cursor = 0,
        KeyCode::End => app.input_cursor = app.input.chars().count(),
        KeyCode::Char('u') if k.modifiers == KeyModifiers::CONTROL => {
            app.input.clear();
            app.input_cursor = 0;
            live_update(app);
        }
        KeyCode::Char('w') if k.modifiers == KeyModifiers::CONTROL => {
            while app.input_cursor > 0 {
                let i = byte_at(&app.input, app.input_cursor - 1);
                let c = app.input[i..].chars().next().unwrap_or(' ');
                app.input.remove(i);
                app.input_cursor -= 1;
                if c.is_whitespace() {
                    break;
                }
            }
            live_update(app);
        }
        KeyCode::Tab if app.mode == Mode::PathEdit => complete_path(app),
        KeyCode::Char(c) => {
            let i = byte_at(&app.input, app.input_cursor);
            app.input.insert(i, c);
            app.input_cursor += 1;
            live_update(app);
        }
        _ => {}
    }
}

fn byte_at(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

/// Search and filter act on every keystroke; the rest wait for Enter.
fn live_update(app: &mut App) {
    match app.mode {
        Mode::Filter => {
            app.pane_mut().filter = app.input.clone();
            app.pane_mut().refilter();
        }
        Mode::Search => {
            let needle = app.input.to_lowercase();
            if needle.is_empty() {
                return;
            }
            let n = app.pane().len();
            for i in 0..n {
                if app
                    .pane()
                    .entry_at(i)
                    .is_some_and(|e| e.name.to_lowercase().contains(&needle))
                {
                    app.pane_mut().cursor = i;
                    return;
                }
            }
        }
        _ => {}
    }
}

fn complete_path(app: &mut App) {
    let p = PathBuf::from(&app.input);
    let (dir, frag) = if app.input.ends_with('/') {
        (p.clone(), String::new())
    } else {
        (
            p.parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/")),
            ops::name(&p),
        )
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut hits: Vec<String> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(&frag))
        .collect();
    hits.sort();
    if let Some(first) = hits.first() {
        app.input = dir.join(first).to_string_lossy().into_owned();
        app.input_cursor = app.input.chars().count();
    }
}

fn commit(app: &mut App) {
    let input = app.input.clone();
    let mode = app.mode.clone();
    app.mode = Mode::Normal;
    app.input.clear();
    app.input_cursor = 0;

    match mode {
        Mode::Command => command(app, &input),
        Mode::Search => {
            app.search_last = input;
        }
        Mode::Filter => {
            // Enter keeps the filter and leaves the bar showing, like Dolphin.
            app.pane_mut().filter = input;
            app.pane_mut().refilter();
        }
        Mode::PathEdit => {
            let p = expand(&input);
            if p.is_dir() {
                app.goto(Target::Dir(p), true);
            } else {
                app.error(format!("Not a directory: {}", p.display()));
            }
        }
        Mode::Rename(from) => match ops::rename(&from, &input) {
            Ok(op) => {
                let to = match &op {
                    ops::UndoOp::Rename { to, .. } => to.clone(),
                    _ => from.clone(),
                };
                app.undo.push(op);
                app.refresh_in_place();
                app.select_by_path(&to);
                app.info(format!("Renamed to {input}"));
            }
            Err(e) => app.error(e),
        },
        Mode::BatchRename => {
            let v = app.pane().selected_paths();
            match ops::batch_rename(&v, &input) {
                Ok(op) => {
                    app.undo.push(op);
                    app.pane_mut().selected.clear();
                    app.refresh_in_place();
                    app.info(format!("Renamed {} item(s)", v.len()));
                }
                Err(e) => app.error(e),
            }
        }
        Mode::NewFolder => {
            let d = app.pane().cwd.clone();
            match ops::new_folder(&d, &input) {
                Ok(op) => {
                    app.undo.push(op);
                    app.refresh_in_place();
                    app.select_by_path(&d.join(&input));
                    app.info(format!("Created {input}"));
                }
                Err(e) => app.error(e),
            }
        }
        Mode::NewFile => {
            let d = app.pane().cwd.clone();
            match ops::new_file(&d, &input) {
                Ok(op) => {
                    app.undo.push(op);
                    app.refresh_in_place();
                    app.info(format!("Created {input}"));
                }
                Err(e) => app.error(e),
            }
        }
        _ => {}
    }
}

fn expand(s: &str) -> PathBuf {
    let s = s.trim();
    if s == "~" {
        return places::home();
    }
    if let Some(rest) = s.strip_prefix("~/") {
        return places::home().join(rest);
    }
    PathBuf::from(s)
}

/// `:` commands. A small fixed set — PLAN.md's list, nothing speculative.
fn command(app: &mut App, line: &str) {
    let line = line.trim();
    let (cmd, arg) = match line.split_once(char::is_whitespace) {
        Some((c, a)) => (c, a.trim()),
        None => (line, ""),
    };
    match cmd {
        "q" => app.close_tab(),
        "qa" | "qall" | "quit" => app.quit = true,
        "e" | "edit" | "cd" => {
            let p = if arg.is_empty() {
                places::home()
            } else {
                expand(arg)
            };
            if p.is_dir() {
                app.goto(Target::Dir(p), true);
            } else {
                app.error(format!("Not a directory: {}", p.display()));
            }
        }
        "sort" => match arg {
            "name" => app.set_sort(SortKey::Name),
            "size" => app.set_sort(SortKey::Size),
            "date" | "time" => app.set_sort(SortKey::Date),
            "type" => app.set_sort(SortKey::Type),
            "" => app.info(format!("sort: {}", app.pane().sort.key.label())),
            _ => app.error(format!("Unknown sort key: {arg}")),
        },
        "view" => match arg {
            "icons" => app.set_view(ViewMode::Icons),
            "compact" => app.set_view(ViewMode::Compact),
            "details" => app.set_view(ViewMode::Details),
            _ => app.error(format!("Unknown view: {arg}")),
        },
        "split" => app.toggle_split(),
        "tabnew" => {
            let d = if arg.is_empty() {
                app.pane().cwd.clone()
            } else {
                expand(arg)
            };
            app.new_tab(d);
        }
        "hidden" => app.toggle_hidden(),
        "trash" => app.goto(Target::Trash, true),
        "help" => app.mode = Mode::Help,
        "" => {}
        _ => app.error(format!("Not a command: {cmd}")),
    }
}

// ---------------------------------------------------------------------------
// Modal overlays
// ---------------------------------------------------------------------------

fn confirm(app: &mut App, k: KeyEvent, what: Confirm) {
    let yes = matches!(k.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter);
    app.mode = Mode::Normal;
    if !yes {
        app.info("Cancelled");
        return;
    }
    match what {
        Confirm::DeletePermanently(paths) => match ops::delete_permanently(&paths) {
            Ok(n) => {
                app.pane_mut().selected.clear();
                app.refresh_in_place();
                app.info(format!("Deleted {n} item(s) permanently"));
            }
            Err(e) => app.error(e),
        },
        Confirm::PurgeFromTrash(paths) => match ops::purge_from_trash(&paths) {
            Ok(n) => {
                app.pane_mut().selected.clear();
                app.reload();
                app.info(format!("Deleted {n} item(s) permanently"));
            }
            Err(e) => app.error(e),
        },
        Confirm::EmptyTrash => match ops::empty_trash() {
            Ok(n) => {
                app.reload();
                app.info(format!("Emptied Trash: {n} item(s)"));
            }
            Err(e) => app.error(e),
        },
    }
}

/// Open a menu with the cursor on the row already in force. A menu that always
/// opened on its first item read as "Icons" being the mode whatever the pane
/// was actually showing, since the highlight is the only mark a row carries.
pub fn open_menu(app: &mut App, kind: MenuKind) {
    app.menu_sel = match kind {
        MenuKind::ViewMode => match app.pane().view {
            ViewMode::Icons => 0,
            ViewMode::Compact => 1,
            ViewMode::Details => 2,
        },
        MenuKind::Sort => match app.pane().sort.key {
            SortKey::Name => 0,
            SortKey::Size => 1,
            SortKey::Date => 2,
            SortKey::Type => 3,
        },
        // The hamburger is a list of verbs; none of them is in force.
        MenuKind::Hamburger => 0,
    };
    app.mode = Mode::Menu(kind);
}

/// Menu contents. Kept next to the handler so adding an item cannot forget one.
pub fn menu_items(kind: &MenuKind) -> Vec<(&'static str, Action)> {
    match kind {
        MenuKind::Hamburger => vec![
            ("New Folder…            F10", Action::NewFolder),
            ("New File…                o", Action::NewFile),
            ("Rename…                 F2", Action::Rename),
            ("Move to Trash        x/Del", Action::Trash),
            ("Delete            Shift+Del", Action::DeletePerm),
            ("Cut                   Ctrl+X", Action::Cut),
            ("Copy                  Ctrl+C", Action::Copy),
            ("Paste                 Ctrl+V", Action::Paste),
            ("Compress                  ", Action::Compress),
            ("Restore from Trash        ", Action::Restore),
            ("Empty Trash               ", Action::EmptyTrash),
            ("Extract here              ", Action::Extract),
            ("Select All            Ctrl+A", Action::SelectAll),
            ("Show Hidden Files          H", Action::ToggleHidden),
            ("Filter                Ctrl+I", Action::ToggleFilterBar),
            ("Sort by…                   ", Action::OpenSortMenu),
            ("View mode…                 ", Action::OpenViewMenu),
            ("Split View                F3", Action::ToggleSplit),
            ("Places Panel              F9", Action::TogglePlaces),
            ("Information Panel        F11", Action::ToggleInfo),
            ("Open Terminal Here        F4", Action::TerminalHere),
            ("Properties          Alt+Enter", Action::Properties),
            ("Help                      F1", Action::Help),
            ("Quit                  Ctrl+Q", Action::QuitAll),
        ],
        MenuKind::ViewMode => vec![
            ("Icons                 Ctrl+1", Action::ViewIcons),
            ("Compact               Ctrl+2", Action::ViewCompact),
            ("Details               Ctrl+3", Action::ViewDetails),
        ],
        MenuKind::Sort => vec![
            ("Name", Action::SortName),
            ("Size", Action::SortSize),
            ("Modified", Action::SortDate),
            ("Type", Action::SortType),
            ("Reverse order", Action::ToggleSortOrder),
            ("Folders first", Action::ToggleDirsFirst),
        ],
    }
}

fn menu(app: &mut App, k: KeyEvent, kind: MenuKind) {
    let items = menu_items(&kind);
    // A menu hanging off the toolbar is also a place in that row, so the row's
    // own motions work from inside it. That takes `h` and `l`, which is why
    // accepting is Enter and not `l`: a motion that sometimes acts is the
    // ambiguity the breadcrumb already refuses.
    if let Some(i) = menu_owner(&kind) {
        if toolbar_nav(app, k, i) {
            return;
        }
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let n = items.len();
    let accept = |app: &mut App| {
        let a = items[app.menu_sel].1;
        leave_toolbar(app);
        act(app, a, 1);
    };
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Char('n') if ctrl => app.menu_sel = (app.menu_sel + 1) % n,
        KeyCode::Char('p') if ctrl => app.menu_sel = (app.menu_sel + n - 1) % n,
        KeyCode::Char('y') if ctrl => accept(app),
        KeyCode::Down | KeyCode::Char('j') => app.menu_sel = (app.menu_sel + 1) % n,
        KeyCode::Up | KeyCode::Char('k') => app.menu_sel = (app.menu_sel + n - 1) % n,
        KeyCode::Home | KeyCode::Char('g') => app.menu_sel = 0,
        KeyCode::End | KeyCode::Char('G') => app.menu_sel = n - 1,
        KeyCode::Enter | KeyCode::Tab => accept(app),
        _ => {}
    }
}

/// The dropdown Dolphin hangs off a breadcrumb segment: the siblings of the
/// next path component, so you can hop sideways without going up first.
pub fn crumb_siblings(app: &App, seg: usize) -> Vec<PathBuf> {
    let comps: Vec<PathBuf> = crate::ui::crumb_paths(&app.pane().cwd);
    let Some(dir) = comps.get(seg) else {
        return Vec::new();
    };
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| app.pane().show_hidden || !ops::name(p).starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// The rightmost trail segment with subdirectories to list. Only the segment
/// you are standing in can be childless — every one to its left holds at least
/// the path you came through — so in a real directory this always finds one.
/// A virtual place like `trash:/` has no trail at all, hence the `None`.
fn openable_crumb(app: &App) -> Option<usize> {
    let mut seg = crate::ui::crumb_paths(&app.pane().cwd)
        .len()
        .saturating_sub(1);
    loop {
        if !crumb_siblings(app, seg).is_empty() {
            return Some(seg);
        }
        seg = seg.checked_sub(1)?;
    }
}

/// Open segment `seg`, remembering it as where the breadcrumb was left.
///
/// The row is the one left highlighted last time this same segment was open.
/// Failing that — a first visit, or a pick that is no longer there — it is the
/// child you are standing in, the one row already in force, as in `open_menu`.
pub fn open_crumb(app: &mut App, seg: usize) {
    let trail = crate::ui::crumb_paths(&app.pane().cwd);
    let sibs = crumb_siblings(app, seg);
    let returning = app.pane().crumb_focus == trail.get(seg).cloned();
    let pick = app.pane().crumb_pick.clone().filter(|_| returning);
    app.menu_sel = pick
        .or_else(|| trail.get(seg + 1).cloned())
        .and_then(|p| sibs.iter().position(|s| *s == p))
        .unwrap_or(0);
    app.pane_mut().crumb_focus = trail.get(seg).cloned();
    app.pane_mut().crumb_pick = sibs.get(app.menu_sel).cloned();
    app.mode = Mode::CrumbMenu(seg);
}

/// Into the breadcrumb, or — when there is no trail to open — onto the button
/// given as the fallback, so the toolbar stays reachable from every place.
///
/// Focus returns to the segment it was left on, not to the end of the trail.
/// Walking down a tree and back up is the common errand, and always landing on
/// the deepest segment means re-walking left across the trail every time. The
/// segment is remembered by path: after descending into it, it is still there,
/// one place further from the end. A path no longer on the trail — a jump to
/// some other tree — falls back to the rightmost openable segment.
fn enter_crumbs(app: &mut App, fallback: usize) {
    let remembered = app.pane().crumb_focus.clone().and_then(|p| {
        crate::ui::crumb_paths(&app.pane().cwd)
            .iter()
            .position(|c| *c == p)
    });
    let seg = remembered
        .filter(|&s| !crumb_siblings(app, s).is_empty())
        .or_else(|| openable_crumb(app));
    match seg {
        Some(seg) => open_crumb(app, seg),
        None => focus_button(app, fallback),
    }
}

/// The toolbar buttons, driven from the keyboard. The row is three panes side
/// by side — nav group, breadcrumb, right group — so Ctrl+h and Ctrl+l cross
/// between them while bare h and l walk the buttons within one.
fn buttons(app: &mut App, k: KeyEvent, i: usize) {
    if toolbar_nav(app, k, i) {
        return;
    }
    match k.code {
        KeyCode::Char('y') if k.modifiers.contains(KeyModifiers::CONTROL) => press_button(app, i),
        KeyCode::Enter | KeyCode::Tab => press_button(app, i),
        _ => {}
    }
}

/// The menu a toolbar button drops, if it drops one.
fn button_menu(i: usize) -> Option<MenuKind> {
    match config::TOOLBAR_BTNS.get(i) {
        Some(Action::OpenViewMenu) => Some(MenuKind::ViewMode),
        Some(Action::OpenMenu) => Some(MenuKind::Hamburger),
        _ => None,
    }
}

/// The button a menu hangs from. `Mode::Menu` carries no index because the same
/// menu opens from `m` and from the right button too, so the owner is derived
/// rather than stored — there is nothing to keep in step.
pub fn menu_owner(kind: &MenuKind) -> Option<usize> {
    let want = match kind {
        MenuKind::ViewMode => Action::OpenViewMenu,
        MenuKind::Hamburger => Action::OpenMenu,
        MenuKind::Sort => return None,
    };
    config::TOOLBAR_BTNS.iter().position(|a| *a == want)
}

/// Put focus on toolbar button `i`. A button that drops a menu shows it on
/// arrival, so the row reads like the breadcrumb, where landing on a segment
/// *is* opening its dropdown. `Mode::Menu` is then both "the menu is open" and
/// "focus is on its button", which is why there is no flag for either.
fn focus_button(app: &mut App, i: usize) {
    match button_menu(i) {
        Some(kind) => open_menu(app, kind),
        None => {
            app.menu_sel = 0;
            app.mode = Mode::Buttons(i);
        }
    }
}

/// The keys that move focus around the toolbar row, for a focus resting on
/// button `i`. Shared by a bare button and by a menu hanging off one: they are
/// the same position in the row. True when the key was consumed.
fn toolbar_nav(app: &mut App, k: KeyEvent, i: usize) -> bool {
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let nav = i < config::NAV_BTNS;
    let (lo, hi) = if nav {
        (0, config::NAV_BTNS - 1)
    } else {
        (config::NAV_BTNS, config::TOOLBAR_BTNS.len() - 1)
    };
    match k.code {
        KeyCode::Char('j') if ctrl => app.mode = Mode::Normal,
        KeyCode::Char('k') if ctrl => {}
        // Crossing lands on the trail, or steps straight over it to the other
        // group when the place has no trail to open.
        KeyCode::Char('h') if ctrl => {
            if !nav {
                enter_crumbs(app, config::NAV_BTNS - 1)
            }
        }
        KeyCode::Char('l') if ctrl => {
            if nav {
                enter_crumbs(app, config::NAV_BTNS)
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Left | KeyCode::Char('h') => focus_button(app, i.saturating_sub(1).max(lo)),
        KeyCode::Right | KeyCode::Char('l') => focus_button(app, (i + 1).min(hi)),
        _ => return false,
    }
    true
}

/// Come down from the toolbar by *acting*, as opposed to cancelling. The action
/// was asked for from up in the row but it lands in the file view, so focus goes
/// there rather than back to whichever pane the row was entered from. `Esc` and
/// `Ctrl+j` are the ways back to where you came up from.
pub fn leave_toolbar(app: &mut App) {
    app.mode = Mode::Normal;
    app.focus = Focus::View;
}

/// Leave the toolbar first: a button whose action opens a menu has to be able
/// to set the mode after us, not have it overwritten.
fn press_button(app: &mut App, i: usize) {
    let Some(a) = config::TOOLBAR_BTNS.get(i) else {
        return;
    };
    leave_toolbar(app);
    act(app, *a, 1);
}

/// The breadcrumb, driven from the keyboard. `Mode::CrumbMenu(seg)` is both
/// "focus is up in the breadcrumb" and "segment `seg` has its menu open" —
/// there is no third state where the crumb has focus with everything shut, so
/// there is no flag for one.
fn crumb_menu(app: &mut App, k: KeyEvent, seg: usize) {
    let items = crumb_siblings(app, seg);
    if items.is_empty() {
        app.mode = Mode::Normal;
        return;
    }
    let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
    let last_seg = crate::ui::crumb_paths(&app.pane().cwd)
        .len()
        .saturating_sub(1);
    let walk = open_crumb;
    match k.code {
        // Ctrl+j is the way back down, the mirror of the Ctrl+k that came up.
        // Ctrl+h and Ctrl+l are pane motions and the toolbar row holds three
        // panes, so they step out to the button group on either side, landing
        // on the button nearest the trail.
        KeyCode::Char('j') if ctrl => app.mode = Mode::Normal,
        KeyCode::Char('k') if ctrl => {}
        KeyCode::Char('h') if ctrl => focus_button(app, config::NAV_BTNS - 1),
        KeyCode::Char('l') if ctrl => focus_button(app, config::NAV_BTNS),
        KeyCode::Char('n') if ctrl => app.menu_sel = (app.menu_sel + 1) % items.len(),
        KeyCode::Char('p') if ctrl => app.menu_sel = (app.menu_sel + items.len() - 1) % items.len(),
        KeyCode::Char('y') if ctrl => accept_crumb(app, &items),
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        // Inside the pane, bare motions move: h/l along the trail, j/k down the
        // menu. `l` does not enter a directory — that is what accept is for,
        // and a motion key that sometimes navigates is the ambiguity to avoid.
        KeyCode::Left | KeyCode::Char('h') => walk(app, seg.saturating_sub(1)),
        KeyCode::Right | KeyCode::Char('l') => walk(app, (seg + 1).min(last_seg)),
        KeyCode::Down | KeyCode::Char('j') => app.menu_sel = (app.menu_sel + 1) % items.len(),
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_sel = (app.menu_sel + items.len() - 1) % items.len()
        }
        KeyCode::Enter | KeyCode::Tab => accept_crumb(app, &items),
        _ => {}
    }
    // Record where the row ended up, so leaving and coming back lands on it.
    // Only while this same segment is still the open one: `walk` has already
    // set the pair for the segment it moved to, and leaving the trail entirely
    // must not overwrite the pick with a row from a menu that is now shut.
    if app.mode == Mode::CrumbMenu(seg) {
        app.pane_mut().crumb_pick = items.get(app.menu_sel).cloned();
    }
}

fn accept_crumb(app: &mut App, items: &[PathBuf]) {
    let Some(d) = items.get(app.menu_sel).cloned() else {
        return;
    };
    leave_toolbar(app);
    app.goto(Target::Dir(d), true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app() -> App {
        App::new(std::env::temp_dir())
    }

    fn press(a: &mut App, c: char) {
        handle_key_event(a, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    #[test]
    fn digits_accumulate_into_a_count() {
        let mut a = app();
        press(&mut a, '1');
        press(&mut a, '2');
        assert_eq!(a.count, "12");
    }

    #[test]
    fn a_bare_zero_is_a_motion_not_a_count() {
        let mut a = app();
        press(&mut a, '0');
        assert!(a.count.is_empty());
    }

    #[test]
    fn gg_completes_as_a_chord() {
        let mut a = app();
        press(&mut a, 'g');
        assert_eq!(a.pending, Some('g'));
        press(&mut a, 'g');
        assert_eq!(a.pending, None);
        assert_eq!(a.pane().cursor, 0);
    }

    #[test]
    fn colon_enters_command_mode_and_esc_leaves_it() {
        let mut a = app();
        press(&mut a, ':');
        assert_eq!(a.mode, Mode::Command);
        handle_key_event(&mut a, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn unknown_commands_report_rather_than_panic() {
        let mut a = app();
        command(&mut a, "nonsense");
        assert!(a.status_is_error);
    }

    #[test]
    fn view_command_switches_mode() {
        let mut a = app();
        command(&mut a, "view details");
        assert_eq!(a.pane().view, ViewMode::Details);
    }

    #[test]
    fn tilde_expands_to_home() {
        assert_eq!(expand("~"), places::home());
        assert_eq!(expand("~/x"), places::home().join("x"));
    }

    #[test]
    fn unbound_printable_keys_feed_typeahead() {
        let mut a = app();
        press(&mut a, 'q');
        assert_eq!(a.typeahead, "q");
    }
}
