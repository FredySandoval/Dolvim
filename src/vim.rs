//! The modal input engine, and the single place where an `Action` turns into
//! a state change.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::ops;
use crate::config;
use crate::fs::SortKey;
use crate::places::{self, Target};
use crate::app::{App, Confirm, Focus, MenuKind, Mode, ViewMode};

/// Modifiers as the keymap has to compare them.
///
/// Terminals disagree about whether a shifted printable also reports SHIFT: `G`
/// may arrive as `Char('G')` with the modifier or without it, depending on the
/// terminal and on which keyboard protocol is in force. The character's own
/// case already carries the shift, so it is dropped and the character alone
/// decides. `BackTab` is the same story from the other side — the shift that
/// produced it is spent on making it `BackTab`.
///
/// Every event and every table row passes through here, so the two can never
/// disagree about what a row means.
pub fn normalize_mods(code: KeyCode, mods: KeyModifiers) -> KeyModifiers {
    match code {
        KeyCode::Char(_) | KeyCode::BackTab => mods.difference(KeyModifiers::SHIFT),
        _ => mods,
    }
}

/// The one door in. Normalizing here rather than at each comparison is what
/// lets the rest of this file test `modifiers.is_empty()` and mean "bare key".
fn normalize(key_event: KeyEvent) -> KeyEvent {
    KeyEvent {
        modifiers: normalize_mods(key_event.code, key_event.modifiers),
        ..key_event
    }
}

pub fn handle_key_event(app: &mut App, key_event: KeyEvent) {
    let key_event = normalize(key_event);
    // The transfer popup is modal, as Dolphin's is. Letting keys through means
    // you can navigate away from a live copy, or start a second one on top of
    // `app.transfer_progress` and orphan the first thread with no way to see or stop it.
    if let Some(active_transfer) = &app.transfer_progress {
        if key_event.code == KeyCode::Esc {
            active_transfer
                .cancel_requested
                .store(true, Ordering::Relaxed);
        }
        return;
    }

    match app.mode.clone() {
        Mode::Normal | Mode::Visual | Mode::VisualLine => handle_normal_key(app, key_event),
        Mode::Confirm(c) => handle_confirm_key(app, key_event, c),
        // Any key dismisses an information overlay.
        Mode::Properties | Mode::Help => app.mode = Mode::Normal,
        Mode::Menu(kind) => handle_menu_key(app, key_event, kind),
        Mode::CrumbMenu(i) => handle_crumb_menu_key(app, key_event, i),
        Mode::Buttons(i) => handle_buttons_key(app, key_event, i),
        _ => handle_text_key(app, key_event),
    }
}

// ---------------------------------------------------------------------------
// Normal / Visual
// ---------------------------------------------------------------------------

fn handle_normal_key(app: &mut App, key_event: KeyEvent) {
    if key_event.code == KeyCode::Esc {
        app.count.clear();
        app.pending_chord_leader = None;
        app.pending_delete = None;
        // Esc ends the visual and takes the range with it: the selection
        // belonged to the drag, not to the pane. Outside a visual it is the
        // same key that clears a selection built with Space.
        app.mode = Mode::Normal;
        app.pane_mut().selected.clear();
        return;
    }

    // A pending chord leader owns the next key, whatever it is.
    if let Some(pending_leader) = app.pending_chord_leader.take() {
        if let KeyCode::Char(c) = key_event.code {
            if let Some(chord) = config::CHORDS
                .iter()
                .find(|chord| chord.leader == pending_leader && chord.follower == c)
            {
                let n = take_count(app);
                run_action(app, chord.action, n);
                return;
            }
        }
        return;
    }

    // Count prefix. A bare `0` is a motion, not a count.
    if let KeyCode::Char(c @ '0'..='9') = key_event.code {
        if key_event.modifiers.is_empty() && !(c == '0' && app.count.is_empty()) {
            app.count.push(c);
            return;
        }
    }

    // A `d` owns the next motion. After the count block, so `d5j` can collect
    // its 5 the same way `5j` does. A key the operator cannot use cancels it
    // and then means what it usually means, so `d` then Ctrl+d scrolls rather
    // than being swallowed.
    if let Some(count_before_operator) = app.pending_delete.take() {
        if delete_motion(app, key_event, count_before_operator) == MotionResult::Handled {
            return;
        }
    }

    if app.focus == Focus::Places && places_key(app, key_event) {
        return;
    }

    if let KeyCode::Char(c) = key_event.code {
        if key_event.modifiers.is_empty() && is_chord_leader(c) {
            // `c` is only a leader when a follower can complete it; `cw` is the
            // one chord it starts, so a lone `c` simply waits.
            app.pending_chord_leader = Some(c);
            return;
        }
    }

    if let Some(a) = lookup_binding(key_event) {
        let n = take_count(app);
        run_action(app, a, n);
        return;
    }

    // Anything printable and unbound is Dolphin's type-ahead.
    if let KeyCode::Char(c) = key_event.code {
        if key_event.modifiers.is_empty() {
            app.typeahead(c);
        }
    }
}

/// A leader is a key some chord starts with — read off `config::CHORDS` rather
/// than listed beside it, so a new leader cannot be half-added. Eight rows: the
/// scan costs nothing, and a second spelling costs a chord that never fires.
fn is_chord_leader(c: char) -> bool {
    config::CHORDS.iter().any(|chord| chord.leader == c)
}

/// Whether an operator's second key was a motion it could use. A cancelled
/// motion has to reach the normal-mode dispatch instead of being dropped.
#[derive(PartialEq, Eq, Debug)]
#[must_use]
enum MotionResult {
    Handled,
    Cancelled,
}

/// The rows `d{motion}` covers, or `None` when the key is not a motion the
/// operator can use. Pure, because the fencepost is the whole of it.
///
/// `count` is the product of the counts either side of the `d`. Everything
/// saturates and both ends clamp: a count is whatever the user typed, and
/// `9999999999d` must be a long delete, not a panic.
fn delete_range(
    code: KeyCode,
    cursor: usize,
    last: usize,
    count: usize,
) -> Option<(usize, usize)> {
    let below = |n: usize| cursor.saturating_add(n).min(last);
    Some(match code {
        // `dd` is this line and the n-1 below it; `dj` is this line *and* the
        // n below, one more, exactly as in vim.
        KeyCode::Char('d') => (cursor, below(count.saturating_sub(1))),
        KeyCode::Char('j') | KeyCode::Down => (cursor, below(count)),
        KeyCode::Char('k') | KeyCode::Up => (cursor.saturating_sub(count), cursor.min(last)),
        // `dG` is to the end whatever the count. In vim a count there names a
        // line number, and a pane has no line numbers to name.
        KeyCode::Char('G') => (cursor, last),
        _ => return None,
    })
}

/// Resolve `d{motion}` into a linewise range and trash it.
///
/// `count_before_operator` is the count typed before the `d`; vim multiplies it
/// by the one typed after, so `2d3j` is six lines. Anything that is not a
/// motion cancels, which is what vim does with a key an operator cannot use —
/// and the count survives the cancel, since it was never spent.
fn delete_motion(
    app: &mut App,
    key_event: KeyEvent,
    count_before_operator: usize,
) -> MotionResult {
    // Ctrl+d is half-page down, not `dd`; only a bare motion completes the
    // operator. Modifiers arrive normalized, so bare is literally none.
    if !key_event.modifiers.is_empty() {
        return MotionResult::Cancelled;
    }
    let total_count = count_before_operator.saturating_mul(peek_count(app));
    let cursor = app.pane().cursor;
    let last = app.pane().len().saturating_sub(1);
    let Some((range_start, range_end)) =
        delete_range(key_event.code, cursor, last, total_count)
    else {
        return MotionResult::Cancelled;
    };
    app.count.clear();
    let range_paths = app.pane().paths_in(range_start, range_end);
    move_to_trash(app, range_paths);
    MotionResult::Handled
}

/// What an operation acts on: the selection when there is one, otherwise
/// `count` rows from the cursor down. Every destructive action follows this
/// rule.
fn operand_paths(app: &App, count: usize) -> Vec<PathBuf> {
    if app.pane().selected.is_empty() {
        let c = app.pane().cursor;
        app.pane().paths_in(c, c + count - 1)
    } else {
        app.pane().selected_paths()
    }
}

/// The one path to the Trash. Every delete key ends here so that the undo
/// entry, the message and the refresh cannot drift apart.
fn move_to_trash(app: &mut App, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    // In the Trash there is no further "away" to move something to, so `x`
    // means purge, as it does in Dolphin. It goes behind the Shift+Del
    // confirmation: this is the one place the key is not undoable.
    if app.pane().target == Target::Trash {
        app.mode = Mode::Confirm(Confirm::PurgeFromTrash(paths));
        return;
    }
    match ops::trash(&paths) {
        Ok(op) => {
            app.undo.push(op);
            app.info(format!("Moved {} item(s) to Trash", paths.len()));
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
fn enter_visual(app: &mut App, target_mode: Mode) {
    if app.mode == target_mode {
        app.mode = Mode::Normal;
        return;
    }
    if !app.mode.is_visual() {
        app.pane_mut().anchor = app.pane().cursor;
        app.pane_mut().selected.clear();
    }
    app.mode = target_mode;
    // Redraw the range under the new rule: `V` has to reach the row edges even
    // though the cursor has not moved yet.
    app.move_cursor(0, true);
}

/// The vim table first, so `h`/`j`/`k`/`l` are motions. Rows are normalized as
/// the event was, so the SHIFT column cannot make a row unreachable.
fn lookup_binding(key_event: KeyEvent) -> Option<Action> {
    config::VIM_KEYS
        .iter()
        .chain(config::DOLPHIN_KEYS.iter())
        .find(|bind| {
            bind.code == key_event.code
                && normalize_mods(bind.code, bind.mods) == key_event.modifiers
        })
        .map(|bind| bind.action)
}

/// The count typed so far, without spending it. A count is only spent by the
/// thing that acts on it, so a key that turns out not to act leaves it standing.
fn peek_count(app: &App) -> usize {
    app.count.parse().unwrap_or(1).max(1)
}

fn take_count(app: &mut App) -> usize {
    let n = peek_count(app);
    app.count.clear();
    n
}

/// Keys the Places panel consumes while it has focus. Returns true when handled.
fn places_key(app: &mut App, key_event: KeyEvent) -> bool {
    // A control chord is never Places navigation: `Ctrl+h` must move focus, not
    // be read as the bare `h` that leaves the panel.
    if key_event
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        return false;
    }
    let move_places_cursor = |app: &mut App, direction: isize| {
        let row_count = app.places.len() as isize;
        let mut row_index = app.places_cursor as isize;
        for _ in 0..row_count {
            row_index = (row_index + direction).rem_euclid(row_count);
            if app.places[row_index as usize].is_selectable() {
                break;
            }
        }
        app.places_cursor = row_index as usize;
    };
    match key_event.code {
        KeyCode::Char('j') | KeyCode::Down => move_places_cursor(app, 1),
        KeyCode::Char('k') | KeyCode::Up => move_places_cursor(app, -1),
        // `l` points the view at the place and stays, so the panel can be walked
        // while the view follows. `Enter` means "this one, take me there" and so
        // hands focus over as well. A bare motion never leaves its pane; an
        // accept key may.
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            if let Some(t) = app.places[app.places_cursor].target().cloned() {
                app.goto(t, true);
                if key_event.code == KeyCode::Enter {
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
pub const fn bind(code: KeyCode, mods: KeyModifiers, action: Action) -> Bind {
    Bind { code, mods, action }
}

pub fn run_action(app: &mut App, action: Action, count: usize) {
    let extend = app.mode.is_visual();
    let stride = app.pane().stride() as isize;
    let page = app.pane().page() as isize;
    match action {
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
        // action line is action row and `h`/`l` walk it; Compact runs down its columns, so
        // action line is action column and `j`/`k` walk it. Details has stride 1 and no
        // horizontal axis at all, so `j`/`k` simply cross.
        Action::MoveDown | Action::MoveUp => {
            let cursor_delta = if action == Action::MoveUp {
                -(count as isize)
            } else {
                count as isize
            };
            if app.pane().view == ViewMode::Compact {
                app.step_along(cursor_delta, extend);
            } else {
                app.step_across(cursor_delta, extend);
            }
        }
        Action::MoveLeft | Action::MoveRight => {
            let left = action == Action::MoveLeft;
            let cursor_delta = if left {
                -(count as isize)
            } else {
                count as isize
            };
            match app.pane().view {
                // `l` opens in Details, `h` goes up. There is nothing sideways.
                ViewMode::Details if left => app.go_up(),
                ViewMode::Details => app.activate(),
                ViewMode::Compact => app.step_across(cursor_delta, extend),
                ViewMode::Icons => app.step_along(cursor_delta, extend),
            }
        }
        // `5gg` goes to item 5, like vim's line numbers. `count` is 1 when no count
        // was typed, so action bare `gg` lands on the first item.
        Action::Top => app.goto_index(count - 1, extend),
        Action::Bottom => {
            let last = app.pane().len().saturating_sub(1);
            app.goto_index(last, extend);
        }
        Action::HalfPageDown => app.move_cursor(page / 2 * count as isize, extend),
        Action::HalfPageUp => app.move_cursor(-page / 2 * count as isize, extend),
        Action::PageDown => app.move_cursor(page * count as isize, extend),
        Action::PageUp => app.move_cursor(-page * count as isize, extend),
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
            let cut = action == Action::Cut;
            let clipboard_paths = app.pane().selected_paths();
            let copied_count = clipboard_paths.len();
            app.clipboard.set(clipboard_paths, cut);
            app.info(format!(
                "{} {copied_count} item(s)",
                if cut { "Cut" } else { "Copied" }
            ));
            app.mode = Mode::Normal;
        }
        Action::Paste => paste_clipboard(app),
        Action::Trash => {
            // `x` on the cursor, `5x` on five rows, the selection if there is
            // one. A selection is an explicit range, so it beats the count
            // rather than being sliced by it.
            let trash_paths = operand_paths(app, count);
            move_to_trash(app, trash_paths);
        }
        Action::DeleteOp => {
            // A selection already is the range, so there is nothing to wait for.
            if app.pane().selected.is_empty() {
                app.pending_delete = Some(count);
            } else {
                let trash_paths = app.pane().selected_paths();
                move_to_trash(app, trash_paths);
            }
        }
        Action::DeletePerm => {
            let perm_delete_paths = operand_paths(app, count);
            if !perm_delete_paths.is_empty() {
                app.mode = Mode::Confirm(if app.pane().target == Target::Trash {
                    Confirm::PurgeFromTrash(perm_delete_paths)
                } else {
                    Confirm::DeletePermanently(perm_delete_paths)
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
            let compress_paths = app.pane().selected_paths();
            if compress_paths.is_empty() {
                return;
            }
            let dest = app
                .pane()
                .cwd
                .join(format!("{}.tar.gz", ops::file_name_of(&compress_paths[0])));
            match ops::compress(&compress_paths, &dest) {
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
            let restore_paths = operand_paths(app, count);
            match ops::restore_from_trash(&restore_paths) {
                Ok(restored_count) => {
                    app.info(format!("Restored {restored_count} item(s)"));
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
            let next_view = match app.pane().view {
                ViewMode::Icons => ViewMode::Compact,
                ViewMode::Compact => ViewMode::Details,
                ViewMode::Details => ViewMode::Icons,
            };
            app.set_view(next_view);
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
                enter_crumbs(app, config::NAV_BUTTONS.len() - 1);
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
            let new_tab_dir = app.pane().cwd.clone();
            app.new_tab(new_tab_dir);
        }
        Action::CloseTab => app.close_tab(),
        Action::NextTab => app.cycle_tab(count as isize),
        Action::PrevTab => app.cycle_tab(-(count as isize)),

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
    let selected_paths = app.pane().selected_paths();
    match selected_paths.len() {
        0 => {}
        1 => {
            let p = selected_paths[0].clone();
            enter_text(app, Mode::Rename(p.clone()), ops::file_name_of(&p));
        }
        _ => enter_text(app, Mode::BatchRename, "Item #".into()),
    }
}

fn paste_clipboard(app: &mut App) {
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
    let transfer_kind = if cut {
        ops::TransferKind::Move
    } else {
        ops::TransferKind::Copy
    };
    app.transfer_progress = Some(ops::start_transfer(paths, dest, transfer_kind));
    if cut {
        app.clipboard.paths.clear();
    }
}

fn search_step(app: &mut App, search_direction: isize) {
    if app.search_last.is_empty() {
        return;
    }
    let needle = app.search_last.to_lowercase();
    let visible_count = app.pane().len();
    if visible_count == 0 {
        return;
    }
    let start = app.pane().cursor as isize;
    for steps_tried in 1..=visible_count as isize {
        let match_index =
            (start + search_direction * steps_tried).rem_euclid(visible_count as isize) as usize;
        if app
            .pane()
            .entry_at(match_index)
            .is_some_and(|e| e.name.to_lowercase().contains(&needle))
        {
            app.pane_mut().cursor = match_index;
            return;
        }
    }
    app.info(format!("Pattern not found: {}", app.search_last));
}

// ---------------------------------------------------------------------------
// Text entry
// ---------------------------------------------------------------------------

fn handle_text_key(app: &mut App, key_event: KeyEvent) {
    match key_event.code {
        KeyCode::Esc => {
            if app.mode == Mode::Filter {
                app.pane_mut().filter.clear();
                app.pane_mut().refilter();
                app.filter_bar = false;
            }
            app.mode = Mode::Normal;
            app.input.clear();
        }
        KeyCode::Enter => commit_text_input(app),
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
        KeyCode::Char('u') if key_event.modifiers == KeyModifiers::CONTROL => {
            app.input.clear();
            app.input_cursor = 0;
            live_update(app);
        }
        KeyCode::Char('w') if key_event.modifiers == KeyModifiers::CONTROL => {
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
    let input_path = PathBuf::from(&app.input);
    let (parent_dir, name_prefix) = if app.input.ends_with('/') {
        (input_path.clone(), String::new())
    } else {
        (
            input_path
                .parent()
                .map(|parent| parent.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("/")),
            ops::file_name_of(&input_path),
        )
    };
    let Ok(read_dir) = std::fs::read_dir(&parent_dir) else {
        return;
    };
    let mut matching_dirs: Vec<String> = read_dir
        .flatten()
        .filter(|dir_entry| dir_entry.path().is_dir())
        .map(|dir_entry| dir_entry.file_name().to_string_lossy().into_owned())
        .filter(|dir_name| dir_name.starts_with(&name_prefix))
        .collect();
    matching_dirs.sort();
    if let Some(best_match) = matching_dirs.first() {
        app.input = parent_dir.join(best_match).to_string_lossy().into_owned();
        app.input_cursor = app.input.chars().count();
    }
}

fn commit_text_input(app: &mut App) {
    let input = app.input.clone();
    let mode = app.mode.clone();
    app.mode = Mode::Normal;
    app.input.clear();
    app.input_cursor = 0;

    match mode {
        Mode::Command => run_ex_command(app, &input),
        Mode::Search => {
            app.search_last = input;
        }
        Mode::Filter => {
            // Enter keeps the filter and leaves the bar showing, like Dolphin.
            app.pane_mut().filter = input;
            app.pane_mut().refilter();
        }
        Mode::PathEdit => {
            let p = expand_tilde(&input);
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
            let rename_paths = app.pane().selected_paths();
            match ops::batch_rename(&rename_paths, &input) {
                Ok(op) => {
                    app.undo.push(op);
                    app.pane_mut().selected.clear();
                    app.refresh_in_place();
                    app.info(format!("Renamed {} item(s)", rename_paths.len()));
                }
                Err(e) => app.error(e),
            }
        }
        Mode::NewFolder => {
            let cwd = app.pane().cwd.clone();
            match ops::new_folder(&cwd, &input) {
                Ok(op) => {
                    app.undo.push(op);
                    app.refresh_in_place();
                    app.select_by_path(&cwd.join(&input));
                    app.info(format!("Created {input}"));
                }
                Err(e) => app.error(e),
            }
        }
        Mode::NewFile => {
            let cwd = app.pane().cwd.clone();
            match ops::new_file(&cwd, &input) {
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

fn expand_tilde(s: &str) -> PathBuf {
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
fn run_ex_command(app: &mut App, line: &str) {
    let line = line.trim();
    let (cmd, arg) = match line.split_once(char::is_whitespace) {
        Some((command_word, argument)) => (command_word, argument.trim()),
        None => (line, ""),
    };
    match cmd {
        "q" => app.close_tab(),
        "qa" | "qall" | "quit" => app.quit = true,
        "e" | "edit" | "cd" => {
            let p = if arg.is_empty() {
                places::home()
            } else {
                expand_tilde(arg)
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
            let new_tab_dir = if arg.is_empty() {
                app.pane().cwd.clone()
            } else {
                expand_tilde(arg)
            };
            app.new_tab(new_tab_dir);
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

fn handle_confirm_key(app: &mut App, key_event: KeyEvent, pending_confirm: Confirm) {
    let yes = matches!(key_event.code, KeyCode::Char('y' | 'Y') | KeyCode::Enter);
    app.mode = Mode::Normal;
    if !yes {
        app.info("Cancelled");
        return;
    }
    match pending_confirm {
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
    app.menu_cursor = match kind {
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
/// One row of a menu: what it reads as, and what it does.
pub struct MenuItem {
    pub label: &'static str,
    pub action: Action,
}

const fn menu_item(label: &'static str, action: Action) -> MenuItem {
    MenuItem { label, action }
}

pub fn menu_items(kind: &MenuKind) -> Vec<MenuItem> {
    match kind {
        MenuKind::Hamburger => vec![
            menu_item("New Folder…               F10", Action::NewFolder),
            menu_item("New File…                   o", Action::NewFile),
            menu_item("Rename…                    F2", Action::Rename),
            menu_item("Move to Trash           x/Del", Action::Trash),
            menu_item("Delete              Shift+Del", Action::DeletePerm),
            menu_item("Cut                    Ctrl+X", Action::Cut),
            menu_item("Copy                   Ctrl+C", Action::Copy),
            menu_item("Paste                  Ctrl+V", Action::Paste),
            menu_item("Compress                     ", Action::Compress),
            menu_item("Restore from Trash           ", Action::Restore),
            menu_item("Empty Trash                  ", Action::EmptyTrash),
            menu_item("Extract here                 ", Action::Extract),
            menu_item("Select All             Ctrl+A", Action::SelectAll),
            menu_item("Show Hidden Files           H", Action::ToggleHidden),
            menu_item("Filter                 Ctrl+I", Action::ToggleFilterBar),
            menu_item("Sort by…                     ", Action::OpenSortMenu),
            menu_item("View mode…                   ", Action::OpenViewMenu),
            menu_item("Split View                 F3", Action::ToggleSplit),
            menu_item("Places Panel               F9", Action::TogglePlaces),
            menu_item("Information Panel         F11", Action::ToggleInfo),
            menu_item("Open Terminal Here         F4", Action::TerminalHere),
            menu_item("Properties          Alt+Enter", Action::Properties),
            menu_item("Help                       F1", Action::Help),
            menu_item("Quit                   Ctrl+Q", Action::QuitAll),
        ],
        MenuKind::ViewMode => vec![
            menu_item("Icons                  Ctrl+1", Action::ViewIcons),
            menu_item("Compact                Ctrl+2", Action::ViewCompact),
            menu_item("Details                Ctrl+3", Action::ViewDetails),
        ],
        MenuKind::Sort => vec![
            menu_item("Name", Action::SortName),
            menu_item("Size", Action::SortSize),
            menu_item("Modified", Action::SortDate),
            menu_item("Type", Action::SortType),
            menu_item("Reverse order", Action::ToggleSortOrder),
            menu_item("Folders first", Action::ToggleDirsFirst),
        ],
    }
}

fn handle_menu_key(app: &mut App, key_event: KeyEvent, kind: MenuKind) {
    let items = menu_items(&kind);
    // A menu hanging off the toolbar is also a place in that row, so the row's
    // own motions work from inside it. That takes `h` and `l`, which is why
    // accepting is Enter and not `l`: a motion that sometimes acts is the
    // ambiguity the breadcrumb already refuses.
    if let Some(i) = menu_owner(&kind) {
        if toolbar_nav(app, key_event, i) {
            return;
        }
    }
    let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
    let n = items.len();
    let accept_menu_item = |app: &mut App| {
        let action = items[app.menu_cursor].action;
        leave_toolbar(app);
        run_action(app, action, 1);
    };
    match key_event.code {
        KeyCode::Esc   | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Char('n') if ctrl          => app.menu_cursor = (app.menu_cursor + 1) % n,
        KeyCode::Char('p') if ctrl          => app.menu_cursor = (app.menu_cursor + n - 1) % n,
        KeyCode::Char('y') if ctrl          => accept_menu_item(app),
        KeyCode::Down  | KeyCode::Char('j') => app.menu_cursor = (app.menu_cursor + 1) % n,
        KeyCode::Up    | KeyCode::Char('k') => app.menu_cursor = (app.menu_cursor + n - 1) % n,
        KeyCode::Home  | KeyCode::Char('g') => app.menu_cursor = 0,
        KeyCode::End   | KeyCode::Char('G') => app.menu_cursor = n - 1,
        KeyCode::Enter | KeyCode::Tab       => accept_menu_item(app),
        _ => {}
    }
}

/// The dropdown Dolphin hangs off a breadcrumb segment: the siblings of the
/// next path component, so you can hop sideways without going up first.
pub fn crumb_siblings(app: &App, segment_index: usize) -> Vec<PathBuf> {
    let comps: Vec<PathBuf> = crate::ui::crumb_paths(&app.pane().cwd);
    let Some(dir) = comps.get(segment_index) else {
        return Vec::new();
    };
    let mut sibling_dirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .filter(|p| app.pane().show_hidden || !ops::file_name_of(p).starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    sibling_dirs.sort();
    sibling_dirs
}

/// The rightmost trail segment with subdirectories to list. Only the segment
/// you are standing in can be childless — every one to its left holds at least
/// the path you came through — so in a real directory this always finds one.
/// A virtual place like `trash:/` has no trail at all, hence the `None`.
fn openable_crumb(app: &App) -> Option<usize> {
    let mut segment_index = crate::ui::crumb_paths(&app.pane().cwd)
        .len()
        .saturating_sub(1);
    loop {
        if !crumb_siblings(app, segment_index).is_empty() {
            return Some(segment_index);
        }
        segment_index = segment_index.checked_sub(1)?;
    }
}

/// Open segment `segment_index`, remembering it as where the breadcrumb was left.
///
/// The row is the one left highlighted last time this same segment was open.
/// Failing that — a first visit, or a pick that is no longer there — it is the
/// child you are standing in, the one row already in force, as in `open_menu`.
pub fn open_crumb(app: &mut App, segment_index: usize) {
    let trail = crate::ui::crumb_paths(&app.pane().cwd);
    let sibling_dirs = crumb_siblings(app, segment_index);
    let returning = app.pane().crumb_focus == trail.get(segment_index).cloned();
    let pick = app.pane().crumb_pick.clone().filter(|_| returning);
    app.menu_cursor = pick
        .or_else(|| trail.get(segment_index + 1).cloned())
        .and_then(|p| sibling_dirs.iter().position(|s| *s == p))
        .unwrap_or(0);
    app.pane_mut().crumb_focus = trail.get(segment_index).cloned();
    app.pane_mut().crumb_pick = sibling_dirs.get(app.menu_cursor).cloned();
    app.mode = Mode::CrumbMenu(segment_index);
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
    let segment_index = remembered
        .filter(|&s| !crumb_siblings(app, s).is_empty())
        .or_else(|| openable_crumb(app));
    match segment_index {
        Some(segment_index) => open_crumb(app, segment_index),
        None => focus_button(app, fallback),
    }
}

/// The toolbar buttons, driven from the keyboard. The row is three panes side
/// by side — nav group, breadcrumb, right group — so Ctrl+h and Ctrl+l cross
/// between them while bare h and l walk the buttons within one.
fn handle_buttons_key(app: &mut App, key_event: KeyEvent, button_index: usize) {
    if toolbar_nav(app, key_event, button_index) {
        return;
    }
    match key_event.code {
        KeyCode::Char('y') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
            press_button(app, button_index)
        }
        KeyCode::Enter | KeyCode::Tab => press_button(app, button_index),
        _ => {}
    }
}

/// The toolbar row as one index space: nav group first, then the right group.
/// Focus, hit-testing and the drawing order all count buttons this way. The
/// group boundary is `config::NAV_BUTTONS.len()`, so a button moved between the
/// two tables moves the boundary with it.
pub fn toolbar_buttons() -> impl Iterator<Item = Action> {
    config::NAV_BUTTONS
        .iter()
        .chain(config::RIGHT_BUTTONS)
        .copied()
}

pub fn toolbar_button(index: usize) -> Option<Action> {
    toolbar_buttons().nth(index)
}

pub fn toolbar_button_count() -> usize {
    config::NAV_BUTTONS.len() + config::RIGHT_BUTTONS.len()
}

/// Which menus hang off a toolbar button, and which button drops each. Both
/// directions are read from this one table: a menu's owning button and a
/// button's menu are the same fact asked from opposite ends. `MenuKind::Sort`
/// is absent because no button drops it — it opens from inside the hamburger.
pub const MENU_BUTTONS: &[(MenuKind, Action)] = &[
    (MenuKind::ViewMode, Action::OpenViewMenu),
    (MenuKind::Hamburger, Action::OpenMenu),
];

/// The menu a toolbar button drops, if it drops one.
fn button_menu(button_index: usize) -> Option<MenuKind> {
    let button_action = toolbar_button(button_index)?;
    MENU_BUTTONS
        .iter()
        .find(|(_, owning_action)| *owning_action == button_action)
        .map(|(kind, _)| kind.clone())
}

/// The button a menu hangs from. `Mode::Menu` carries no index because the same
/// menu opens from `m` and from the right button too, so the owner is derived
/// rather than stored — there is nothing to keep in step.
pub fn menu_owner(kind: &MenuKind) -> Option<usize> {
    let owning_action = MENU_BUTTONS
        .iter()
        .find(|(menu_kind, _)| menu_kind == kind)
        .map(|(_, owning_action)| *owning_action)?;
    toolbar_buttons().position(|toolbar_action| toolbar_action == owning_action)
}

/// Put focus on toolbar button `i`. A button that drops a menu shows it on
/// arrival, so the row reads like the breadcrumb, where landing on a segment
/// *is* opening its dropdown. `Mode::Menu` is then both "the menu is open" and
/// "focus is on its button", which is why there is no flag for either.
fn focus_button(app: &mut App, button_index: usize) {
    match button_menu(button_index) {
        Some(kind) => open_menu(app, kind),
        None => {
            app.menu_cursor = 0;
            app.mode = Mode::Buttons(button_index);
        }
    }
}

/// The keys that move focus around the toolbar row, for a focus resting on
/// button `i`. Shared by a bare button and by a menu hanging off one: they are
/// the same position in the row. True when the key was consumed.
fn toolbar_nav(app: &mut App, key_event: KeyEvent, button_index: usize) -> bool {
    let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
    let in_nav_group = button_index < config::NAV_BUTTONS.len();
    let (first_button, last_button) = if in_nav_group {
        (0, config::NAV_BUTTONS.len() - 1)
    } else {
        (config::NAV_BUTTONS.len(), toolbar_button_count() - 1)
    };
    match key_event.code {
        KeyCode::Char('j') if ctrl => app.mode = Mode::Normal,
        KeyCode::Char('k') if ctrl => {}
        // Crossing lands on the trail, or steps straight over it to the other
        // group when the place has no trail to open.
        KeyCode::Char('h') if ctrl => {
            if !in_nav_group {
                enter_crumbs(app, config::NAV_BUTTONS.len() - 1)
            }
        }
        KeyCode::Char('l') if ctrl => {
            if in_nav_group {
                enter_crumbs(app, config::NAV_BUTTONS.len())
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        KeyCode::Left | KeyCode::Char('h') => {
            focus_button(app, button_index.saturating_sub(1).max(first_button))
        }
        KeyCode::Right | KeyCode::Char('l') => {
            focus_button(app, (button_index + 1).min(last_button))
        }
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
fn press_button(app: &mut App, button_index: usize) {
    let Some(a) = toolbar_button(button_index) else {
        return;
    };
    leave_toolbar(app);
    run_action(app, a, 1);
}

/// The breadcrumb, driven from the keyboard. `Mode::CrumbMenu(segment_index)` is both
/// "focus is up in the breadcrumb" and "segment `segment_index` has its menu open" —
/// there is no third state where the crumb has focus with everything shut, so
/// there is no flag for one.
fn handle_crumb_menu_key(app: &mut App, key_event: KeyEvent, segment_index: usize) {
    let items = crumb_siblings(app, segment_index);
    if items.is_empty() {
        app.mode = Mode::Normal;
        return;
    }
    let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
    let last_segment_index = crate::ui::crumb_paths(&app.pane().cwd)
        .len()
        .saturating_sub(1);
    match key_event.code {
        // Ctrl+j is the way back down, the mirror of the Ctrl+k that came up.
        // Ctrl+h and Ctrl+l are pane motions and the toolbar row holds three
        // panes, so they step out to the button group on either side, landing
        // on the button nearest the trail.
        KeyCode::Char('j') if ctrl => app.mode = Mode::Normal,
        KeyCode::Char('k') if ctrl => {}
        KeyCode::Char('h') if ctrl => focus_button(app, config::NAV_BUTTONS.len() - 1),
        KeyCode::Char('l') if ctrl => focus_button(app, config::NAV_BUTTONS.len()),
        KeyCode::Char('n') if ctrl => app.menu_cursor = (app.menu_cursor + 1) % items.len(),
        KeyCode::Char('p') if ctrl => {
            app.menu_cursor = (app.menu_cursor + items.len() - 1) % items.len()
        }
        KeyCode::Char('y') if ctrl => accept_crumb(app, &items),
        KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
        // Inside the pane, bare motions move: h/l along the trail, j/k down the
        // menu. `l` does not enter a directory — that is what accept is for,
        // and a motion key that sometimes navigates is the ambiguity to avoid.
        KeyCode::Left | KeyCode::Char('h') => open_crumb(app, segment_index.saturating_sub(1)),
        KeyCode::Right | KeyCode::Char('l') => {
            open_crumb(app, (segment_index + 1).min(last_segment_index))
        }
        KeyCode::Down | KeyCode::Char('j') => app.menu_cursor = (app.menu_cursor + 1) % items.len(),
        KeyCode::Up | KeyCode::Char('k') => {
            app.menu_cursor = (app.menu_cursor + items.len() - 1) % items.len()
        }
        KeyCode::Enter | KeyCode::Tab => accept_crumb(app, &items),
        _ => {}
    }
    // Record where the row ended up, so leaving and coming back lands on it.
    // Only while this same segment is still the open one: `open_crumb` has already
    // set the pair for the segment it moved to, and leaving the trail entirely
    // must not overwrite the pick with a row from a menu that is now shut.
    if app.mode == Mode::CrumbMenu(segment_index) {
        app.pane_mut().crumb_pick = items.get(app.menu_cursor).cloned();
    }
}

fn accept_crumb(app: &mut App, items: &[PathBuf]) {
    let Some(crumb_dir) = items.get(app.menu_cursor).cloned() else {
        return;
    };
    leave_toolbar(app);
    app.goto(Target::Dir(crumb_dir), true);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;

    fn test_app() -> App {
        App::new(std::env::temp_dir())
    }

    fn press_char(app: &mut App, key_char: char) {
        handle_key_event(
            app,
            KeyEvent::new(KeyCode::Char(key_char), KeyModifiers::NONE),
        );
    }

    #[test]
    fn digits_accumulate_into_a_count() {
        let mut app = test_app();
        press_char(&mut app, '1');
        press_char(&mut app, '2');
        assert_eq!(app.count, "12");
    }

    #[test]
    fn a_bare_zero_is_a_motion_not_a_count() {
        let mut app = test_app();
        press_char(&mut app, '0');
        assert!(app.count.is_empty());
    }

    #[test]
    fn gg_completes_as_a_chord() {
        let mut app = test_app();
        press_char(&mut app, 'g');
        assert_eq!(app.pending_chord_leader, Some('g'));
        press_char(&mut app, 'g');
        assert_eq!(app.pending_chord_leader, None);
        assert_eq!(app.pane().cursor, 0);
    }

    #[test]
    fn colon_enters_command_mode_and_esc_leaves_it() {
        let mut app = test_app();
        press_char(&mut app, ':');
        assert_eq!(app.mode, Mode::Command);
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn unknown_commands_report_rather_than_panic() {
        let mut app = test_app();
        run_ex_command(&mut app, "nonsense");
        assert!(app.status_is_error);
    }

    #[test]
    fn view_command_switches_mode() {
        let mut app = test_app();
        run_ex_command(&mut app, "view details");
        assert_eq!(app.pane().view, ViewMode::Details);
    }

    #[test]
    fn tilde_expands_to_home() {
        assert_eq!(expand_tilde("~"), places::home());
        assert_eq!(expand_tilde("~/x"), places::home().join("x"));
    }

    /// `dd` is n lines counting this one; `dj` is n *more* than this one.
    #[test]
    fn dd_and_dj_differ_by_one_line() {
        let (cursor, last) = (0, 99);
        assert_eq!(delete_range(KeyCode::Char('d'), cursor, last, 3), Some((0, 2)));
        assert_eq!(delete_range(KeyCode::Char('j'), cursor, last, 3), Some((0, 3)));
    }

    #[test]
    fn ranges_stop_at_the_ends_of_the_pane() {
        assert_eq!(delete_range(KeyCode::Char('d'), 8, 9, 50), Some((8, 9)));
        assert_eq!(delete_range(KeyCode::Char('j'), 8, 9, 50), Some((8, 9)));
        assert_eq!(delete_range(KeyCode::Char('k'), 2, 9, 50), Some((0, 2)));
        assert_eq!(delete_range(KeyCode::Char('G'), 4, 9, 1), Some((4, 9)));
    }

    /// A count is whatever was typed at it, so the arithmetic has to survive
    /// numbers no pane could hold.
    #[test]
    fn an_absurd_count_does_not_overflow() {
        let huge = usize::MAX;
        assert_eq!(delete_range(KeyCode::Char('d'), 5, 9, huge), Some((5, 9)));
        assert_eq!(delete_range(KeyCode::Char('k'), 5, 9, huge), Some((0, 5)));
    }

    #[test]
    fn a_key_that_is_not_a_motion_cancels_the_operator() {
        assert_eq!(delete_range(KeyCode::Char('w'), 0, 9, 1), None);
    }

    /// `d` then Ctrl+d is a half page down, not a delete — and not swallowed
    /// either: the operator cancels and the key goes on being itself.
    #[test]
    fn ctrl_d_after_d_cancels_and_is_re_dispatched() {
        let mut app = test_app();
        press_char(&mut app, 'd');
        assert_eq!(app.pending_delete, Some(1));
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.pending_delete, None);
    }

    /// A cancelled operator has spent nothing, so the count typed after the
    /// `d` is still standing for whatever the key turns out to mean.
    #[test]
    fn a_cancelled_operator_leaves_the_count_alone() {
        let mut app = test_app();
        press_char(&mut app, 'd');
        press_char(&mut app, '3');
        // `w` is not a motion `d` can use, and nothing else claims it either.
        press_char(&mut app, 'w');
        assert_eq!(app.pending_delete, None);
        assert_eq!(app.count, "3");
    }

    /// The shift is in the character, so a terminal that reports it as well
    /// and one that does not must reach the same binding.
    #[test]
    fn shifted_printables_match_with_or_without_the_modifier() {
        let with = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        let without = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE);
        assert_eq!(lookup_binding(normalize(with)), Some(Action::Bottom));
        assert_eq!(lookup_binding(normalize(without)), Some(Action::Bottom));
    }

    /// Ctrl+Shift+Tab under the kitty protocol, and the bare `CSI Z` a legacy
    /// terminal sends for the same chord: both are the previous tab.
    #[test]
    fn shift_tab_reaches_prev_tab_under_either_encoding() {
        for mods in [
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
            KeyModifiers::SHIFT,
        ] {
            let ev = KeyEvent::new(KeyCode::BackTab, mods);
            assert_eq!(lookup_binding(normalize(ev)), Some(Action::PrevTab));
        }
    }

    /// Shift+Delete is not a printable: its modifier is the whole difference
    /// between trashing and deleting, so normalization must leave it alone.
    #[test]
    fn shift_still_counts_on_a_named_key() {
        let ev = KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT);
        assert_eq!(lookup_binding(normalize(ev)), Some(Action::DeletePerm));
    }

    #[test]
    fn unbound_printable_keys_feed_typeahead() {
        let mut app = test_app();
        press_char(&mut app, 'q');
        assert_eq!(app.typeahead, "q");
    }
}
