//! The modal input engine, and the single place where an `Action` turns into
//! a state change.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{
    App, Confirm, CreationIntent, Direction, Focus, FocusRegion, MarkPending, MenuKind, Mode,
    ViewMode,
};
use crate::config;
use crate::fs::SortKey;
use crate::ops;
use crate::places::{self, Target};

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
        Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock => {
            handle_normal_key(app, key_event)
        }
        Mode::Confirm(c) => handle_confirm_key(app, key_event, c),
        Mode::Properties | Mode::Help => {
            if lookup_binding(app, key_event) == Some(Action::Cancel) {
                app.mode = Mode::Normal;
            }
        }
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
    // Places and Tabs are complete local input owners. They never enter the
    // file view's count/operator/chord/typeahead pipeline.
    if app.focus != Focus::View {
        if let Some(action) = lookup_binding(app, key_event) {
            run_action(app, action, 1);
        }
        return;
    }

    if key_event.code == KeyCode::Esc {
        app.count.clear();
        app.pending_chord_leader = None;
        app.pending_delete = None;
        app.pending_mark = None;
        // Esc ends the visual and takes the range with it: the selection
        // belonged to the drag, not to the pane. Outside a visual it is the
        // key that clears whatever selection stands.
        if app.mode.is_visual() {
            app.leave_visual();
        } else {
            app.pane_mut().selected.clear();
        }
        return;
    }

    // A pending chord leader owns the next key, whatever it is.
    if let Some(pending_leader) = app.pending_chord_leader.take() {
        if let KeyCode::Char(c) = key_event.code {
            if key_event.modifiers.is_empty() {
                if let Some(chord) = config::CHORDS.iter().find(|chord| {
                    chord.leader == pending_leader
                        && chord.follower == c
                        && chord.modes.iter().any(|mode| mode.matches(&app.mode))
                }) {
                    let n = take_count(app);
                    run_action(app, chord.action, n);
                    return;
                }
            }
        }
        return;
    }

    // A pending mark owns the next key too, and for the same reason: the letter
    // names the mark and can be any letter, so nothing else may read it.
    if let Some(pending_mark) = app.pending_mark.take() {
        if let KeyCode::Char(c) = key_event.code {
            if !key_event
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                mark_key(app, pending_mark, c);
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

    if let KeyCode::Char(c) = key_event.code {
        if key_event.modifiers.is_empty() && is_chord_leader(&app.mode, c) {
            // `c` is only a leader when a follower can complete it; `cw` is the
            // one chord it starts, so a lone `c` simply waits.
            app.pending_chord_leader = Some(c);
            return;
        }
    }

    if let Some(a) = lookup_binding(app, key_event) {
        let n = take_count(app);
        run_action(app, a, n);
        return;
    }

    // Type-ahead belongs to Normal mode. In a visual mode an unknown key must
    // not refilter the listing underneath the path-based visual range.
    if app.mode == Mode::Normal {
        if let KeyCode::Char(c) = key_event.code {
            if key_event.modifiers.is_empty() {
                app.typeahead(c);
            }
        }
    }
}

/// Write or read one mark. A mark remembers a pane's target rather than its
/// path, so `'t` returns to the Trash or to a saved search as readily as to a
/// folder — and a jump to a letter never written says so rather than doing
/// nothing, since a silent no-op is indistinguishable from a mistyped letter.
fn mark_key(app: &mut App, pending_mark: MarkPending, letter: char) {
    match pending_mark {
        MarkPending::Set => {
            let target = app.pane().target.clone();
            app.marks.insert(letter, target);
            app.info(format!("Marked '{letter}"));
        }
        MarkPending::Jump => match app.marks.get(&letter).cloned() {
            Some(target) => app.goto(target, true),
            None => app.error(format!("No mark '{letter}")),
        },
    }
}

/// A leader is a key some chord starts with — read off `config::CHORDS` rather
/// than listed beside it, so a new leader cannot be half-added. Eight rows: the
/// scan costs nothing, and a second spelling costs a chord that never fires.
fn is_chord_leader(mode: &Mode, c: char) -> bool {
    config::CHORDS
        .iter()
        .any(|chord| chord.leader == c && chord.modes.iter().any(|m| m.matches(mode)))
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
fn delete_range(code: KeyCode, cursor: usize, last: usize, count: usize) -> Option<(usize, usize)> {
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
fn delete_motion(app: &mut App, key_event: KeyEvent, count_before_operator: usize) -> MotionResult {
    // Ctrl+d is half-page down, not `dd`; only a bare motion completes the
    // operator. Modifiers arrive normalized, so bare is literally none.
    if !key_event.modifiers.is_empty() {
        return MotionResult::Cancelled;
    }
    let total_count = count_before_operator.saturating_mul(peek_count(app));
    let cursor = app.pane().cursor;
    let last = app.pane().len().saturating_sub(1);
    let Some((range_start, range_end)) = delete_range(key_event.code, cursor, last, total_count)
    else {
        return MotionResult::Cancelled;
    };
    app.count.clear();
    let range_paths = app.pane().paths_in(range_start, range_end);
    delete_to_register(app, range_paths);
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

/// Write live paths to the unnamed register and desktop clipboard through one
/// transaction. An empty request is rejected so a failed yank cannot erase an
/// existing register. A visual range is consumed only after the write commits.
fn write_live_register(app: &mut App, cut: bool, verb: &str, empty_message: &str) {
    let paths = app.pane().selected_paths();
    if paths.is_empty() {
        app.error(empty_message);
        return;
    }
    let count = paths.len();
    app.register.set(paths, cut);
    app.info(format!("{verb} {count} item(s)"));
    app.leave_visual();
}

/// The one path to the Trash. Every delete key ends here so that the undo
/// entry, the message and the refresh cannot drift apart.
fn move_to_trash(app: &mut App, paths: Vec<PathBuf>) -> Vec<ops::TrashRef> {
    if paths.is_empty() {
        return Vec::new();
    }
    // In the Trash there is no further "away" to move something to, so `x`
    // means purge, as it does in Dolphin. It goes behind the Shift+Del
    // confirmation: this is the one place the key is not undoable.
    if app.pane().target == Target::Trash {
        let items = app.pane().selected_trash_refs();
        if !items.is_empty() {
            app.mode = Mode::Confirm(Confirm::PurgeFromTrash(items));
        }
        return Vec::new();
    }

    let outcome = ops::trash(&paths);
    if outcome.committed.is_empty() {
        let message = outcome
            .failed
            .first()
            .map(|failure| failure.message.clone())
            .unwrap_or_else(|| "Nothing moved to Trash".into());
        app.error(message);
        return Vec::new();
    }

    let committed_paths: std::collections::HashSet<_> = outcome
        .committed
        .iter()
        .map(|item| item.original_path.clone())
        .collect();
    app.undo.push(ops::UndoOp::Trash {
        items: outcome.committed.clone(),
    });
    app.pane_mut()
        .selected
        .retain(|path| !committed_paths.contains(path));
    app.refresh_in_place();
    if outcome.is_complete() {
        app.info(format!(
            "Moved {} item(s) to Trash",
            outcome.committed.len()
        ));
    } else {
        app.error(format!(
            "Moved {} item(s) to Trash; {} failed: {}",
            outcome.committed.len(),
            outcome.failed.len(),
            outcome.failed[0].message
        ));
    }
    app.leave_visual();
    outcome.committed
}

/// Vim's `d` is both a removal and a write to the unnamed register. The
/// register is derived from committed effects, never from requested operands.
fn delete_to_register(app: &mut App, paths: Vec<PathBuf>) {
    let deleted = move_to_trash(app, paths);
    if !deleted.is_empty() {
        app.register.set_deleted(deleted);
    }
}

/// `v` and `V` start a range at the cursor. Pressing the key you are already in
/// leaves visual, as in vim; pressing the other switches between charwise and
/// linewise without disturbing the anchor.
fn enter_visual(app: &mut App, target_mode: Mode) {
    if app.mode == target_mode {
        app.leave_visual();
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

/// Resolve keyboard focus exactly once from body focus plus toolbar mode.
pub fn current_focus_region(app: &App) -> FocusRegion {
    match &app.mode {
        Mode::CrumbMenu(_) => FocusRegion::Breadcrumb,
        Mode::Buttons(i) => button_region(*i),
        Mode::Menu(kind) => menu_owner(kind)
            .map(button_region)
            .unwrap_or(FocusRegion::ToolbarRight),
        _ => match app.focus {
            Focus::Places => FocusRegion::Places,
            Focus::Tabs => FocusRegion::Tabs,
            Focus::View => FocusRegion::View(app.tab().active),
        },
    }
}

fn button_region(index: usize) -> FocusRegion {
    if index < config::NAV_BUTTONS.len() {
        FocusRegion::ToolbarNav
    } else {
        FocusRegion::ToolbarRight
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusTransition {
    Stay,
    Move(FocusRegion),
}

impl FocusRegion {
    /// The complete directional-neighbour policy for this region.
    pub fn move_focus(self, direction: Direction, app: &App) -> FocusTransition {
        use Direction::{Down, Left, Right, Up};
        use FocusRegion::{Breadcrumb, Places, Tabs, ToolbarNav, ToolbarRight, View};
        let target = match (self, direction) {
            (Places, Right) => Some(View(0)),
            (Places, Up) => Some(ToolbarNav),
            (View(0), Left) if app.places_visible => Some(Places),
            (View(0), Right) if app.split_on() => Some(View(1)),
            (View(0 | 1), Up) if app.tabs.len() > 1 => Some(Tabs),
            (View(0 | 1), Up) => Some(Breadcrumb),
            (View(1), Left) => Some(View(0)),
            (Tabs, Left) => Some(View(0)),
            (Tabs, Right) => Some(View(usize::from(app.split_on()))),
            (Tabs, Up) => Some(Breadcrumb),
            (Tabs, Down) => Some(View(app.tab().active)),
            (ToolbarNav, Right) => Some(Breadcrumb),
            (ToolbarNav, Down) | (Breadcrumb, Down) | (ToolbarRight, Down) => {
                Some(app.toolbar_return)
            }
            (Breadcrumb, Left) => Some(ToolbarNav),
            (Breadcrumb, Right) => Some(ToolbarRight),
            (ToolbarRight, Left) => Some(Breadcrumb),
            _ => None,
        };
        target.map_or(FocusTransition::Stay, FocusTransition::Move)
    }
}

/// The only transition application path. Region entry helpers retain menu and
/// breadcrumb side effects; body entry repairs the active pane explicitly.
pub fn move_focus(app: &mut App, direction: Direction) {
    app.repair_focus();
    let from = current_focus_region(app);
    match from.move_focus(direction, app) {
        FocusTransition::Stay => {}
        FocusTransition::Move(to) => enter_focus_region(app, from, to),
    }
}

fn enter_focus_region(app: &mut App, from: FocusRegion, to: FocusRegion) {
    if to.is_toolbar() {
        if !from.is_toolbar() {
            app.toolbar_return = from;
        }
        match to {
            FocusRegion::ToolbarNav => focus_button(app, 0),
            FocusRegion::Breadcrumb => {
                let fallback = if from == FocusRegion::ToolbarRight {
                    config::NAV_BUTTONS.len()
                } else {
                    config::NAV_BUTTONS.len() - 1
                };
                enter_crumbs(app, fallback);
            }
            FocusRegion::ToolbarRight => focus_button(app, config::NAV_BUTTONS.len()),
            _ => unreachable!(),
        }
        return;
    }

    app.mode = Mode::Normal;
    match to {
        FocusRegion::Places => app.focus = Focus::Places,
        FocusRegion::Tabs => app.focus = Focus::Tabs,
        FocusRegion::View(index) => {
            app.focus = Focus::View;
            app.tab_mut().active = index.min(usize::from(app.split_on()));
        }
        _ => unreachable!(),
    }
}

fn cancel_toolbar(app: &mut App) {
    let from = current_focus_region(app);
    let to = app.toolbar_return;
    enter_focus_region(app, from, to);
}

/// Find the one row that names this key in the current mode. Modifiers match
/// exactly after normalization, so an unlisted combination never inherits a binding.
fn lookup_binding(app: &App, key_event: KeyEvent) -> Option<Action> {
    let find = |mode: BindMode| {
        config::KEY_BINDINGS.iter().find(|bind| {
            bind.code == key_event.code
                && normalize_mods(bind.code, bind.mods) == key_event.modifiers
                && bind.modes.contains(&mode)
        })
    };

    if matches!(
        app.mode,
        Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock
    ) {
        match app.focus {
            Focus::Places => return find(BindMode::Places).map(|binding| binding.action),
            Focus::Tabs => return find(BindMode::Tabs).map(|binding| binding.action),
            Focus::View => {}
        }
    }

    lookup_binding_for_mode(&app.mode, key_event)
}

fn lookup_binding_for_mode(mode: &Mode, key_event: KeyEvent) -> Option<Action> {
    config::KEY_BINDINGS
        .iter()
        .find(|bind| {
            bind.code == key_event.code
                && normalize_mods(bind.code, bind.mods) == key_event.modifiers
                && bind
                    .modes
                    .iter()
                    .any(|binding_mode| binding_mode.matches(mode))
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

fn move_places_cursor(app: &mut App, direction: isize) {
    let row_count = app.places.len() as isize;
    if row_count == 0 {
        return;
    }
    let mut row_index = app.places_cursor as isize;
    for _ in 0..row_count {
        row_index = (row_index + direction).rem_euclid(row_count);
        if app.places[row_index as usize].is_selectable() {
            break;
        }
    }
    app.places_cursor = row_index as usize;
}

fn open_place(app: &mut App, leave_panel: bool) {
    let target = app
        .places
        .get(app.places_cursor)
        .and_then(|place| place.target())
        .cloned();
    if let Some(target) = target {
        app.goto(target, true);
        if leave_panel {
            app.focus = Focus::View;
        }
    }
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
    /// `H`/`L`: out of the folder and into the one under the cursor, the pair
    /// Vimium spells the same way. Only the list views have them — Icons uses
    /// its horizontal axis for the grid.
    NavigateUp,
    NavigateInto,
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
    /// Unbound for now: Space, the key that used to say this, is the leader.
    #[allow(dead_code)]
    ToggleSelect,
    SelectAll,
    InvertSelect,
    EnterVisual,
    EnterVisualLine,
    EnterVisualBlock,
    /* file operations */
    Yank,
    Copy,
    Cut,
    /// `d` in Normal: trash a range once a motion says which.
    DeleteOp,
    /// `d` in a visual mode: trash the selected range immediately.
    DeleteSelection,
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
    /// Move keyboard focus between UI regions. Local movement uses the
    /// `Move*` and `Interface*` actions instead.
    Focus(Direction),
    /// Deliberately consume a binding in a focused keymap.
    NoOp,
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
    /* marks */
    /// `m`: the next key names a mark to write.
    SetMark,
    /// `'`: the next key names a mark to jump to.
    JumpMark,
    /* misc */
    TerminalPanel,
    TerminalHere,
    /// Unbound for now: `D` and `P` were the keys, and both mean something
    /// else in vim. The drag itself still works from the mouse.
    #[allow(dead_code)]
    DragOut,
    #[allow(dead_code)]
    DropIn,
    OpenMenu,
    OpenViewMenu,
    OpenSortMenu,
    Help,
    QuitAll,
    /* text-entry and interface actions */
    Cancel,
    CommitInput,
    InputBackspace,
    InputDelete,
    InputLeft,
    InputRight,
    InputHome,
    InputEnd,
    InputClear,
    InputDeleteWord,
    CompletePath,
    ConfirmAccept,
    InterfaceDown,
    InterfaceUp,
    InterfaceLeft,
    InterfaceRight,
    InterfaceFirst,
    InterfaceLast,
    InterfaceAccept,
    PlacesDown,
    PlacesUp,
    PlacesOpen,
    PlacesAccept,
}

/// A payload-free input context for the static keymap.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindMode {
    Normal,
    Visual,
    VisualLine,
    VisualBlock,
    /// Places panel focus.
    Places,
    /// Tab pane focus. Like Places, this refines the normal/visual modes.
    Tabs,
    Command,
    Search,
    Filter,
    PathEdit,
    Rename,
    BatchRename,
    NewFolder,
    NewFile,
    Confirm,
    Properties,
    Help,
    CrumbMenu,
    Buttons,
    Menu,
}

impl BindMode {
    /// Whether this configured mode describes the application's current mode.
    pub fn matches(self, mode: &Mode) -> bool {
        matches!(
            (self, mode),
            (Self::Normal, Mode::Normal)
                | (Self::Visual, Mode::Visual)
                | (Self::VisualLine, Mode::VisualLine)
                | (Self::VisualBlock, Mode::VisualBlock)
                | (Self::Command, Mode::Command)
                | (Self::Search, Mode::Search)
                | (Self::Filter, Mode::Filter)
                | (Self::PathEdit, Mode::PathEdit)
                | (Self::Rename, Mode::Rename(_))
                | (Self::BatchRename, Mode::BatchRename)
                | (Self::NewFolder, Mode::NewFolder(_))
                | (Self::NewFile, Mode::NewFile(_))
                | (Self::Confirm, Mode::Confirm(_))
                | (Self::Properties, Mode::Properties)
                | (Self::Help, Mode::Help)
                | (Self::CrumbMenu, Mode::CrumbMenu(_))
                | (Self::Buttons, Mode::Buttons(_))
                | (Self::Menu, Mode::Menu(_))
        )
    }
}

/// One row of the mode-aware keymap table.
pub struct Bind {
    pub mods: KeyModifiers,
    pub code: KeyCode,
    pub modes: &'static [BindMode],
    pub action: Action,
}

/// Terse constructor so every table row has the same four-column shape.
pub const fn bind(
    mods: KeyModifiers,
    code: KeyCode,
    modes: &'static [BindMode],
    action: Action,
) -> Bind {
    Bind {
        mods,
        code,
        modes,
        action,
    }
}

/// One row of the chord table.
pub struct Chord {
    pub leader: char,
    pub follower: char,
    pub modes: &'static [BindMode],
    pub action: Action,
}

/// Terse constructor so every chord row has the same four-column shape.
pub const fn chord(
    leader: char,
    follower: char,
    modes: &'static [BindMode],
    action: Action,
) -> Chord {
    Chord {
        leader,
        follower,
        modes,
        action,
    }
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
        // `H`/`L` walk the tree in the two list views. Icons spends its
        // horizontal axis on the grid, so there they mean nothing rather than
        // meaning something the row beside them contradicts.
        Action::NavigateUp | Action::NavigateInto => {
            if app.pane().view != ViewMode::Icons {
                if action == Action::NavigateUp {
                    app.go_up();
                } else {
                    app.activate();
                }
            }
        }
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
                // Details is one column wide, so there is nowhere sideways to
                // go. Walking the tree is `H`/`L`, the same keys as in Compact.
                ViewMode::Details => {}
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
        Action::EnterVisualBlock => {
            if app.pane().view == ViewMode::Icons {
                enter_visual(app, Mode::VisualBlock);
            } else {
                app.error("Visual block selection is only available in Icons mode");
            }
        }

        // file operations
        Action::Yank | Action::Copy | Action::Cut => {
            let cut = action == Action::Cut;
            let (verb, empty_message) = match action {
                Action::Yank => ("Yanked", "Nothing to yank"),
                Action::Copy => ("Copied", "Nothing to copy"),
                Action::Cut => ("Cut", "Nothing to cut"),
                _ => unreachable!(),
            };
            write_live_register(app, cut, verb, empty_message);
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
            if app.pane().selected.is_empty() {
                app.pending_delete = Some(count);
            } else {
                let trash_paths = app.pane().selected_paths();
                delete_to_register(app, trash_paths);
            }
        }
        Action::DeleteSelection => {
            let trash_paths = app.pane().selected_paths();
            delete_to_register(app, trash_paths);
        }
        Action::DeletePerm => {
            let perm_delete_paths = operand_paths(app, count);
            if !perm_delete_paths.is_empty() {
                app.mode = Mode::Confirm(if app.pane().target == Target::Trash {
                    Confirm::PurgeFromTrash(app.pane().selected_trash_refs())
                } else {
                    Confirm::DeletePermanently(perm_delete_paths)
                });
            }
        }
        Action::Rename => start_rename(app),
        Action::NewFolder => {
            if let Some(intent) = creation_intent(app, app.pane().cwd.clone()) {
                enter_text(app, Mode::NewFolder(intent), String::new());
            }
        }
        Action::NewFile => {
            let parent = app
                .pane()
                .current()
                .filter(|entry| entry.path.is_dir())
                .map_or_else(|| app.pane().cwd.clone(), |entry| entry.path.clone());
            if let Some(intent) = creation_intent(app, parent) {
                enter_text(app, Mode::NewFile(intent), String::new());
            }
        }
        Action::Undo => match app.undo.pop() {
            None => app.info("Nothing to undo"),
            Some(op) => match ops::undo(&op) {
                Ok(outcome) => {
                    if let Some(change) = outcome.register_change {
                        if app.register == change.expected {
                            app.register = change.replacement;
                        }
                    }
                    ops::rebase_trash_history(&mut app.undo, &outcome.trash_replacements);
                    app.info(outcome.message);
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
            let items = app.pane().selected_trash_refs();
            match ops::restore_from_trash(&items) {
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
        Action::Focus(direction) => move_focus(app, direction),
        Action::NoOp => {}
        Action::PlacesDown => move_places_cursor(app, 1),
        Action::PlacesUp => move_places_cursor(app, -1),
        Action::PlacesOpen => open_place(app, false),
        Action::PlacesAccept => open_place(app, true),
        Action::SwapPane => app.other_pane(),
        Action::TogglePlaces => {
            app.places_visible = !app.places_visible;
            app.repair_focus();
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

        // marks
        //
        // Both keys only arm themselves; `handle_normal_key` owns the letter
        // that follows, the way it owns a chord's follower.
        Action::SetMark => app.pending_mark = Some(MarkPending::Set),
        Action::JumpMark => app.pending_mark = Some(MarkPending::Jump),

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
        Action::Cancel
        | Action::CommitInput
        | Action::InputBackspace
        | Action::InputDelete
        | Action::InputLeft
        | Action::InputRight
        | Action::InputHome
        | Action::InputEnd
        | Action::InputClear
        | Action::InputDeleteWord
        | Action::CompletePath
        | Action::ConfirmAccept
        | Action::InterfaceDown
        | Action::InterfaceUp
        | Action::InterfaceLeft
        | Action::InterfaceRight
        | Action::InterfaceFirst
        | Action::InterfaceLast
        | Action::InterfaceAccept => {
            debug_assert!(false, "mode-local action reached normal dispatcher");
        }
    }
}

fn enter_text(app: &mut App, mode: Mode, seed: String) {
    app.mode = mode;
    app.input = seed;
    app.input_cursor = app.input.chars().count();
}

fn creation_intent(app: &mut App, directory: PathBuf) -> Option<CreationIntent> {
    if !matches!(app.pane().target, Target::Dir(_)) {
        app.error("Creation is unavailable in this location");
        return None;
    }
    let metadata = match std::fs::metadata(&directory) {
        Ok(metadata) if metadata.is_dir() => metadata,
        _ => {
            app.error(format!("Not a directory: {}", directory.display()));
            return None;
        }
    };
    if metadata.permissions().mode() & 0o222 == 0 {
        app.error(format!(
            "Directory is not writable: {}",
            directory.display()
        ));
        return None;
    }
    Some(CreationIntent {
        directory,
        pane_id: app.pane().id,
    })
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
    // Deleted generations use the same background completion pipeline as live
    // copies: collision allocation, progress, cancellation and partial effects.
    if let ops::UnnamedRegister::Deleted { items } = app.register.clone() {
        app.transfer_progress = Some(ops::start_restore(items, app.pane().cwd.clone()));
        return;
    }

    // Prefer our own clipboard: it knows cut-vs-copy, which uri-list cannot say.
    let (mut paths, cut) = match &app.register {
        ops::UnnamedRegister::Live { paths, cut } => (paths.clone(), *cut),
        ops::UnnamedRegister::Empty | ops::UnnamedRegister::Deleted { .. } => (Vec::new(), false),
    };
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
    let mut progress = ops::start_transfer(paths, dest, transfer_kind);
    progress.expected_register = Some(app.register.clone());
    app.transfer_progress = Some(progress);
    // The completion reducer removes only committed move sources from a cut
    // register; failed and cancelled sources stay retryable.
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
    match lookup_binding(app, key_event) {
        Some(Action::Cancel) => {
            if app.mode == Mode::Filter {
                app.pane_mut().filter.clear();
                app.pane_mut().refilter();
                app.filter_bar = false;
            }
            app.mode = Mode::Normal;
            app.input.clear();
        }
        Some(Action::CommitInput) => commit_text_input(app),
        Some(Action::InputBackspace) => {
            if app.input_cursor > 0 {
                let i = byte_at(&app.input, app.input_cursor - 1);
                app.input.remove(i);
                app.input_cursor -= 1;
                live_update(app);
            }
        }
        Some(Action::InputDelete) => {
            if app.input_cursor < app.input.chars().count() {
                let i = byte_at(&app.input, app.input_cursor);
                app.input.remove(i);
                live_update(app);
            }
        }
        Some(Action::InputLeft) => app.input_cursor = app.input_cursor.saturating_sub(1),
        Some(Action::InputRight) => {
            app.input_cursor = (app.input_cursor + 1).min(app.input.chars().count());
        }
        Some(Action::InputHome) => app.input_cursor = 0,
        Some(Action::InputEnd) => app.input_cursor = app.input.chars().count(),
        Some(Action::InputClear) => {
            app.input.clear();
            app.input_cursor = 0;
            live_update(app);
        }
        Some(Action::InputDeleteWord) => {
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
        Some(Action::CompletePath) => complete_path(app),
        None => {
            if key_event.modifiers.is_empty() {
                if let KeyCode::Char(c) = key_event.code {
                    let i = byte_at(&app.input, app.input_cursor);
                    app.input.insert(i, c);
                    app.input_cursor += 1;
                    live_update(app);
                }
            }
        }
        Some(_) => debug_assert!(false, "non-text action in a text-entry mode"),
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
        Mode::NewFolder(intent) => match ops::new_folder(&intent.directory, &input) {
            Ok(op) => {
                let created = intent.directory.join(&input);
                app.undo.push(op);
                app.reveal_created(intent, created);
                app.info(format!("Created {input}"));
            }
            Err(e) => app.error(e),
        },
        Mode::NewFile(intent) => match ops::new_file(&intent.directory, &input) {
            Ok(op) => {
                let created = intent.directory.join(&input);
                app.undo.push(op);
                app.reveal_created(intent, created);
                app.info(format!("Created {input}"));
            }
            Err(e) => app.error(e),
        },
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
    match lookup_binding(app, key_event) {
        Some(Action::ConfirmAccept) => app.mode = Mode::Normal,
        Some(Action::Cancel) => {
            app.mode = Mode::Normal;
            app.info("Cancelled");
            return;
        }
        _ => return,
    }
    match pending_confirm {
        Confirm::DeletePermanently(paths) => {
            let outcome = ops::delete_permanently(&paths);
            let committed: std::collections::HashSet<_> =
                outcome.committed.iter().cloned().collect();
            app.pane_mut()
                .selected
                .retain(|key| !committed.contains(key));
            if !outcome.committed.is_empty() {
                app.refresh_in_place();
            }
            if outcome.failed.is_empty() {
                app.info(format!(
                    "Deleted {} item(s) permanently",
                    outcome.committed.len()
                ));
            } else {
                app.error(format!(
                    "Deleted {} item(s); {} failed: {}",
                    outcome.committed.len(),
                    outcome.failed.len(),
                    outcome.failed[0].message
                ));
            }
        }
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
    let current = current_focus_region(app);
    if !current.is_toolbar() {
        app.toolbar_return = current;
    }
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
            menu_item("New Folder…                 O", Action::NewFolder),
            menu_item("New File…                   o", Action::NewFile),
            menu_item("Rename…                    F2", Action::Rename),
            menu_item("Move to Trash           x/Del", Action::Trash),
            menu_item("Delete              Shift+Del", Action::DeletePerm),
            menu_item("Cut                    Ctrl+X", Action::Cut),
            menu_item("Copy                   Ctrl+C", Action::Copy),
            menu_item("Paste                       p", Action::Paste),
            menu_item("Compress                     ", Action::Compress),
            menu_item("Restore from Trash           ", Action::Restore),
            menu_item("Empty Trash                  ", Action::EmptyTrash),
            menu_item("Extract here                 ", Action::Extract),
            menu_item("Select All             Ctrl+A", Action::SelectAll),
            menu_item("Show Hidden Files     <Space>h", Action::ToggleHidden),
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
    if let Some(i) = menu_owner(&kind) {
        if toolbar_nav(app, key_event, i) {
            return;
        }
    }
    let n = items.len();
    match lookup_binding(app, key_event) {
        Some(Action::Focus(direction)) => move_focus(app, direction),
        Some(Action::Cancel) => cancel_toolbar(app),
        Some(Action::InterfaceDown) => app.menu_cursor = (app.menu_cursor + 1) % n,
        Some(Action::InterfaceUp) => app.menu_cursor = (app.menu_cursor + n - 1) % n,
        Some(Action::InterfaceFirst) => app.menu_cursor = 0,
        Some(Action::InterfaceLast) => app.menu_cursor = n - 1,
        Some(Action::InterfaceAccept) => {
            let action = items[app.menu_cursor].action;
            leave_toolbar(app);
            run_action(app, action, 1);
        }
        None | Some(_) => {}
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
    let current = current_focus_region(app);
    if !current.is_toolbar() {
        app.toolbar_return = current;
    }
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
    if lookup_binding(app, key_event) == Some(Action::InterfaceAccept) {
        press_button(app, button_index);
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
    let in_nav_group = button_index < config::NAV_BUTTONS.len();
    let (first_button, last_button) = if in_nav_group {
        (0, config::NAV_BUTTONS.len() - 1)
    } else {
        (config::NAV_BUTTONS.len(), toolbar_button_count() - 1)
    };
    match lookup_binding(app, key_event) {
        Some(Action::Focus(direction)) => move_focus(app, direction),
        Some(Action::Cancel) => cancel_toolbar(app),
        Some(Action::InterfaceLeft) => {
            focus_button(app, button_index.saturating_sub(1).max(first_button));
        }
        Some(Action::InterfaceRight) => {
            focus_button(app, (button_index + 1).min(last_button));
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
        cancel_toolbar(app);
        return;
    }
    let last_segment_index = crate::ui::crumb_paths(&app.pane().cwd)
        .len()
        .saturating_sub(1);
    match lookup_binding(app, key_event) {
        Some(Action::Focus(direction)) => move_focus(app, direction),
        Some(Action::Cancel) => cancel_toolbar(app),
        Some(Action::InterfaceDown) => app.menu_cursor = (app.menu_cursor + 1) % items.len(),
        Some(Action::InterfaceUp) => {
            app.menu_cursor = (app.menu_cursor + items.len() - 1) % items.len();
        }
        Some(Action::InterfaceLeft) => open_crumb(app, segment_index.saturating_sub(1)),
        Some(Action::InterfaceRight) => {
            open_crumb(app, (segment_index + 1).min(last_segment_index));
        }
        Some(Action::InterfaceAccept) => accept_crumb(app, &items),
        None | Some(_) => {}
    }
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

    fn finish_test_transfer(app: &mut App) {
        let progress = app.transfer_progress.as_ref().unwrap();
        while !progress.finished.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::yield_now();
        }
        crate::finish_transfer(app);
    }

    #[test]
    fn digits_accumulate_into_a_count() {
        let mut app = test_app();
        press_char(&mut app, '1');
        press_char(&mut app, '2');
        assert_eq!(app.count, "12");
    }

    /// `ma` then `'a` is the whole of marks: the letter is read by the pending
    /// state rather than by the keymap, so any letter works — including one the
    /// keymap binds, as `'d` here proves by not deleting anything.
    #[test]
    fn a_mark_is_written_and_read_by_its_letter() {
        let mut app = test_app();
        let here = app.pane().target.clone();
        press_char(&mut app, 'm');
        assert_eq!(app.pending_mark, Some(MarkPending::Set));
        press_char(&mut app, 'd');
        assert_eq!(app.pending_mark, None);
        assert_eq!(app.marks.get(&'d'), Some(&here));
        assert_eq!(app.pending_delete, None);

        app.goto(Target::Dir(std::path::PathBuf::from("/")), true);
        press_char(&mut app, '\'');
        press_char(&mut app, 'd');
        assert_eq!(app.pane().target, here);
    }

    /// A letter no mark was written to reports rather than silently doing
    /// nothing, or a mistyped letter looks like a broken program.
    #[test]
    fn jumping_to_an_unwritten_mark_reports() {
        let mut app = test_app();
        press_char(&mut app, '\'');
        press_char(&mut app, 'z');
        assert!(app.status_is_error);
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
        assert_eq!(
            delete_range(KeyCode::Char('d'), cursor, last, 3),
            Some((0, 2))
        );
        assert_eq!(
            delete_range(KeyCode::Char('j'), cursor, last, 3),
            Some((0, 3))
        );
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

    #[test]
    fn normal_delete_uses_an_existing_selection() {
        let mut app = test_app();
        app.pane_mut()
            .selected
            .insert(std::path::PathBuf::from("selected-item"));

        run_action(&mut app, Action::DeleteOp, 1);

        assert_eq!(app.pending_delete, None);
    }

    #[test]
    fn configured_overlay_and_confirm_keys_are_authoritative() {
        let mut app = test_app();
        app.mode = Mode::Help;
        press_char(&mut app, 'x');
        assert_eq!(app.mode, Mode::Help);
        press_char(&mut app, 'q');
        assert_eq!(app.mode, Mode::Normal);

        app.mode = Mode::Confirm(Confirm::EmptyTrash);
        press_char(&mut app, 'n');
        assert!(matches!(app.mode, Mode::Confirm(Confirm::EmptyTrash)));
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn places_bindings_come_from_the_keymap() {
        let mut app = test_app();
        app.focus = Focus::Places;
        let left = normalize(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(lookup_binding(&app, left), Some(Action::NoOp));

        let ctrl_h = normalize(KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
        assert_eq!(
            lookup_binding(&app, ctrl_h),
            Some(Action::Focus(Direction::Left))
        );

        handle_key_event(&mut app, KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.focus, Focus::View);
    }

    #[test]
    fn ctrl_j_and_k_step_through_the_tab_pane() {
        let mut app = test_app();
        app.tabs
            .push(crate::app::Tab::new(std::env::temp_dir().join("other")));
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);

        handle_key_event(&mut app, ctrl('k'));
        assert_eq!(app.focus, Focus::Tabs);
        assert_eq!(app.mode, Mode::Normal);

        handle_key_event(&mut app, ctrl('j'));
        assert_eq!(app.focus, Focus::View);

        handle_key_event(&mut app, ctrl('k'));
        handle_key_event(&mut app, ctrl('k'));
        assert!(matches!(app.mode, Mode::CrumbMenu(_)));
        handle_key_event(&mut app, ctrl('j'));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.focus, Focus::Tabs);
        handle_key_event(&mut app, ctrl('j'));
        assert_eq!(app.focus, Focus::View);
    }

    #[test]
    fn ctrl_h_and_l_leave_tabs_for_the_left_and_right_views() {
        let mut app = test_app();
        let second = crate::app::Pane::new(std::env::temp_dir().join("right-pane"));
        app.tab_mut().panes.push(second);
        let ctrl = |c| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);

        app.focus = Focus::Tabs;
        app.tab_mut().active = 1;
        handle_key_event(&mut app, ctrl('h'));
        assert_eq!(app.focus, Focus::View);
        assert_eq!(app.tab().active, 0);

        app.focus = Focus::Tabs;
        handle_key_event(&mut app, ctrl('l'));
        assert_eq!(app.focus, Focus::View);
        assert_eq!(app.tab().active, 1);
    }

    #[test]
    fn tab_pane_h_and_l_select_tabs_without_moving_the_file_cursor() {
        let mut app = test_app();
        app.tabs
            .push(crate::app::Tab::new(std::env::temp_dir().join("other")));
        app.focus = Focus::Tabs;
        app.active_tab = 0;
        let cursor = app.pane().cursor;

        press_char(&mut app, 'l');
        assert_eq!(app.active_tab, 1);
        press_char(&mut app, 'h');
        assert_eq!(app.active_tab, 0);
        assert_eq!(app.pane().cursor, cursor);
    }

    #[test]
    fn modifiers_match_exactly() {
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let shift_enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            lookup_binding_for_mode(&Mode::Command, enter),
            Some(Action::CommitInput)
        );
        assert_eq!(lookup_binding_for_mode(&Mode::Command, shift_enter), None);

        let mut app = test_app();
        app.mode = Mode::Command;
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('x'), KeyModifiers::ALT),
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn chord_followers_require_exact_modifiers() {
        let mut app = test_app();
        press_char(&mut app, '2');
        press_char(&mut app, 'g');
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.pending_chord_leader, None);
        assert_eq!(app.count, "2");
    }

    fn install_test_entries(app: &mut App, names: &[&str]) {
        app.pane_mut().entries = names
            .iter()
            .map(|name| crate::fs::Entry {
                name: (*name).into(),
                path: PathBuf::from("/tmp").join(name),
                kind: crate::fs::Kind::File,
                size: 0,
                mtime: 0,
                mode: 0,
                readable: true,
                hidden: false,
                trash_id: None,
                depth: 0,
                expanded: false,
            })
            .collect();
        app.pane_mut().visible = (0..names.len()).collect();
    }

    #[test]
    fn visual_line_yank_commits_register_and_consumes_range() {
        let mut app = test_app();
        install_test_entries(&mut app, &["a", "b", "c"]);
        app.pane_mut().view = ViewMode::Compact;
        app.pane_mut().grid_rows = 3;
        app.pane_mut().cursor = 1;
        app.pane_mut().anchor = 1;
        app.mode = Mode::VisualLine;
        app.move_cursor(0, true);

        press_char(&mut app, 'y');

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pane().selected.is_empty());
        assert_eq!(app.status, "Yanked 1 item(s)");
        assert_eq!(
            app.register,
            ops::UnnamedRegister::Live {
                paths: vec![PathBuf::from("/tmp/b")],
                cut: false,
            }
        );
    }

    #[test]
    fn unbound_visual_keys_cannot_refilter_the_range() {
        let mut app = test_app();
        install_test_entries(&mut app, &["a", "b"]);
        app.mode = Mode::VisualLine;
        app.pane_mut().anchor = 0;
        app.move_cursor(0, true);

        press_char(&mut app, 'r');

        assert!(app.typeahead.is_empty());
        assert!(app.pane().filter.is_empty());
        assert_eq!(app.pane().selected.len(), 1);
        assert_eq!(app.mode, Mode::VisualLine);
    }

    #[test]
    fn ctrl_v_is_visual_block_never_paste() {
        let ctrl_v = normalize(KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL));
        for mode in [
            Mode::Normal,
            Mode::Visual,
            Mode::VisualLine,
            Mode::VisualBlock,
        ] {
            assert_eq!(
                lookup_binding_for_mode(&mode, ctrl_v),
                Some(Action::EnterVisualBlock)
            );
        }
    }

    #[test]
    fn visual_block_warns_outside_icons() {
        let mut app = test_app();
        app.pane_mut().view = ViewMode::Compact;
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status_is_error);
        assert!(app.status.contains("only available in Icons"));
    }

    #[test]
    fn ctrl_v_enters_visual_block_in_icons() {
        let mut app = test_app();
        app.pane_mut().view = ViewMode::Icons;
        handle_key_event(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
        );
        assert_eq!(app.mode, Mode::VisualBlock);
    }

    #[test]
    fn lookup_uses_the_current_mode() {
        let d = normalize(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE));
        assert_eq!(
            lookup_binding_for_mode(&Mode::Normal, d),
            Some(Action::DeleteOp)
        );
        assert_eq!(
            lookup_binding_for_mode(&Mode::Visual, d),
            Some(Action::DeleteSelection)
        );
        assert_eq!(
            lookup_binding_for_mode(&Mode::VisualLine, d),
            Some(Action::DeleteSelection)
        );
        assert_eq!(lookup_binding_for_mode(&Mode::Command, d), None);
    }

    #[test]
    fn the_same_key_is_discoverable_in_normal_text_and_interface_modes() {
        let left = normalize(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        assert_eq!(
            lookup_binding_for_mode(&Mode::Normal, left),
            Some(Action::MoveLeft)
        );
        assert_eq!(
            lookup_binding_for_mode(&Mode::Search, left),
            Some(Action::InputLeft)
        );
        assert_eq!(
            lookup_binding_for_mode(&Mode::Buttons(0), left),
            Some(Action::InterfaceLeft)
        );
        assert_eq!(
            lookup_binding_for_mode(&Mode::CrumbMenu(0), left),
            Some(Action::InterfaceLeft)
        );
    }

    #[test]
    fn text_mode_printables_do_not_leak_normal_bindings() {
        let mut app = test_app();
        app.mode = Mode::Command;
        press_char(&mut app, 'h');
        assert_eq!(app.input, "h");
        assert_eq!(app.mode, Mode::Command);
    }

    /// The shift is in the character, so a terminal that reports it as well
    /// and one that does not must reach the same binding.
    #[test]
    fn shifted_printables_match_with_or_without_the_modifier() {
        let with = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT);
        let without = KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE);
        assert_eq!(
            lookup_binding_for_mode(&Mode::Normal, normalize(with)),
            Some(Action::Bottom)
        );
        assert_eq!(
            lookup_binding_for_mode(&Mode::Normal, normalize(without)),
            Some(Action::Bottom)
        );
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
            assert_eq!(
                lookup_binding_for_mode(&Mode::Normal, normalize(ev)),
                Some(Action::PrevTab)
            );
        }
    }

    /// Shift+Delete is not a printable: its modifier is the whole difference
    /// between trashing and deleting, so normalization must leave it alone.
    #[test]
    fn shift_still_counts_on_a_named_key() {
        let ev = KeyEvent::new(KeyCode::Delete, KeyModifiers::SHIFT);
        assert_eq!(
            lookup_binding_for_mode(&Mode::Normal, normalize(ev)),
            Some(Action::DeletePerm)
        );
    }

    #[test]
    fn unbound_printable_keys_feed_typeahead() {
        let mut app = test_app();
        press_char(&mut app, 'q');
        assert_eq!(app.typeahead, "q");
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn directional_focus_table_is_complete() {
        use Direction::{Down, Left, Right, Up};
        use FocusRegion::{Breadcrumb, Places, Tabs, ToolbarNav, ToolbarRight, View};
        let mut app = test_app();
        app.tabs.push(crate::app::Tab::new(std::env::temp_dir()));
        app.tab_mut()
            .panes
            .push(crate::app::Pane::new(std::env::temp_dir()));
        app.toolbar_return = Tabs;
        let stay = FocusTransition::Stay;
        let mv = |r| FocusTransition::Move(r);
        let cases = [
            (Places, Left, stay),
            (Places, Right, mv(View(0))),
            (Places, Up, mv(ToolbarNav)),
            (Places, Down, stay),
            (View(0), Left, mv(Places)),
            (View(0), Right, mv(View(1))),
            (View(0), Up, mv(Tabs)),
            (View(0), Down, stay),
            (View(1), Left, mv(View(0))),
            (View(1), Right, stay),
            (View(1), Up, mv(Tabs)),
            (View(1), Down, stay),
            (Tabs, Left, mv(View(0))),
            (Tabs, Right, mv(View(1))),
            (Tabs, Up, mv(Breadcrumb)),
            (Tabs, Down, mv(View(0))),
            (ToolbarNav, Left, stay),
            (ToolbarNav, Right, mv(Breadcrumb)),
            (ToolbarNav, Up, stay),
            (ToolbarNav, Down, mv(Tabs)),
            (Breadcrumb, Left, mv(ToolbarNav)),
            (Breadcrumb, Right, mv(ToolbarRight)),
            (Breadcrumb, Up, stay),
            (Breadcrumb, Down, mv(Tabs)),
            (ToolbarRight, Left, mv(Breadcrumb)),
            (ToolbarRight, Right, stay),
            (ToolbarRight, Up, stay),
            (ToolbarRight, Down, mv(Tabs)),
        ];
        for (region, direction, expected) in cases {
            assert_eq!(
                region.move_focus(direction, &app),
                expected,
                "{region:?} {direction:?}"
            );
        }
    }

    #[test]
    fn directional_neighbours_follow_dynamic_layout() {
        use Direction::{Left, Right, Up};
        use FocusRegion::{Breadcrumb, View};
        let mut app = test_app();
        app.places_visible = false;
        assert_eq!(View(0).move_focus(Left, &app), FocusTransition::Stay);
        assert_eq!(View(0).move_focus(Right, &app), FocusTransition::Stay);
        assert_eq!(
            View(0).move_focus(Up, &app),
            FocusTransition::Move(Breadcrumb)
        );
        app.tab_mut()
            .panes
            .push(crate::app::Pane::new(std::env::temp_dir()));
        assert_eq!(
            View(0).move_focus(Right, &app),
            FocusTransition::Move(View(1))
        );
    }

    #[test]
    fn toolbar_down_and_cancel_restore_each_body_region() {
        for origin in [
            FocusRegion::Places,
            FocusRegion::Tabs,
            FocusRegion::View(0),
            FocusRegion::View(1),
        ] {
            let mut app = test_app();
            app.tab_mut()
                .panes
                .push(crate::app::Pane::new(std::env::temp_dir()));
            if origin == FocusRegion::Tabs {
                app.tabs.push(crate::app::Tab::new(std::env::temp_dir()));
            }
            let current = current_focus_region(&app);
            enter_focus_region(&mut app, current, origin);
            move_focus(&mut app, Direction::Up);
            assert!(current_focus_region(&app).is_toolbar());
            move_focus(&mut app, Direction::Down);
            assert_eq!(current_focus_region(&app), origin);

            move_focus(&mut app, Direction::Up);
            cancel_toolbar(&mut app);
            assert_eq!(current_focus_region(&app), origin);
        }
    }

    #[test]
    fn focused_tabs_shadow_the_file_view_pipeline() {
        let mut app = test_app();
        app.tabs.push(crate::app::Tab::new(std::env::temp_dir()));
        app.focus = Focus::Tabs;
        app.pane_mut().cursor = 7;
        app.pane_mut().selected.insert(PathBuf::from("selected"));
        for key in ['j', 'k', 'd', 'z'] {
            press_char(&mut app, key);
        }
        assert_eq!(app.pane().cursor, 7);
        assert!(!app.pane().selected.is_empty());
        assert_eq!(app.pending_delete, None);
        assert_eq!(app.pending_chord_leader, None);
        assert!(app.typeahead.is_empty());
    }

    #[test]
    fn layout_changes_repair_body_and_return_focus() {
        let mut app = test_app();
        app.tab_mut()
            .panes
            .push(crate::app::Pane::new(std::env::temp_dir()));
        app.tab_mut().active = 1;
        app.toolbar_return = FocusRegion::View(1);
        app.toggle_split();
        assert_eq!(app.tab().active, 0);
        assert_eq!(app.toolbar_return, FocusRegion::View(0));

        app.focus = Focus::Places;
        app.toolbar_return = FocusRegion::Places;
        run_action(&mut app, Action::TogglePlaces, 1);
        assert_eq!(app.focus, Focus::View);
        assert_eq!(app.toolbar_return, FocusRegion::View(0));

        app.tabs.push(crate::app::Tab::new(std::env::temp_dir()));
        app.focus = Focus::Tabs;
        app.toolbar_return = FocusRegion::Tabs;
        app.close_tab();
        assert_eq!(app.focus, Focus::View);
        assert!(matches!(app.toolbar_return, FocusRegion::View(_)));
    }

    #[test]
    fn modal_and_text_modes_do_not_leak_focus_actions() {
        let mut app = test_app();
        for mode in [
            Mode::Command,
            Mode::Search,
            Mode::PathEdit,
            Mode::Confirm(Confirm::EmptyTrash),
        ] {
            app.mode = mode.clone();
            let before = current_focus_region(&app);
            handle_key_event(&mut app, ctrl('k'));
            assert_eq!(app.mode, mode);
            assert_eq!(current_focus_region(&app), before);
        }
    }

    #[test]
    fn breadcrumb_entry_restores_a_remembered_openable_segment() {
        let base = std::env::temp_dir().join(format!("dolvim-focus-{}", std::process::id()));
        let left = base.join("left");
        let right = base.join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        let mut app = App::new(left);
        app.pane_mut().crumb_focus = Some(base.clone());
        enter_crumbs(&mut app, 0);
        let expected = crate::ui::crumb_paths(&app.pane().cwd)
            .iter()
            .position(|path| path == &base)
            .unwrap();
        assert_eq!(app.mode, Mode::CrumbMenu(expected));
        assert_eq!(app.pane().crumb_focus.as_ref(), Some(&base));
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn unavailable_breadcrumb_falls_back_to_the_adjacent_group() {
        let mut app = test_app();
        app.pane_mut().cwd = PathBuf::new();
        app.mode = Mode::Buttons(config::NAV_BUTTONS.len() - 1);
        app.toolbar_return = FocusRegion::View(0);
        move_focus(&mut app, Direction::Right);
        assert_eq!(current_focus_region(&app), FocusRegion::ToolbarNav);
        assert_eq!(app.mode, Mode::Menu(MenuKind::ViewMode));
    }

    #[test]
    fn new_folder_is_a_sibling_and_receives_focus() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-new-folder-{unique}"));
        std::fs::create_dir_all(base.join("folder-under-cursor")).unwrap();
        let mut app = App::new(base.clone());
        app.pane_mut()
            .set_entries(crate::fs::read_dir(&base, 0).unwrap());

        press_char(&mut app, 'O');
        assert!(matches!(
            &app.mode,
            Mode::NewFolder(intent) if intent.directory == base
        ));
        for character in "sibling".chars() {
            press_char(&mut app, character);
        }
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        for _ in 0..1_000 {
            app.pump_fs_events();
            if !app.pane().loading && app.pane().pending_focus.is_none() {
                break;
            }
            std::thread::yield_now();
        }
        assert_eq!(
            app.pane().current().map(|entry| entry.path.clone()),
            Some(base.join("sibling"))
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn creation_is_rejected_in_virtual_locations() {
        let mut app = test_app();
        app.pane_mut().target = Target::Trash;
        press_char(&mut app, 'o');
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status_is_error);
    }

    #[test]
    fn creation_intent_survives_a_split_pane_focus_change() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-create-split-{unique}"));
        let left = base.join("left");
        let right = base.join("right");
        std::fs::create_dir_all(&left).unwrap();
        std::fs::create_dir_all(&right).unwrap();
        let mut app = App::new(left.clone());
        app.tab_mut().panes.push(crate::app::Pane::new(right));

        press_char(&mut app, 'o');
        app.tab_mut().active = 1;
        for character in "left.txt".chars() {
            press_char(&mut app, character);
        }
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

        assert!(left.join("left.txt").is_file());
        assert_eq!(app.tab().active, 0);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn disappearing_creation_target_fails_without_history() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-create-gone-{unique}"));
        let folder = base.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let mut app = App::new(base.clone());
        app.pane_mut()
            .set_entries(crate::fs::read_dir(&base, 0).unwrap());
        press_char(&mut app, 'o');
        let undo_len = app.undo.len();
        std::fs::remove_dir(&folder).unwrap();
        for character in "never.txt".chars() {
            press_char(&mut app, character);
        }
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.undo.len(), undo_len);
        assert!(app.status_is_error);
        assert!(!folder.join("never.txt").exists());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn symlinked_folder_is_a_defined_new_file_destination() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-create-symlink-{unique}"));
        let target = base.join("target");
        let link = base.join("link");
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let mut app = App::new(base.clone());
        app.pane_mut()
            .set_entries(crate::fs::read_dir(&base, 0).unwrap());
        let link_index = app
            .pane()
            .visible
            .iter()
            .position(|&index| app.pane().entries[index].path == link)
            .unwrap();
        app.pane_mut().cursor = link_index;
        press_char(&mut app, 'o');
        for character in "through-link.txt".chars() {
            press_char(&mut app, character);
        }
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(target.join("through-link.txt").is_file());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn new_file_on_a_folder_creates_the_file_inside_it() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base =
            std::env::temp_dir().join(format!("dolvim-new-file-{}-{unique}", std::process::id()));
        let folder = base.join("folder");
        std::fs::create_dir_all(&folder).unwrap();

        {
            let mut app = App::new(base.clone());
            let entries = crate::fs::read_dir(&base, 0).unwrap();
            app.pane_mut().set_entries(entries);

            press_char(&mut app, 'o');
            assert!(matches!(
                &app.mode,
                Mode::NewFile(intent) if intent.directory == folder
            ));
            for character in "created.txt".chars() {
                press_char(&mut app, character);
            }
            handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

            assert!(folder.join("created.txt").is_file());
            assert!(!base.join("created.txt").exists());
            for _ in 0..1_000 {
                app.pump_fs_events();
                if !app.pane().loading && app.pane().pending_focus.is_none() {
                    break;
                }
                std::thread::yield_now();
            }
            assert_eq!(app.pane().cwd, folder);
            assert_eq!(
                app.pane().current().map(|entry| entry.path.clone()),
                Some(folder.join("created.txt"))
            );
        }

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn visual_delete_can_be_pasted_back() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "dolvim-visual-delete-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let deleted = base.join("preserved.txt");
        std::fs::write(&deleted, b"keep me").unwrap();

        {
            let mut app = App::new(base.clone());
            let entries = crate::fs::read_dir(&base, 0).unwrap();
            app.pane_mut().set_entries(entries);

            press_char(&mut app, 'v');
            press_char(&mut app, 'd');
            assert!(!deleted.exists());
            match &app.register {
                ops::UnnamedRegister::Deleted { items } => {
                    assert_eq!(items.len(), 1);
                    assert_eq!(items[0].original_path, deleted);
                }
                other => panic!("expected deleted register, got {other:?}"),
            }

            press_char(&mut app, 'p');
            finish_test_transfer(&mut app);
            assert_eq!(std::fs::read(&deleted).unwrap(), b"keep me");
            assert!(matches!(
                app.register,
                ops::UnnamedRegister::Live { cut: false, .. }
            ));
        }

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn partial_move_reduces_register_history_and_status_from_committed_effects() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-partial-move-{unique}"));
        let source_dir = base.join("source");
        let destination = base.join("destination");
        std::fs::create_dir_all(&source_dir).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        let valid = source_dir.join("valid.txt");
        let missing = source_dir.join("missing.txt");
        std::fs::write(&valid, b"payload").unwrap();

        let mut app = App::new(destination.clone());
        app.register = ops::UnnamedRegister::Live {
            paths: vec![valid.clone(), missing.clone()],
            cut: true,
        };
        let mut progress = ops::start_transfer(
            vec![valid.clone(), missing.clone()],
            destination.clone(),
            ops::TransferKind::Move,
        );
        progress.expected_register = Some(app.register.clone());
        app.transfer_progress = Some(progress);
        finish_test_transfer(&mut app);

        assert!(!valid.exists());
        assert_eq!(
            std::fs::read(destination.join("valid.txt")).unwrap(),
            b"payload"
        );
        assert_eq!(
            app.register,
            ops::UnnamedRegister::Live {
                paths: vec![missing],
                cut: true,
            }
        );
        assert!(
            matches!(app.undo.last(), Some(ops::UndoOp::Move { moved_pairs }) if moved_pairs.len() == 1)
        );
        assert!(app.status_is_error);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn failed_delete_preserves_register_and_history() {
        let mut app = test_app();
        let previous = PathBuf::from("previous-register-item");
        app.register.set(vec![previous.clone()], false);
        let undo_len = app.undo.len();

        delete_to_register(
            &mut app,
            vec![std::env::temp_dir()
                .join(format!("dolvim-definitely-missing-{}", std::process::id()))],
        );

        assert_eq!(
            app.register,
            ops::UnnamedRegister::Live {
                paths: vec![previous],
                cut: false,
            }
        );
        assert_eq!(app.undo.len(), undo_len);
    }

    #[test]
    fn delete_paste_and_undo_rebases_the_older_delete_history() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "dolvim-delete-paste-undo-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("item.txt");
        std::fs::write(&path, b"payload").unwrap();

        {
            let mut app = App::new(base.clone());
            app.pane_mut()
                .set_entries(crate::fs::read_dir(&base, 0).unwrap());
            press_char(&mut app, 'v');
            press_char(&mut app, 'd');
            press_char(&mut app, 'p');
            finish_test_transfer(&mut app);
            assert!(path.exists());

            press_char(&mut app, 'u');
            assert!(!path.exists());
            assert!(matches!(app.register, ops::UnnamedRegister::Deleted { .. }));

            // The older delete entry was rebound to the new Trash generation,
            // rather than being left stale by undoing paste.
            press_char(&mut app, 'u');
            assert_eq!(std::fs::read(&path).unwrap(), b"payload");
            assert!(matches!(
                app.register,
                ops::UnnamedRegister::Live { cut: false, .. }
            ));
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn delete_undo_then_paste_uses_the_restored_live_register() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!(
            "dolvim-delete-undo-paste-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&base).unwrap();
        let path = base.join("item.txt");
        std::fs::write(&path, b"payload").unwrap();

        {
            let mut app = App::new(base.clone());
            app.pane_mut()
                .set_entries(crate::fs::read_dir(&base, 0).unwrap());
            press_char(&mut app, 'v');
            press_char(&mut app, 'd');
            press_char(&mut app, 'u');
            assert!(matches!(
                app.register,
                ops::UnnamedRegister::Live { cut: false, .. }
            ));

            press_char(&mut app, 'p');
            let progress = app.transfer_progress.as_ref().unwrap();
            while !progress.finished.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::yield_now();
            }
            assert_eq!(
                std::fs::read(base.join("item (1).txt")).unwrap(),
                b"payload"
            );
        }
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn menu_button_opens_on_entry_and_accept_lands_in_the_view() {
        let mut app = test_app();
        app.focus = Focus::Places;
        move_focus(&mut app, Direction::Up);
        assert_eq!(app.mode, Mode::Buttons(0));
        press_char(&mut app, 'l');
        press_char(&mut app, 'l');
        assert_eq!(app.mode, Mode::Menu(MenuKind::ViewMode));
        handle_key_event(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(current_focus_region(&app), FocusRegion::View(0));
    }
}
