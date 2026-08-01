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

pub fn key(app: &mut App, k: KeyEvent) {
    // The transfer popup is modal, as Dolphin's is. Letting keys through means
    // you can navigate away from a live copy, or start a second one on top of
    // `app.progress` and orphan the first thread with no way to see or stop it.
    if let Some(p) = &app.progress {
        if k.code == KeyCode::Esc {
            p.cancel.store(true, Ordering::Relaxed);
        }
        return;
    }

    match app.mode.clone() {
        Mode::Normal | Mode::Visual => normal(app, k),
        Mode::Confirm(c) => confirm(app, k, c),
        // Any key dismisses an information overlay.
        Mode::Properties | Mode::Help => app.mode = Mode::Normal,
        Mode::Menu(kind) => menu(app, k, kind),
        Mode::CrumbMenu(i) => crumb_menu(app, k, i),
        _ => text(app, k),
    }
}

// ---------------------------------------------------------------------------
// Normal / Visual
// ---------------------------------------------------------------------------

fn normal(app: &mut App, k: KeyEvent) {
    if k.code == KeyCode::Esc {
        app.count.clear();
        app.pending = None;
        if app.mode == Mode::Visual {
            app.mode = Mode::Normal;
        } else {
            app.pane_mut().selected.clear();
        }
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

fn lookup(k: KeyEvent) -> Option<Action> {
    let m = k.modifiers.difference(KeyModifiers::NONE);
    config::VIM_KEYS
        .iter()
        .chain(config::DOLPHIN_KEYS.iter())
        .find(|b| b.code == k.code && b.mods == m)
        .map(|b| b.action)
}

fn take_count(app: &mut App) -> usize {
    let n = app.count.parse().unwrap_or(1);
    app.count.clear();
    n.max(1)
}

/// Keys the Places panel consumes while it has focus. Returns true when handled.
fn places_key(app: &mut App, k: KeyEvent) -> bool {
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
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => {
            if let Some(t) = app.places[app.places_sel].target().cloned() {
                app.goto(t, true);
                app.focus = Focus::View;
            }
        }
        KeyCode::Char('h') | KeyCode::Left | KeyCode::Tab => app.focus = Focus::View,
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
    let extend = app.mode == Mode::Visual;
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
        Action::MoveDown => app.move_cursor(stride * n as isize, extend),
        Action::MoveUp => app.move_cursor(-stride * n as isize, extend),
        Action::MoveRight => {
            if app.pane().view == ViewMode::Details {
                // `l` opens in Details, where there is no horizontal axis.
                app.activate();
            } else {
                app.move_cursor(n as isize, extend);
            }
        }
        Action::MoveLeft => {
            if app.pane().view == ViewMode::Details {
                app.go_up();
            } else {
                app.move_cursor(-(n as isize), extend);
            }
        }
        Action::Top => {
            // `5gg` goes to item 5, like vim's line numbers.
            let target = if app.count.is_empty() {
                n.saturating_sub(1)
            } else {
                n - 1
            };
            app.goto_index(target, extend);
        }
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
        Action::EnterVisual => {
            app.mode = Mode::Visual;
            let c = app.pane().cursor;
            app.pane_mut().anchor = c;
            app.toggle_select();
        }
        Action::EnterVisualLine => {
            // `V` takes the whole row in a grid, the whole listing in Details.
            let (c, s) = (app.pane().cursor, app.pane().stride());
            let (start, end) = if app.pane().view == ViewMode::Details {
                (0, app.pane().len().saturating_sub(1))
            } else {
                (
                    c - c % s,
                    (c - c % s + s - 1).min(app.pane().len().saturating_sub(1)),
                )
            };
            let paths: Vec<PathBuf> = (start..=end)
                .filter_map(|i| app.pane().at(i).map(|e| e.path.clone()))
                .collect();
            app.pane_mut().selected.extend(paths);
        }

        // file operations
        Action::Copy => {
            let v = app.pane().selected_paths();
            let n = v.len();
            app.clipboard.set(v, false);
            app.info(format!("Copied {n} item(s)"));
            app.mode = Mode::Normal;
        }
        Action::Cut => {
            let v = app.pane().selected_paths();
            let n = v.len();
            app.clipboard.set(v, true);
            app.info(format!("Cut {n} item(s)"));
            app.mode = Mode::Normal;
        }
        Action::Paste => paste(app),
        Action::Trash => {
            let v = app.pane().selected_paths();
            if v.is_empty() {
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
            app.mode = Mode::Normal;
        }
        Action::DeletePerm => {
            let v = app.pane().selected_paths();
            if !v.is_empty() {
                app.mode = Mode::Confirm(Confirm::DeletePermanently(v));
            }
        }
        Action::Rename => start_rename(app),
        Action::NewFolder => {
            app.mode = Mode::NewFolder;
            app.input.clear();
            app.input_cursor = 0;
        }
        Action::NewFile => {
            app.mode = Mode::NewFile;
            app.input.clear();
            app.input_cursor = 0;
        }
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
            let v = app.pane().selected_paths();
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
        Action::SwapPane => {
            if app.split_on() {
                app.other_pane();
            } else if app.places_visible {
                app.focus = if app.focus == Focus::View {
                    Focus::Places
                } else {
                    Focus::View
                };
            }
        }
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
        Action::OpenMenu => {
            app.mode = Mode::Menu(MenuKind::Hamburger);
            app.menu_sel = 0;
        }
        Action::OpenViewMenu => {
            app.mode = Mode::Menu(MenuKind::ViewMode);
            app.menu_sel = 0;
        }
        Action::OpenSortMenu => {
            app.mode = Mode::Menu(MenuKind::Sort);
            app.menu_sel = 0;
        }
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
            .at(i)
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
                    .at(i)
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
    let yes = matches!(
        k.code,
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter
    );
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
        Confirm::EmptyTrash => match ops::empty_trash() {
            Ok(n) => {
                app.reload();
                app.info(format!("Emptied Trash: {n} item(s)"));
            }
            Err(e) => app.error(e),
        },
        Confirm::Quit => app.quit = true,
    }
}

/// Menu contents. Kept next to the handler so adding an item cannot forget one.
pub fn menu_items(kind: &MenuKind) -> Vec<(&'static str, Action)> {
    match kind {
        MenuKind::Hamburger => vec![
            ("New Folder…            F10", Action::NewFolder),
            ("New File…                o", Action::NewFile),
            ("Rename…                 F2", Action::Rename),
            ("Move to Trash          Del", Action::Trash),
            ("Delete            Shift+Del", Action::DeletePerm),
            ("Copy                  Ctrl+C", Action::Copy),
            ("Paste                 Ctrl+V", Action::Paste),
            ("Compress                  ", Action::Compress),
            ("Restore from Trash        ", Action::Restore),
            ("Empty Trash               ", Action::EmptyTrash),
            ("Extract here              ", Action::Extract),
            ("Select All            Ctrl+A", Action::SelectAll),
            ("Show Hidden Files     Ctrl+H", Action::ToggleHidden),
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
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Down | KeyCode::Char('j') => {
            app.menu_sel = (app.menu_sel + 1) % items.len();
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_sel = (app.menu_sel + items.len() - 1) % items.len();
        }
        KeyCode::Home | KeyCode::Char('g') => app.menu_sel = 0,
        KeyCode::End | KeyCode::Char('G') => app.menu_sel = items.len() - 1,
        KeyCode::Enter | KeyCode::Char('l') => {
            let a = items[app.menu_sel].1;
            app.mode = Mode::Normal;
            act(app, a, 1);
        }
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

fn crumb_menu(app: &mut App, k: KeyEvent, seg: usize) {
    let items = crumb_siblings(app, seg);
    if items.is_empty() {
        app.mode = Mode::Normal;
        return;
    }
    match k.code {
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Down | KeyCode::Char('j') => app.menu_sel = (app.menu_sel + 1) % items.len(),
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_sel = (app.menu_sel + items.len() - 1) % items.len()
        }
        KeyCode::Enter | KeyCode::Char('l') => {
            let d = items[app.menu_sel].clone();
            app.mode = Mode::Normal;
            app.goto(Target::Dir(d), true);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn app() -> App {
        App::new(std::env::temp_dir())
    }

    fn press(a: &mut App, c: char) {
        key(a, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
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
        key(&mut a, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
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
