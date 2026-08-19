//! Application state: tabs, panes, selection, navigation, modes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use ratatui::layout::Rect;

use crate::config;
pub use crate::config::ViewMode;
use crate::editor;
use crate::fs::{self, Entry, Lister, Sort, SortKey};
use crate::open;
use crate::ops::{self, Progress, UndoOp, UnnamedRegister};
use crate::places::{self, Row, Target};
use crate::thumbs::Thumbs;
use crate::watch::Watcher;

/// Work that needs the terminal to itself. Dolvim leaves the alternate
/// screen, runs it to completion, and comes back.
pub enum Suspend {
    /// F4: a shell in this directory.
    Shell(PathBuf),
    /// A resolved command whose handler needs exclusive use of the terminal.
    Open(open::Plan),
    /// An offline block device whose authorization prompt needs a normal tty.
    Mount(PathBuf),
    /// A mounted block device whose authorization prompt needs a normal tty.
    Unmount(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Places,
    Tabs,
    View,
}

/// Directional intent shared by every focusable region.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The effective keyboard focus. Unlike `Focus`, this includes the active view
/// index and the three independently navigable parts of the toolbar row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FocusRegion {
    Places,
    Tabs,
    View(usize),
    ToolbarNav,
    Breadcrumb,
    ToolbarRight,
}

impl FocusRegion {
    pub fn is_toolbar(self) -> bool {
        matches!(
            self,
            Self::ToolbarNav | Self::Breadcrumb | Self::ToolbarRight
        )
    }
}

/// What the keyboard is currently feeding. Text-entry modes carry their buffer
/// in `App::input`; `Mode` only says who owns the next keystroke.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RevealMode {
    /// Open the operation's destination when it differs from the pane cwd.
    NavigateDirectory,
    /// Keep the parent visible and expand the destination inline.
    ExpandDirectory,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RevealIntent {
    pub directory: PathBuf,
    pub pane_id: u64,
    pub mode: RevealMode,
}

/// A filesystem worker and the UI contract captured when it was started.
/// Keeping them together prevents asynchronous completion from losing which
/// pane, destination, and reveal policy belong to the operation.
pub struct ActiveTransfer {
    pub progress: Progress,
    pub observation_id: Option<u64>,
    pub reveal: Option<RevealIntent>,
    /// Pane whose selection produced the operation. This is deliberately not
    /// derived from `reveal`: a split-pane drag starts in one pane and is
    /// revealed in the other.
    pub selection_pane_id: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Visual,
    /// `V`. Linewise: the range grows by whole Icon rows and by individual
    /// file lines in Compact and Details.
    VisualLine,
    /// `Ctrl+V`. Rectangular selection, available only in Icons view.
    VisualBlock,
    /// `:` command line.
    Command,
    /// `/` incremental search.
    Search,
    /// `Ctrl+I` live filter.
    Filter,
    /// `F6` editable location bar.
    PathEdit,
    /// F2 inline rename; the payload is the path being renamed.
    Rename(PathBuf),
    /// F2 with a multi-selection: one pattern renames them all.
    BatchRename,
    /// F10 / `O`.
    NewFolder(RevealIntent),
    /// `o`; captures the destination and pane when the prompt opens.
    NewFile(RevealIntent),
    /// A yes/no gate. Carries what to do when the answer is yes.
    Confirm(Confirm),
    /// Modal information overlays.
    Properties,
    Help,
    /// A dropdown of sibling directories hanging off a breadcrumb segment.
    CrumbMenu(usize),
    /// Focus is on a toolbar button: an index into `config::toolbar_buttons`.
    Buttons(usize),
    Menu(MenuKind),
}

impl Mode {
    /// Any visual mode: a range is being dragged and motions extend it.
    pub fn is_visual(&self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine | Mode::VisualBlock)
    }

    /// What the status bar calls this mode. The match is exhaustive so that a
    /// new variant cannot ship without deciding what the user is told it is.
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::VisualBlock => "V-BLOCK",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Filter => "FILTER",
            Mode::PathEdit => "PATH",
            Mode::Rename(_) | Mode::BatchRename => "RENAME",
            Mode::NewFolder(_) => "NEW FOLDER",
            Mode::NewFile(_) => "NEW FILE",
            Mode::Confirm(_) => "CONFIRM",
            Mode::Properties => "PROPERTIES",
            Mode::Help => "HELP",
            Mode::CrumbMenu(_) => "CRUMBS",
            Mode::Buttons(_) => "TOOLBAR",
            Mode::Menu(_) => "MENU",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Hamburger,
    ViewMode,
    Sort,
}

/// Which half of a mark is waiting for its letter: `m` writing one, or `'`
/// reading one. The letter itself is whatever key comes next.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkPending {
    Set,
    Jump,
}

/// A stable row to focus after a listing arrives. The path restores expanded
/// tree context; the selection key distinguishes repeated Trash generations.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EntryFocus {
    pub path: PathBuf,
    pub selection_key: PathBuf,
    retry_missing: bool,
}

impl EntryFocus {
    fn new(entry: &Entry) -> Self {
        Self {
            path: entry.path.clone(),
            selection_key: entry.selection_key(),
            retry_missing: false,
        }
    }

    fn matches(&self, entry: &Entry) -> bool {
        self.selection_key == entry.selection_key()
    }
}

/// A browsable target together with the row under the cursor there.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Location {
    pub target: Target,
    pub focus: Option<EntryFocus>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Confirm {
    DeletePermanently(Vec<PathBuf>),
    /// The same question for items already in the Trash, which have to be
    /// purged through the trash index rather than unlinked where they lie.
    PurgeFromTrash(Vec<ops::TrashRef>),
    EmptyTrash,
}

/// One file view. Two of these exist when the split view is on.
pub struct Pane {
    pub id: u64,
    pub cwd: PathBuf,
    /// The Places target this pane is showing, when it is not a plain dir.
    pub target: Target,
    pub entries: Vec<Entry>,
    /// Indices into `entries`, after hidden/filter, in sort order.
    pub visible: Vec<usize>,
    pub cursor: usize,
    pub anchor: usize,
    pub offset: usize,
    pub selected: HashSet<PathBuf>,
    pub view: ViewMode,
    pub sort: Sort,
    pub show_hidden: bool,
    pub filter: String,
    pub expanded: HashSet<PathBuf>,
    /// Stable row that an asynchronous refresh should place under the cursor.
    pub pending_focus: Option<EntryFocus>,
    pub history: Vec<Target>,
    pub history_pos: usize,
    place: Target,
    place_histories: Vec<(Target, Vec<Target>, usize)>,
    pub seq: u64,
    pub loading: bool,
    pub error: Option<String>,
    /// Geometry cached at render time so mouse events can be hit-tested.
    pub area: Rect,
    pub grid_cols: u16,
    pub grid_rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    /// Compact sizes each column to its own longest name, so the one cell width
    /// above cannot describe it. Widths of the rendered columns, left to right.
    pub column_widths: Vec<u16>,
    /// All Compact column widths. Unlike `column_widths`, this survives redraws
    /// and scrolling; its dimensions identify the layout it was measured for.
    pub compact_widths: Vec<u16>,
    pub compact_width_rows: u16,
    pub compact_width_avail: u16,
    /// Left edge of the icon grid, which floats inside the pane as the leftover
    /// columns are split into margin.
    pub grid_x: u16,
    /// Cursor and view the viewport was last scrolled to follow. The renderer
    /// reveals the cursor when this goes stale, and only then.
    pub last_reveal: (usize, ViewMode),
    /// Trail segment the breadcrumb focus last sat on, and the row left
    /// highlighted in its menu. Both are paths, not indices, so that navigating
    /// or a directory changing underneath cannot leave them pointing at
    /// something else — a stale one is simply not found, and the default wins.
    pub crumb_focus: Option<PathBuf>,
    pub crumb_pick: Option<PathBuf>,
}

fn expanded_listing(roots: Vec<Entry>, expanded: &HashSet<PathBuf>, sort: Sort) -> Vec<Entry> {
    fn append(
        mut entry: Entry,
        depth: u16,
        expanded: &HashSet<PathBuf>,
        sort: Sort,
        output: &mut Vec<Entry>,
    ) {
        entry.depth = depth;
        entry.expanded = entry.is_dir() && expanded.contains(&entry.path);
        let path = entry.path.clone();
        let backing_path = entry.backing_path.clone();
        let is_expanded = entry.expanded;
        output.push(entry);
        if !is_expanded {
            return;
        }
        let physical = backing_path.as_deref().unwrap_or(&path);
        if let Ok(mut children) =
            fs::read_dir_as(physical, &path, depth + 1, backing_path.is_some())
        {
            fs::sort_entries(&mut children, sort);
            for child in children {
                append(child, depth + 1, expanded, sort, output);
            }
        }
    }

    let mut output = Vec::new();
    for root in roots {
        append(root, 0, expanded, sort, &mut output);
    }
    output
}

/// Collect folders below `roots`, guarding against directory symlink cycles.
/// The paths themselves (rather than canonical paths) remain the fold keys.
fn recursive_folders(roots: Vec<Entry>) -> (HashSet<PathBuf>, Option<(PathBuf, std::io::Error)>) {
    let mut folders = HashSet::new();
    let mut visited = HashSet::new();
    let mut pending = roots;
    let mut first_error = None;

    while let Some(entry) = pending.pop() {
        folders.insert(entry.path.clone());
        let physical = entry.filesystem_path();
        let identity = std::fs::canonicalize(physical).unwrap_or_else(|_| physical.to_path_buf());
        if !visited.insert(identity) {
            continue;
        }
        match fs::read_dir_as(
            physical,
            &entry.path,
            entry.depth + 1,
            entry.backing_path.is_some(),
        ) {
            Ok(entries) => pending.extend(entries.into_iter().filter(Entry::is_dir)),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some((entry.path, error));
                }
            }
        }
    }
    (folders, first_error)
}

static NEXT_PANE_ID: AtomicU64 = AtomicU64::new(1);

impl Pane {
    /// User-facing location. Virtual targets must never expose their backing
    /// storage or vanished original filesystem path as the current location.
    pub fn display_path(&self) -> &Path {
        match &self.target {
            Target::TrashDir { display, .. } => display,
            _ => &self.cwd,
        }
    }

    pub fn new(cwd: PathBuf) -> Pane {
        Pane {
            id: NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed),
            target: Target::Dir(cwd.clone()),
            history: vec![Target::Dir(cwd.clone())],
            place: Target::Dir(cwd.clone()),
            place_histories: Vec::new(),
            cwd,
            entries: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            anchor: 0,
            offset: 0,
            selected: HashSet::new(),
            view: config::DEFAULT_VIEW,
            sort: Sort::default(),
            show_hidden: false,
            filter: String::new(),
            expanded: HashSet::new(),
            pending_focus: None,
            history_pos: 0,
            seq: 0,
            loading: true,
            error: None,
            area: Rect::default(),
            grid_cols: 1,
            grid_rows: 1,
            cell_width: 1,
            cell_height: 1,
            column_widths: Vec::new(),
            compact_widths: Vec::new(),
            compact_width_rows: 0,
            compact_width_avail: 0,
            grid_x: 0,
            last_reveal: (0, config::DEFAULT_VIEW),
            crumb_focus: None,
            crumb_pick: None,
        }
    }

    pub fn len(&self) -> usize {
        self.visible.len()
    }

    pub fn is_empty(&self) -> bool {
        self.visible.is_empty()
    }

    pub fn entry_at(&self, visible_index: usize) -> Option<&Entry> {
        self.visible
            .get(visible_index)
            .and_then(|&entry_index| self.entries.get(entry_index))
    }

    pub fn current(&self) -> Option<&Entry> {
        self.entry_at(self.cursor)
    }

    /// Install a fresh listing, keeping the cursor on the same file.
    ///
    /// The path under the cursor has to be read *before* the swap: `visible`
    /// holds indices into the old `entries`, and a new listing arrives in
    /// readdir order, so afterwards those indices point at arbitrary files.
    /// That is why this exists rather than assigning `entries` and calling
    /// `refilter` — doing it in that order sends the cursor to a random file
    /// on every refresh.
    pub fn set_entries(&mut self, entries: Vec<Entry>) {
        let keep = self.current().map(EntryFocus::new);
        self.entries = entries;
        self.refilter_keeping(keep);
        let Some(wanted) = self.pending_focus.clone() else {
            return;
        };
        let Some(found) = self.entries.iter().find(|entry| wanted.matches(entry)) else {
            // Creates may race their first listing; ordinary navigation must
            // settle even when the remembered row was removed in the meantime.
            if !wanted.retry_missing {
                self.pending_focus = None;
            }
            return;
        };
        if found.hidden {
            self.show_hidden = true;
        }
        self.filter.clear();
        self.pending_focus = None;
        self.refilter_keeping(Some(wanted));
    }

    /// Ask the next listing to focus a path that may not exist in the current
    /// snapshot yet, as after create or rename.
    pub fn focus_after_refresh(&mut self, path: PathBuf) {
        self.pending_focus = Some(EntryFocus {
            selection_key: path.clone(),
            path,
            retry_missing: true,
        });
    }

    fn reveal_ancestors(&mut self, focus: &EntryFocus) {
        let Some(mut ancestor) = focus.path.parent() else {
            return;
        };
        while ancestor != self.cwd && ancestor.starts_with(&self.cwd) {
            self.expanded.insert(ancestor.to_path_buf());
            let Some(parent) = ancestor.parent() else {
                break;
            };
            ancestor = parent;
        }
    }

    fn focus_now(&mut self, focus: EntryFocus) {
        self.reveal_ancestors(&focus);
        self.filter.clear();
        self.refilter_keeping(None);
        if self
            .entries
            .iter()
            .any(|entry| focus.matches(entry) && entry.hidden)
        {
            self.show_hidden = true;
        }
        self.refilter_keeping(Some(focus));
    }

    /// Recompute `visible` from `entries` honouring hidden, filter and sort,
    /// keeping the cursor on the same file where possible.
    pub fn refilter(&mut self) {
        let keep = self.current().map(EntryFocus::new);
        self.refilter_keeping(keep);
    }

    fn refilter_keeping(&mut self, keep: Option<EntryFocus>) {
        // A worker listing contains roots only. Rebuild expanded descendants
        // from the durable path set before filtering; globally sorting the flat
        // tree would separate children from their parents.
        let mut roots: Vec<Entry> = self
            .entries
            .iter()
            .filter(|entry| entry.depth == 0)
            .cloned()
            .collect();
        fs::sort_entries(&mut roots, self.sort);
        self.entries = expanded_listing(roots, &self.expanded, self.sort);
        self.revisible();
        self.cursor = keep
            .and_then(|wanted| {
                self.visible
                    .iter()
                    .position(|&entry_index| wanted.matches(&self.entries[entry_index]))
            })
            .unwrap_or_else(|| self.cursor.min(self.visible.len().saturating_sub(1)));
        self.clamp();
    }

    /// Rebuild `visible` only. Kept separate from `refilter` because the
    /// Details tree is ordered positionally and must not be re-sorted.
    fn revisible(&mut self) {
        self.compact_widths.clear();
        let filter_lower = self.filter.to_lowercase();
        let hidden_ok = self.show_hidden;
        let visible_indices: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| hidden_ok || !e.hidden)
            .filter(|(_, e)| {
                filter_lower.is_empty() || e.name.to_lowercase().contains(&filter_lower)
            })
            .map(|(i, _)| i)
            .collect();
        self.visible = visible_indices;
    }

    pub fn clamp(&mut self) {
        if self.visible.is_empty() {
            self.cursor = 0;
            self.offset = 0;
            return;
        }
        self.cursor = self.cursor.min(self.visible.len() - 1);
    }

    /// Put the navigation line containing the cursor halfway down (or across)
    /// the viewport, as Vim's `zz` does. Near the start there is no preceding
    /// content with which to fill the first half, so the offset saturates at zero.
    pub fn center_cursor(&mut self) {
        let rows = self.grid_rows.max(1) as usize;
        self.offset = match self.view {
            ViewMode::Icons => {
                (self.cursor / self.grid_cols.max(1) as usize).saturating_sub(rows / 2)
            }
            ViewMode::Details => self.cursor.saturating_sub(rows / 2),
            ViewMode::Compact => {
                let column = self.cursor / rows;
                let half = self.compact_width_avail as usize / 2;
                let mut offset = column;
                let mut width = self.compact_widths.get(column).copied().unwrap_or(0) as usize / 2;
                while offset > 0 && width < half {
                    offset -= 1;
                    width += self.compact_widths.get(offset).copied().unwrap_or(0) as usize;
                }
                offset
            }
        };
        self.last_reveal = (self.cursor, self.view);
    }

    /// Furthest the viewport may scroll: the last screenful, not the last item.
    pub fn max_offset(&self) -> usize {
        let (cols, rows) = (
            self.grid_cols.max(1) as usize,
            self.grid_rows.max(1) as usize,
        );
        match self.view {
            ViewMode::Icons => self.len().div_ceil(cols).saturating_sub(rows),
            // Compact flows down columns, so it scrolls sideways.
            ViewMode::Compact => self.len().div_ceil(rows).saturating_sub(cols),
            ViewMode::Details => self.len().saturating_sub(rows),
        }
    }

    /// Items in one contiguous navigation line: a row in Icons, a column in
    /// Compact, and one entry in Details.
    pub fn stride(&self) -> usize {
        match self.view {
            ViewMode::Icons => self.grid_cols.max(1) as usize,
            // Compact flows down columns, so a horizontal step is a full column.
            ViewMode::Compact => self.grid_rows.max(1) as usize,
            ViewMode::Details => 1,
        }
    }

    /// Items in a linewise selection unit. This deliberately differs from
    /// navigation stride in Compact: its column-major storage makes a whole
    /// column contiguous, but each vertically navigated entry is one file line.
    pub fn linewise_width(&self) -> usize {
        match self.view {
            ViewMode::Icons => self.grid_cols.max(1) as usize,
            ViewMode::Compact | ViewMode::Details => 1,
        }
    }

    /// Rows visible at once, for page motions.
    pub fn page(&self) -> usize {
        match self.view {
            ViewMode::Icons => (self.grid_rows.max(1) as usize) * self.stride(),
            ViewMode::Compact => (self.grid_cols.max(1) as usize) * self.stride(),
            ViewMode::Details => self.area.height.saturating_sub(1).max(1) as usize,
        }
    }

    pub fn selected_paths(&self) -> Vec<PathBuf> {
        // Selection first; with nothing selected, the cursor is the selection,
        // which is what Dolphin does for Copy/Delete/Rename.
        if self.selected.is_empty() {
            self.current()
                .map(|e| vec![e.path.clone()])
                .unwrap_or_default()
        } else {
            self.visible
                .iter()
                .filter_map(|&entry_index| {
                    let e = &self.entries[entry_index];
                    self.selected
                        .contains(&e.selection_key())
                        .then(|| e.path.clone())
                })
                .collect()
        }
    }

    /// Exact Trash generations selected by the cursor/range. Trash rows may
    /// share an original path, so pathname alone is never an operation identity.
    pub fn selected_trash_refs(&self) -> Vec<ops::TrashRef> {
        let to_ref = |entry: &Entry| {
            Some(ops::TrashRef {
                id: entry.trash_id.clone()?,
                original_path: entry.path.clone(),
                name: entry.path.file_name()?.to_os_string(),
            })
        };
        if self.selected.is_empty() {
            return self.current().and_then(to_ref).into_iter().collect();
        }
        self.visible
            .iter()
            .filter_map(|&index| self.entries.get(index))
            .filter(|entry| self.selected.contains(&entry.selection_key()))
            .filter_map(to_ref)
            .fold(Vec::new(), |mut items, item| {
                if !items
                    .iter()
                    .any(|existing: &ops::TrashRef| existing.id == item.id)
                {
                    items.push(item);
                }
                items
            })
    }

    /// Visible items `a..=b`, clamped to the listing. The linewise range a vim
    /// operator works on: one row is one line.
    pub fn paths_in(&self, a: usize, b: usize) -> Vec<PathBuf> {
        if self.visible.is_empty() {
            return Vec::new();
        }
        let last = self.visible.len() - 1;
        (a.min(last)..=b.min(last))
            .map(|i| self.entries[self.visible[i]].path.clone())
            .collect()
    }

    pub fn counts(&self) -> PaneCounts {
        let (mut dir_count, mut file_count, mut total_bytes) = (0, 0, 0);
        for &entry_index in &self.visible {
            let e = &self.entries[entry_index];
            if e.is_dir() {
                dir_count += 1;
            } else {
                file_count += 1;
                total_bytes += e.size;
            }
        }
        PaneCounts {
            dirs: dir_count,
            files: file_count,
            bytes: total_bytes,
        }
    }
}

/// Which listing a streamed batch belongs to: the directory, and the pane
/// sequence number that asked for it.
#[derive(Clone, PartialEq, Eq, Hash)]
struct StreamKey {
    path: PathBuf,
    seq: u64,
}

/// When the last left click landed, and on which visible row.
#[derive(Clone, Copy)]
pub struct LastClick {
    pub at: Instant,
    pub visible_index: usize,
}

/// The last `df` answer, and when it was taken.
struct DiskCache {
    path: PathBuf,
    space: fs::DiskSpace,
    measured_at: Instant,
}

/// What the status bar reports about a pane's visible entries.
pub struct PaneCounts {
    pub dirs: usize,
    pub files: usize,
    pub bytes: u64,
}

pub struct Tab {
    pub panes: Vec<Pane>,
    pub active: usize,
}

impl Tab {
    pub fn new(cwd: PathBuf) -> Tab {
        Tab {
            panes: vec![Pane::new(cwd)],
            active: 0,
        }
    }

    pub fn pane(&self) -> &Pane {
        &self.panes[self.active]
    }

    pub fn pane_mut(&mut self) -> &mut Pane {
        &mut self.panes[self.active]
    }

    pub fn title(&self) -> String {
        ops::file_name_of(&self.pane().cwd)
    }
}

/// Rects captured during the last render, so mouse events hit-test against
/// what the user actually sees rather than against a recomputed guess.
#[derive(Default, Clone)]
pub struct Hitboxes {
    pub back: Rect,
    pub forward: Rect,
    /// The view button is two controls side by side, as in Dolphin: the icon
    /// cycles the mode, the caret opens the menu.
    pub view_cycle: Rect,
    pub view_menu: Rect,
    pub split: Rect,
    pub search: Rect,
    pub menu: Rect,
    pub crumbs: Vec<(Rect, PathBuf)>,
    pub crumb_arrows: Vec<(Rect, usize)>,
    pub path_bar: Rect,
    pub places: Rect,
    pub tabs: Vec<Rect>,
    pub headers: Vec<(Rect, SortKey)>,
    /// The popup currently on screen, for click-to-pick.
    pub menu_popup: Rect,
}

/// A drag in flight. Terminals have no native DnD, so we draw our own.
/// A cell position in the terminal grid.
#[derive(Clone, Copy)]
pub struct CellPos {
    pub x: u16,
    pub y: u16,
}

pub struct Drag {
    pub paths: Vec<PathBuf>,
    /// Stable identity of the source pane; active-pane state may change while
    /// the pointer crosses a split.
    pub source_pane_id: u64,
    pub position: CellPos,
    pub origin: CellPos,
    pub started: bool,
}

#[derive(Clone, Debug)]
pub enum Observation {
    PasteCommand {
        sources: Vec<PathBuf>,
        destination: PathBuf,
    },
    OperationStarted {
        id: u64,
        action: &'static str,
        destination: PathBuf,
        item_count: usize,
    },
    OperationFinished {
        id: u64,
        committed: usize,
        failed: usize,
        cancelled: bool,
    },
}

pub struct EditorState {
    pub root: PathBuf,
    pub layout: editor::Layout,
    pub selected_path: Option<PathBuf>,
    pub terminal_focused: bool,
    handle: editor::Handle,
    next_request_id: u64,
    latest_request: Option<(u64, PathBuf)>,
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub places: Vec<Row>,
    pub places_cursor: usize,
    pub places_visible: bool,
    places_checked_at: Instant,
    pub info_visible: bool,
    pub filter_bar: bool,
    pub focus: Focus,
    /// Region below the toolbar, restored by cancellation or `Ctrl+j`.
    pub toolbar_return: FocusRegion,
    pub mode: Mode,
    /// Buffer for whichever text-entry mode is active.
    pub input: String,
    pub input_cursor: usize,
    pub register: UnnamedRegister,
    pub undo: Vec<UndoOp>,
    pub status: String,
    pub status_is_error: bool,
    pub typeahead: String,
    pub typeahead_at: Option<Instant>,
    /// Pending vim state: count prefix and chord leader (`g`, `z`, `c`).
    pub count: String,
    pub pending_chord_leader: Option<char>,
    /// A `d` waiting for its motion, holding the exact count typed before it
    /// (`3d` -> `"3"`, a bare `d` -> `""`) so showcmd can echo the literal
    /// prefix. The count typed after the `d` still accumulates in `count`.
    pub pending_delete: Option<String>,
    /// An `m` or `'` waiting for the letter that names the mark.
    pub pending_mark: Option<MarkPending>,
    /// Vim's marks remember both the browsable target and its focused row.
    /// Kept for the session only, as vim's lowercase marks are kept per file.
    pub marks: HashMap<char, Location>,
    pub search_last: String,
    pub drag: Option<Drag>,
    pub hits: Hitboxes,
    pub menu_cursor: usize,
    /// Free/total bytes for the status bar, and when they were measured.
    disk: Option<DiskCache>,
    /// Last left-click (when, which item) — the double-click detector.
    pub last_click: Option<LastClick>,
    pub thumbs: Thumbs,
    pub active_transfer: Option<ActiveTransfer>,
    pub editor: Option<EditorState>,
    external_launches: Vec<Receiver<Result<(), String>>>,
    pub quit: bool,
    /// Set when something needs the terminal to itself; `main` hands it over.
    pub suspend: Option<Suspend>,
    lister: Lister,
    pub listing_rx: Receiver<fs::ListingMsg>,
    watcher: Watcher,
    /// Directory listings cached per pane `seq` sequence number, keyed while streaming.
    streaming: HashMap<StreamKey, Vec<Entry>>,
    observation_enabled: bool,
    observation_events: Vec<Observation>,
    next_operation_id: u64,
}

impl App {
    pub fn new(start: PathBuf) -> App {
        let (listing_tx, listing_rx) = channel();
        let lister = Lister::new(listing_tx);
        let watcher = Watcher::new();
        let mut app = App {
            tabs: vec![Tab::new(start.clone())],
            active_tab: 0,
            places: places::build(),
            places_cursor: 1,
            places_visible: true,
            places_checked_at: Instant::now(),
            info_visible: false,
            filter_bar: false,
            focus: Focus::View,
            toolbar_return: FocusRegion::View(0),
            mode: Mode::Normal,
            input: String::new(),
            input_cursor: 0,
            register: UnnamedRegister::default(),
            undo: Vec::new(),
            status: String::new(),
            status_is_error: false,
            typeahead: String::new(),
            typeahead_at: None,
            count: String::new(),
            pending_chord_leader: None,
            pending_delete: None,
            pending_mark: None,
            marks: HashMap::new(),
            search_last: String::new(),
            drag: None,
            hits: Hitboxes::default(),
            menu_cursor: 0,
            disk: None,
            last_click: None,
            thumbs: Thumbs::new(),
            active_transfer: None,
            editor: None,
            external_launches: Vec::new(),
            quit: false,
            suspend: None,
            lister,
            listing_rx,
            watcher,
            streaming: HashMap::new(),
            observation_enabled: false,
            observation_events: Vec::new(),
            next_operation_id: 1,
        };
        app.reload();
        app
    }

    pub fn enable_observation(&mut self) {
        self.observation_enabled = true;
    }

    pub fn enable_editor(&mut self, root: PathBuf, handle: editor::Handle) {
        let selected_path = self.pane().current().map(|entry| entry.path.clone());
        self.editor = Some(EditorState {
            root,
            layout: editor::Layout::Full,
            selected_path,
            terminal_focused: true,
            handle,
            next_request_id: 1,
            latest_request: None,
        });
    }

    pub fn integrated(&self) -> bool {
        self.editor.is_some()
    }

    pub fn editor_layout(&self) -> Option<editor::Layout> {
        self.editor.as_ref().map(|editor| editor.layout)
    }

    pub fn set_editor_layout(&mut self, layout: editor::Layout) {
        if let Some(editor) = &mut self.editor {
            editor.layout = layout;
        }
    }

    pub fn set_editor_terminal_focus(&mut self, focused: bool) {
        if let Some(editor) = &mut self.editor {
            editor.terminal_focused = focused;
        }
    }

    pub fn sync_editor_selection(&mut self) {
        let current = self.pane().current().map(|entry| entry.path.clone());
        if let (Some(editor), Some(current)) = (&mut self.editor, current) {
            editor.selected_path = Some(current);
        }
    }

    pub fn editor_opened(&mut self, id: u64) {
        let path = self.editor.as_mut().and_then(|editor| {
            if editor.latest_request.as_ref().map(|request| request.0) == Some(id) {
                editor.latest_request.take().map(|request| request.1)
            } else {
                None
            }
        });
        if let Some(path) = path {
            self.reveal_editor_path(path, true);
        }
    }

    pub fn reveal_editor_path(&mut self, path: PathBuf, own_open: bool) {
        let Some(editor) = &self.editor else { return };
        let root = editor.root.clone();
        if !path.starts_with(&root) && !own_open {
            self.error(format!(
                "Cannot reveal a path outside the integration root: {}",
                path.display()
            ));
            return;
        }
        let browse_root = if path.starts_with(&root) {
            root
        } else {
            path.parent().map(Path::to_path_buf).unwrap_or(root)
        };
        if !browse_root.is_dir() {
            self.error(format!("Cannot reveal {}", path.display()));
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.root = browse_root.clone();
            editor.selected_path = Some(path.clone());
        }
        self.goto_location(
            Location {
                target: Target::Dir(browse_root),
                focus: Some(EntryFocus {
                    selection_key: path.clone(),
                    path,
                    retry_missing: false,
                }),
            },
            false,
        );
    }

    pub fn reconcile_editor_selection(&mut self) {
        if self.pane().loading {
            return;
        }
        let Some(wanted) = self
            .editor
            .as_ref()
            .and_then(|editor| editor.selected_path.clone())
        else {
            return;
        };
        if self
            .pane()
            .visible
            .iter()
            .any(|&index| self.pane().entries[index].path == wanted)
        {
            self.select_by_path(&wanted);
            return;
        }
        let fallback = wanted
            .ancestors()
            .skip(1)
            .find(|ancestor| {
                self.pane()
                    .visible
                    .iter()
                    .any(|&index| self.pane().entries[index].path == *ancestor)
            })
            .map(Path::to_path_buf);
        if let Some(path) = fallback {
            self.select_by_path(&path);
            if let Some(editor) = &mut self.editor {
                editor.selected_path = Some(path);
            }
        }
    }

    pub fn observe_paste(&mut self, sources: Vec<PathBuf>, destination: PathBuf) {
        if self.observation_enabled {
            self.observation_events.push(Observation::PasteCommand {
                sources,
                destination,
            });
        }
    }

    pub fn take_observation_events(&mut self) -> Vec<Observation> {
        std::mem::take(&mut self.observation_events)
    }

    pub fn observation_events_finished(
        &mut self,
        id: u64,
        committed: usize,
        failed: usize,
        cancelled: bool,
    ) {
        self.observation_events
            .push(Observation::OperationFinished {
                id,
                committed,
                failed,
                cancelled,
            });
    }

    pub fn pending_listings(&self) -> usize {
        self.tabs
            .iter()
            .flat_map(|tab| &tab.panes)
            .filter(|pane| pane.loading)
            .count()
    }

    pub fn pending_refreshes(&self) -> usize {
        self.tabs
            .iter()
            .flat_map(|tab| &tab.panes)
            .filter(|pane| pane.pending_focus.is_some())
            .count()
    }

    // -- accessors ---------------------------------------------------------

    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }

    pub fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }

    pub fn pane(&self) -> &Pane {
        self.tab().pane()
    }

    pub fn pane_mut(&mut self) -> &mut Pane {
        self.tab_mut().pane_mut()
    }

    /// A pane of the current tab by index, for the renderer and the hit-tests,
    /// which work on both panes of a split rather than only the active one.
    pub fn pane_at(&self, i: usize) -> &Pane {
        &self.tabs[self.active_tab].panes[i]
    }

    pub fn pane_at_mut(&mut self, i: usize) -> &mut Pane {
        &mut self.tabs[self.active_tab].panes[i]
    }

    /// Start a worker together with the UI behavior its completion owns.
    pub fn begin_transfer(&mut self, progress: Progress, reveal: Option<RevealIntent>) {
        let selection_pane_id = self.pane().id;
        self.begin_transfer_from(progress, reveal, selection_pane_id);
    }

    pub fn begin_observed_transfer(
        &mut self,
        progress: Progress,
        reveal: Option<RevealIntent>,
        destination: PathBuf,
        item_count: usize,
    ) {
        let selection_pane_id = self.pane().id;
        let id = self.next_operation_id;
        self.next_operation_id += 1;
        let action = match progress.kind {
            ops::TransferKind::Copy => "copy",
            ops::TransferKind::Move => "move",
            ops::TransferKind::Restore => "restore",
        };
        if self.observation_enabled {
            self.observation_events.push(Observation::OperationStarted {
                id,
                action,
                destination,
                item_count,
            });
        }
        self.active_transfer = Some(ActiveTransfer {
            progress,
            observation_id: self.observation_enabled.then_some(id),
            reveal,
            selection_pane_id,
        });
    }

    /// Split-pane transfers have two owners: the source owns selection cleanup,
    /// while the destination owns refresh/reveal. Never reconstruct either from
    /// whichever pane happens to be active when the worker finishes.
    pub fn begin_transfer_from(
        &mut self,
        progress: Progress,
        reveal: Option<RevealIntent>,
        selection_pane_id: u64,
    ) {
        debug_assert!(self.active_transfer.is_none());
        self.active_transfer = Some(ActiveTransfer {
            progress,
            observation_id: None,
            reveal,
            selection_pane_id,
        });
    }

    /// Capture the destination pane and its view policy before an asynchronous
    /// operation starts. Indices are render-local; pane ids survive focus and
    /// tab changes while the worker runs.
    pub fn reveal_intent_for_pane(&self, pane_index: usize, directory: PathBuf) -> RevealIntent {
        let pane = self.pane_at(pane_index);
        let mode = if pane.view == ViewMode::Compact
            && directory != pane.cwd
            && directory.starts_with(&pane.cwd)
        {
            RevealMode::ExpandDirectory
        } else {
            RevealMode::NavigateDirectory
        };
        RevealIntent {
            directory,
            pane_id: pane.id,
            mode,
        }
    }

    /// Consume state for completed recursive operands in the pane that started
    /// the operation, rather than whichever split is active when it finishes.
    pub fn remove_operation_paths(
        &mut self,
        pane_id: u64,
        paths: &HashSet<PathBuf>,
        sources_removed: bool,
    ) {
        if let Some(pane) = self
            .tabs
            .iter_mut()
            .flat_map(|tab| tab.panes.iter_mut())
            .find(|pane| pane.id == pane_id)
        {
            pane.selected
                .retain(|key| !paths.iter().any(|path| key.starts_with(path)));
            if sources_removed {
                pane.expanded
                    .retain(|key| !paths.iter().any(|path| key.starts_with(path)));
            }
        }
    }

    /// Reveal a newly completed path in the pane that started the operation,
    /// even if another pane became active while filesystem events were arriving.
    pub fn reveal_completed(&mut self, intent: RevealIntent, path: PathBuf) {
        let owner = self.tabs.iter().enumerate().find_map(|(tab_index, tab)| {
            tab.panes
                .iter()
                .position(|pane| pane.id == intent.pane_id)
                .map(|pane_index| (tab_index, pane_index))
        });
        let Some((tab_index, pane_index)) = owner else {
            self.error("The pane that requested creation was closed");
            return;
        };
        self.active_tab = tab_index;
        self.tabs[tab_index].active = pane_index;
        let can_expand_in_place = intent.mode == RevealMode::ExpandDirectory
            && intent.directory != self.pane().cwd
            && intent.directory.starts_with(&self.pane().cwd);
        if can_expand_in_place {
            let pane = self.pane_mut();
            pane.expanded.insert(intent.directory);
            pane.focus_after_refresh(path);
            self.refresh_in_place();
        } else if self.pane().cwd != intent.directory {
            self.goto(Target::Dir(intent.directory.clone()), true);
            self.pane_mut().focus_after_refresh(path);
        } else {
            self.pane_mut().focus_after_refresh(path);
            self.refresh_in_place();
        }
    }

    pub fn split_on(&self) -> bool {
        self.tab().panes.len() > 1
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_is_error = false;
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        self.status = msg.into();
        self.status_is_error = true;
    }

    /// End a visual range and discard its transient selection. Visual ranges
    /// are mode-owned; callers must copy their paths before leaving.
    pub fn leave_visual(&mut self) {
        if self.mode.is_visual() {
            self.mode = Mode::Normal;
            self.pane_mut().selected.clear();
        }
    }

    // -- loading -----------------------------------------------------------

    /// Ask the worker for the active pane's directory.
    pub fn reload(&mut self) {
        let seq = self.pane().seq + 1;
        let target = self.pane().target.clone();
        {
            let pane = self.pane_mut();
            pane.seq = seq;
            pane.loading = true;
            pane.error = None;
        }
        match target {
            Target::Dir(ref d) => {
                self.watcher.watch(d);
                self.lister.request(d.clone(), seq);
            }
            Target::Trash => {
                let entries = ops::list_trash();
                self.apply_listing(seq, entries, None);
            }
            Target::TrashDir {
                ref original,
                ref backing,
                ..
            } => {
                let (entries, error) = match fs::read_dir_as(backing, original, 0, true) {
                    Ok(entries) => (entries, None),
                    Err(error) => (Vec::new(), Some(error.to_string())),
                };
                self.apply_listing(seq, entries, error);
            }
            Target::Network => {
                self.apply_listing(
                    seq,
                    Vec::new(),
                    Some("Network browsing is not implemented".into()),
                );
            }
            Target::RecentDays(days) => {
                let entries = recent(&places::home(), days);
                self.apply_listing(seq, entries, None);
            }
        }
    }

    fn apply_listing(&mut self, seq: u64, entries: Vec<Entry>, err: Option<String>) {
        let pane = self.pane_mut();
        if pane.seq != seq {
            return;
        }
        pane.error = err;
        pane.loading = false;
        pane.set_entries(entries);
        pane.offset = 0;
    }

    /// Drain worker messages. Called once per event-loop tick.
    pub fn pump_fs_events(&mut self) -> bool {
        let listing_messages: Vec<fs::ListingMsg> = self.listing_rx.try_iter().collect();
        let mut changed = !listing_messages.is_empty();
        for message in listing_messages {
            match message {
                fs::ListingMsg::Batch { path, seq, entries } => {
                    self.streaming
                        .entry(StreamKey { path, seq })
                        .or_default()
                        .extend(entries);
                }
                fs::ListingMsg::Done { path, seq } => {
                    let entries = self
                        .streaming
                        .remove(&StreamKey {
                            path: path.clone(),
                            seq,
                        })
                        .unwrap_or_default();
                    // A pane other than the active one may own this `seq`.
                    self.deliver_listing(&path, seq, entries, None);
                }
                fs::ListingMsg::Listed(listing) => {
                    self.deliver_listing(
                        &listing.path.clone(),
                        listing.seq,
                        listing.entries,
                        listing.error,
                    );
                }
            }
        }
        if self.watcher.take_dirty() {
            self.refresh_in_place();
            changed = true;
        }
        let live_seqs: HashSet<u64> = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter())
            .map(|pane| pane.seq)
            .collect();
        self.streaming.retain(|key, _| live_seqs.contains(&key.seq));
        changed
    }

    fn deliver_listing(&mut self, path: &Path, seq: u64, entries: Vec<Entry>, err: Option<String>) {
        for tab in &mut self.tabs {
            for pane in &mut tab.panes {
                if pane.seq == seq && pane.cwd == path {
                    pane.error = err.clone();
                    pane.loading = false;
                    pane.set_entries(entries.clone());
                }
            }
        }
    }

    /// Relist a particular directory pane without changing tab or split focus.
    /// Completion reducers use this for an operation's source pane; the global
    /// watcher only follows one directory and therefore cannot make split-pane
    /// consistency dependably converge on its own.
    pub fn refresh_pane_in_place(&mut self, pane_id: u64) {
        let request = self
            .tabs
            .iter_mut()
            .flat_map(|tab| tab.panes.iter_mut())
            .find(|pane| pane.id == pane_id)
            .and_then(|pane| match &pane.target {
                Target::Dir(directory) => {
                    pane.seq += 1;
                    pane.loading = true;
                    Some((directory.clone(), pane.seq))
                }
                _ => None,
            });
        if let Some((directory, seq)) = request {
            self.lister.request(directory, seq);
        }
    }

    /// inotify said something changed: relist without moving the cursor.
    pub fn refresh_in_place(&mut self) {
        let target = self.pane().target.clone();
        if let Target::Dir(d) = target {
            let seq = self.pane().seq + 1;
            self.pane_mut().seq = seq;
            self.pane_mut().loading = true;
            self.lister.request(d, seq);
        } else {
            self.reload();
        }
    }

    // -- navigation --------------------------------------------------------

    pub fn location(&self) -> Location {
        Location {
            target: self.pane().target.clone(),
            focus: self.pane().current().map(EntryFocus::new),
        }
    }

    pub fn goto(&mut self, target: Target, push_history: bool) {
        self.goto_location(
            Location {
                target,
                focus: None,
            },
            push_history,
        );
    }

    /// Navigate and restore a stable row as one intent. Listings are
    /// asynchronous, so the focus must be installed before requesting one.
    pub fn goto_location(&mut self, location: Location, push_history: bool) {
        let target = location.target;
        let focus = location.focus;
        let cwd = match &target {
            Target::Dir(d) => d.clone(),
            Target::Trash => PathBuf::from("trash:/"),
            Target::TrashDir { original, .. } => original.clone(),
            Target::Network => PathBuf::from("remote:/"),
            Target::RecentDays(1) => PathBuf::from("recent:/today"),
            Target::RecentDays(_) => PathBuf::from("recent:/yesterday"),
        };
        if let Target::Dir(d) = &target {
            if !d.is_dir() {
                self.error(format!("Not a directory: {}", d.display()));
                return;
            }
        }
        if self.pane().target == target {
            if let Some(focus) = focus {
                self.pane_mut().focus_now(focus);
                return;
            }
        }
        {
            let pane = self.pane_mut();
            if push_history && pane.target != target {
                pane.history.truncate(pane.history_pos + 1);
                pane.history.push(target.clone());
                pane.history_pos = pane.history.len() - 1;
            }
            pane.cwd = cwd;
            pane.target = target.clone();
            pane.cursor = 0;
            pane.offset = 0;
            pane.selected.clear();
            pane.filter.clear();
            pane.expanded.clear();
            if let Some(focus) = focus {
                pane.reveal_ancestors(&focus);
                pane.pending_focus = Some(focus);
            } else {
                pane.pending_focus = None;
            }
        }
        if let Some(i) = places::index_of(&self.places, &target) {
            self.places_cursor = i;
        }
        self.reload();
    }

    pub fn open_dir(&mut self, d: PathBuf) {
        self.goto(Target::Dir(d), true);
    }

    /// Open a Places row, mounting an offline block device first.
    pub fn open_place_index(&mut self, index: usize) {
        let Some(Row::Item {
            target,
            offline,
            device,
            ..
        }) = self.places.get(index).cloned()
        else {
            return;
        };
        if offline {
            let Some(device) = device else { return };
            self.suspend = Some(Suspend::Mount(device));
        } else {
            self.open_place(target);
        }
    }

    /// Unmount a removable device from its Places-row affordance.
    pub fn eject_place_index(&mut self, index: usize) {
        let Some(Row::Item {
            eject: true,
            device: Some(device),
            ..
        }) = self.places.get(index).cloned()
        else {
            return;
        };
        self.suspend = Some(Suspend::Unmount(device));
    }

    /// Poll lsblk sparingly so hotplugged and removed media update live.
    pub fn refresh_places(&mut self) -> bool {
        if self.places_checked_at.elapsed() < Duration::from_secs(1) {
            return false;
        }
        self.places_checked_at = Instant::now();
        let rows = places::build();
        if rows == self.places {
            return false;
        }
        self.places = rows;
        self.places_cursor = self.places_cursor.min(self.places.len().saturating_sub(1));
        true
    }

    pub fn open_place(&mut self, target: Target) {
        let pane = self.pane_mut();
        if pane.place == target {
            self.goto(target, true);
            return;
        }

        if let Some(saved) = pane
            .place_histories
            .iter_mut()
            .find(|(place, _, _)| *place == pane.place)
        {
            saved.1 = pane.history.clone();
            saved.2 = pane.history_pos;
        } else {
            pane.place_histories
                .push((pane.place.clone(), pane.history.clone(), pane.history_pos));
        }
        let restored = pane
            .place_histories
            .iter()
            .find(|(place, _, _)| *place == target)
            .map(|(_, history, pos)| (history.clone(), *pos));
        pane.place = target.clone();
        if let Some((history, pos)) = restored {
            pane.history = history;
            pane.history_pos = pos;
        } else {
            pane.history = vec![target.clone()];
            pane.history_pos = 0;
        }
        self.goto(target, true);
    }

    /// Navigate a rendered breadcrumb without turning a virtual Trash path
    /// into a live filesystem directory.
    pub fn open_breadcrumb(&mut self, path: PathBuf) {
        let Target::TrashDir {
            display,
            original,
            backing,
        } = self.pane().target.clone()
        else {
            self.goto(Target::Dir(path), true);
            return;
        };

        if path.as_os_str().is_empty() || path == Path::new("trash:") {
            self.goto(Target::Trash, true);
            return;
        }
        let Ok(suffix) = display.strip_prefix(&path) else {
            self.error("Invalid Trash breadcrumb");
            return;
        };
        let levels = suffix.components().count();
        let ancestor = |mut path: PathBuf| {
            for _ in 0..levels {
                path.pop();
            }
            path
        };
        self.goto(
            Target::TrashDir {
                display: path,
                original: ancestor(original),
                backing: ancestor(backing),
            },
            true,
        );
    }

    pub fn go_up(&mut self) {
        if matches!(self.pane().target, Target::TrashDir { .. }) {
            self.back();
            return;
        }
        let cwd = self.pane().cwd.clone();
        if let Some(parent) = cwd.parent().map(Path::to_path_buf) {
            self.open_dir(parent.clone());
            // Dolphin leaves the cursor on the directory you came out of.
            self.select_by_path(&cwd);
        }
    }

    pub fn back_or_up(&mut self) {
        if self.pane().history_pos > 0 {
            self.back();
            return;
        }

        let Target::Dir(cwd) = self.pane().target.clone() else {
            self.go_up();
            return;
        };
        let Some(parent) = cwd.parent().map(Path::to_path_buf) else {
            return;
        };
        let target = Target::Dir(parent);
        self.goto(target.clone(), false);
        self.select_by_path(&cwd);
        let pane = self.pane_mut();
        pane.history.insert(0, target);
        pane.history_pos = 0;
    }

    pub fn back(&mut self) {
        let pane = self.pane();
        if pane.history_pos == 0 {
            return;
        }
        let pos = pane.history_pos - 1;
        let target = pane.history[pos].clone();
        self.pane_mut().history_pos = pos;
        self.goto(target, false);
    }

    pub fn forward(&mut self) {
        let pane = self.pane();
        if pane.history_pos + 1 >= pane.history.len() {
            return;
        }
        let pos = pane.history_pos + 1;
        let target = pane.history[pos].clone();
        self.pane_mut().history_pos = pos;
        self.goto(target, false);
    }

    /// Put the cursor on `path` once it appears; used after `go_up` and rename.
    pub fn select_by_path(&mut self, path: &Path) {
        let pane = self.pane_mut();
        if let Some(i) = pane
            .visible
            .iter()
            .position(|&entry_index| pane.entries[entry_index].path == path)
        {
            pane.cursor = i;
        }
    }

    /// Enter, `l`, or double-click.
    pub fn activate(&mut self) {
        let Some(entry) = self.pane().current().cloned() else {
            return;
        };
        if matches!(self.pane().target, Target::Trash | Target::TrashDir { .. }) {
            if entry.is_dir() {
                let Some(backing) = entry.backing_path else {
                    self.error(format!("Cannot browse {} in Trash", entry.name));
                    return;
                };
                let display = self.pane().display_path().join(&entry.name);
                self.goto(
                    Target::TrashDir {
                        display,
                        original: entry.path,
                        backing,
                    },
                    true,
                );
            } else if self.editor.is_some() {
                self.open_in_editor(entry.filesystem_path().to_path_buf());
            } else {
                self.open_external(entry.filesystem_path());
            }
        } else if entry.is_dir() {
            self.open_dir(entry.path);
        } else if self.editor.is_some() {
            self.open_in_editor(entry.path);
        } else {
            self.open_external(&entry.path);
        }
    }

    fn open_in_editor(&mut self, path: PathBuf) {
        let Some(editor) = &mut self.editor else {
            return;
        };
        editor.selected_path = Some(path.clone());
        let id = editor.next_request_id;
        editor.next_request_id += 1;
        editor.latest_request = Some((id, path.clone()));
        match editor.handle.open(id, &path) {
            Ok(()) => self.info(format!("Opening {}", ops::file_name_of(&path))),
            Err(error) => self.error(error),
        }
    }

    pub fn open_terminal_here(&mut self) {
        let dir = self.pane().cwd.clone();
        match open::spawn_terminal(&dir) {
            Ok(()) => self.info(format!("Opened terminal in {}", dir.display())),
            Err(error) => self.error(error),
        }
    }

    pub fn open_external(&mut self, path: &Path) {
        let plan = match open::resolve(path) {
            Ok(plan) => plan,
            Err(error) => {
                self.error(error.to_string());
                return;
            }
        };
        let message = format!("Opening {}", ops::file_name_of(path));
        if plan.needs_terminal() {
            // A TUI handler cannot share our raw alternate screen. Main owns
            // terminal lifecycle, so pass it the already-resolved command.
            self.info(message);
            self.suspend = Some(Suspend::Open(plan));
        } else {
            match plan.spawn_detached() {
                Ok(result_rx) => {
                    self.external_launches.push(result_rx);
                    self.info(message);
                }
                Err(error) => self.error(error),
            }
        }
    }

    /// Collect failures from detached launchers without blocking the UI.
    pub fn pump_external_launches(&mut self) -> bool {
        let mut failures = Vec::new();
        self.external_launches
            .retain(|receiver| match receiver.try_recv() {
                Ok(Ok(())) => false,
                Ok(Err(error)) => {
                    failures.push(error);
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
            });
        if let Some(error) = failures.pop() {
            self.error(error);
            true
        } else {
            false
        }
    }

    // -- cursor motion -----------------------------------------------------

    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        let is_linewise = self.mode == Mode::VisualLine;
        let is_blockwise = self.mode == Mode::VisualBlock;
        let pane = self.pane_mut();
        if pane.visible.is_empty() {
            return;
        }
        let last = pane.visible.len() as isize - 1;
        let next = (pane.cursor as isize + delta).clamp(0, last) as usize;
        pane.cursor = next;
        if extend {
            let indices = if is_blockwise {
                let cols = pane.grid_cols.max(1) as usize;
                let (anchor_row, anchor_col) = (pane.anchor / cols, pane.anchor % cols);
                let (cursor_row, cursor_col) = (next / cols, next % cols);
                let mut block = Vec::new();
                for row in anchor_row.min(cursor_row)..=anchor_row.max(cursor_row) {
                    for col in anchor_col.min(cursor_col)..=anchor_col.max(cursor_col) {
                        let index = row * cols + col;
                        if index < pane.visible.len() {
                            block.push(index);
                        }
                    }
                }
                block
            } else {
                let (mut range_start, mut range_end) =
                    (pane.anchor.min(next), pane.anchor.max(next));
                // Linewise: grow the range out to the selection-line edges. This
                // is not always the navigation stride; Compact stores whole columns
                // contiguously, while its vertically walked file lines are singular.
                if is_linewise {
                    let width = pane.linewise_width();
                    range_start -= range_start % width;
                    range_end = (range_end - range_end % width + width - 1).min(last as usize);
                }
                (range_start..=range_end).collect()
            };
            let range: Vec<PathBuf> = indices
                .into_iter()
                .filter_map(|visible_index| {
                    pane.entry_at(visible_index)
                        .map(|entry| entry.selection_key())
                })
                .collect();
            pane.selected.clear();
            pane.selected.extend(range);
        }
    }

    /// A step along the line the index runs down — the row in Icons, the column
    /// in Compact. It stops at the end of the line instead of spilling into the
    /// next one, the way `l` stops at the end of a line in vim.
    pub fn step_along(&mut self, step: isize, extend: bool) {
        let stride = self.pane().stride() as isize;
        let cursor_index = self.pane().cursor as isize;
        let row_start = cursor_index - cursor_index % stride;
        let next = (cursor_index + step).clamp(row_start, row_start + stride - 1);
        self.move_cursor(next - cursor_index, extend);
    }

    /// A step across lines, keeping the position along the line. A step past
    /// the first or last line stays where it is, as `k` does on vim's first
    /// line — clamping the raw index instead would slide sideways to item 0.
    /// A short final line is landed on at its end, which vim also does.
    pub fn step_across(&mut self, step: isize, extend: bool) {
        let stride = self.pane().stride() as isize;
        let cursor_index = self.pane().cursor as isize;
        let last = self.pane().len() as isize - 1;
        let target_row = cursor_index / stride + step;
        if last < 0 || target_row < 0 || target_row > last / stride {
            return;
        }
        let next = (target_row * stride + cursor_index % stride).min(last);
        self.move_cursor(next - cursor_index, extend);
    }

    pub fn goto_index(&mut self, i: usize, extend: bool) {
        let cur = self.pane().cursor as isize;
        self.move_cursor(i as isize - cur, extend);
    }

    pub fn toggle_select(&mut self) {
        let Some(entry) = self.pane().current().cloned() else {
            return;
        };
        let pane = self.pane_mut();
        let key = entry.selection_key();
        if !pane.selected.remove(&key) {
            pane.selected.insert(key);
        }
    }

    pub fn select_all(&mut self) {
        let pane = self.pane_mut();
        pane.selected = pane
            .visible
            .iter()
            .map(|&entry_index| pane.entries[entry_index].selection_key())
            .collect();
    }

    pub fn invert_selection(&mut self) {
        let pane = self.pane_mut();
        let all: HashSet<PathBuf> = pane
            .visible
            .iter()
            .map(|&entry_index| pane.entries[entry_index].selection_key())
            .collect();
        pane.selected = all.difference(&pane.selected).cloned().collect();
    }

    // -- type-ahead --------------------------------------------------------

    pub fn typeahead(&mut self, c: char) {
        let now = Instant::now();
        let stale = self
            .typeahead_at
            .map(|t| now.duration_since(t).as_millis() > config::TYPEAHEAD_TIMEOUT_MS as u128)
            .unwrap_or(true);
        if stale {
            self.typeahead.clear();
        }
        self.typeahead.push(c.to_ascii_lowercase());
        self.typeahead_at = Some(now);
        let needle = self.typeahead.clone();
        let start = self.pane().cursor;
        let n = self.pane().visible.len();
        if n == 0 {
            return;
        }
        // Search forward from the cursor, wrapping — Dolphin's behaviour.
        let from = if needle.len() == 1 { start + 1 } else { start };
        for k in 0..n {
            let i = (from + k) % n;
            let name_matches = self
                .pane()
                .entry_at(i)
                .is_some_and(|e| e.name.to_lowercase().starts_with(&needle));
            if name_matches {
                self.pane_mut().cursor = i;
                return;
            }
        }
    }

    /// The incomplete Normal-mode command the user has half-typed, like Vim's
    /// `showcmd`. Everything still waiting on a further key joins the string:
    /// the count prefix, a `d` awaiting its motion, a chord leader (`g`, `z`,
    /// `c`) awaiting a follower, or a mark operator (`m`, `'`) awaiting its
    /// name. Empty when no key is pending, so the status bar falls through to
    /// the disk-free readout.
    pub fn pending_command(&self) -> String {
        let mut s = String::new();
        // A `d` armed with the literal count typed before it (`` for a bare
        // `d`, `"1"` for `1d`, `"3"` for `3d`), echoed verbatim; the trailing
        // count typed after the operator joins it as `d5` does.
        if let Some(before) = &self.pending_delete {
            s.push_str(before);
            s.push('d');
            s.push_str(&self.count);
            return s;
        }
        // A chord leader (`g`, `z`, `c`, Space) awaiting its follower; the
        // count typed before it stays in `count`. A Space leader is literal
        // whitespace, so it reads as `<Space>` rather than invisible.
        if let Some(leader) = self.pending_chord_leader {
            s.push_str(&self.count);
            if leader == ' ' {
                s.push_str("<Space>");
            } else {
                s.push(leader);
            }
            return s;
        }
        // A mark operator (`m`, `'`) awaiting its name, with the count that
        // preceded it — marks never consume it, so it reads as `5m`.
        if let Some(mark) = self.pending_mark {
            s.push_str(&self.count);
            s.push(match mark {
                MarkPending::Set => 'm',
                MarkPending::Jump => '\'',
            });
            return s;
        }
        self.count.clone()
    }

    // -- view controls -----------------------------------------------------

    pub fn set_view(&mut self, v: ViewMode) {
        if self.mode == Mode::VisualBlock && v != ViewMode::Icons {
            self.leave_visual();
            self.info("Visual block selection ended: it is only available in Icons mode");
        }
        self.pane_mut().view = v;
    }

    /// Free and total bytes on the filesystem holding the current directory.
    ///
    /// Cached: this shells out to `df`, and the status bar asks once per frame.
    /// Spawning a process twenty-five times a second to redraw a number that
    /// changes by the minute is bad on its own, and it was worse than that —
    /// `df` opens the directory, inotify reports the open, and the watcher
    /// treated our own status bar as a reason to relist. See docs/DECISIONS.md.
    pub fn disk_space(&mut self) -> Option<fs::DiskSpace> {
        let cwd = self.pane().cwd.clone();
        let cache_is_fresh = self.disk.as_ref().is_some_and(|cache| {
            cache.path == cwd
                && cache.measured_at.elapsed()
                    < std::time::Duration::from_millis(config::DISK_POLL_MS)
        });
        if !cache_is_fresh {
            let space = fs::disk_space(&cwd)?;
            self.disk = Some(DiskCache {
                path: cwd,
                space,
                measured_at: Instant::now(),
            });
        }
        self.disk.as_ref().map(|cache| cache.space)
    }

    pub fn toggle_hidden(&mut self) {
        let pane = self.pane_mut();
        pane.show_hidden = !pane.show_hidden;
        pane.refilter();
    }

    pub fn set_sort(&mut self, key: SortKey) {
        let pane = self.pane_mut();
        if pane.sort.key == key {
            pane.sort.reverse = !pane.sort.reverse;
        } else {
            pane.sort.key = key;
            pane.sort.reverse = false;
        }
        pane.refilter();
    }

    pub fn toggle_split(&mut self) {
        let tab = self.tab_mut();
        if tab.panes.len() > 1 {
            tab.panes.truncate(1);
            tab.active = 0;
        } else {
            let mut second_pane = Pane::new(tab.panes[0].cwd.clone());
            second_pane.view = tab.panes[0].view;
            second_pane.sort = tab.panes[0].sort;
            second_pane.show_hidden = tab.panes[0].show_hidden;
            tab.panes.push(second_pane);
            tab.active = 1;
        }
        self.reload();
        self.repair_focus();
    }

    /// Repair body and toolbar-return focus after a dynamic layout change.
    pub fn repair_focus(&mut self) {
        if self.focus == Focus::Places && !self.places_visible {
            self.focus = Focus::View;
            self.tab_mut().active = 0;
        }
        if self.focus == Focus::Tabs && self.tabs.len() < 2 {
            self.focus = Focus::View;
        }
        let split_on = self.split_on();
        if !split_on && self.tab().active > 0 {
            self.tab_mut().active = 0;
        }
        self.toolbar_return = match self.toolbar_return {
            FocusRegion::Places if !self.places_visible => FocusRegion::View(0),
            FocusRegion::Tabs if self.tabs.len() < 2 => {
                FocusRegion::View(self.tab().active.min(usize::from(split_on)))
            }
            FocusRegion::View(1) if !split_on => FocusRegion::View(0),
            region if region.is_toolbar() => FocusRegion::View(self.tab().active),
            region => region,
        };
    }

    pub fn other_pane(&mut self) {
        let tab = self.tab_mut();
        if tab.panes.len() > 1 {
            tab.active = 1 - tab.active;
        }
    }

    // -- tabs --------------------------------------------------------------

    pub fn new_tab(&mut self, dir: PathBuf) {
        self.tabs.push(Tab::new(dir));
        self.active_tab = self.tabs.len() - 1;
        self.reload();
    }

    /// Closing the last tab quits, the way `:q` on vim's last window does.
    pub fn close_tab(&mut self) {
        if self.tabs.len() == 1 {
            self.quit = true;
            return;
        }
        self.tabs.remove(self.active_tab);
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        self.repair_focus();
    }

    pub fn cycle_tab(&mut self, delta: isize) {
        let n = self.tabs.len() as isize;
        self.active_tab = ((self.active_tab as isize + delta).rem_euclid(n)) as usize;
    }

    // -- details tree ------------------------------------------------------

    /// Toggle the innermost fold at the cursor (`za`). A folder row starts its
    /// own fold, while any other row belongs to its nearest expanded ancestor.
    /// Closing from a descendant moves the cursor to that folder's row and
    /// preserves deeper fold state, as a one-level Vim fold toggle does.
    pub fn toggle_expand(&mut self) {
        let Some(entry) = self.pane().current().cloned() else {
            return;
        };
        if entry.is_dir() {
            if self.pane().expanded.contains(&entry.path) {
                self.close_fold(false);
            } else {
                self.open_fold(false);
            }
            return;
        }

        // `depth` bounds this walk to represented tree ancestors: the pane's
        // cwd is not itself a visible fold, even if a stale key exists for it.
        let containing_fold = entry
            .path
            .ancestors()
            .skip(1)
            .take(entry.depth as usize)
            .find(|path| self.pane().expanded.contains(*path))
            .map(Path::to_path_buf);
        let Some(containing_fold) = containing_fold else {
            return;
        };
        let pane = self.pane_mut();
        pane.expanded.remove(&containing_fold);
        pane.refilter_keeping(Some(EntryFocus {
            selection_key: containing_fold.clone(),
            path: containing_fold,
            retry_missing: false,
        }));
    }

    /// Open the folder under the cursor, optionally including every folder
    /// beneath it (`zo`/`zO`).
    pub fn open_fold(&mut self, recursive: bool) {
        let Some(entry) = self.pane().current().cloned() else {
            return;
        };
        if !entry.is_dir() {
            return;
        }

        if recursive {
            let (folders, first_error) = recursive_folders(vec![entry.clone()]);
            let pane = self.pane_mut();
            pane.expanded.extend(folders);
            pane.refilter();
            if let Some((path, error)) = first_error {
                self.error(format!("Cannot read {}: {error}", path.display()));
            }
        } else {
            if let Err(error) = fs::read_dir_as(
                entry.filesystem_path(),
                &entry.path,
                entry.depth + 1,
                entry.backing_path.is_some(),
            ) {
                self.error(format!("Cannot read {}: {error}", entry.name));
                return;
            }
            let pane = self.pane_mut();
            pane.expanded.insert(entry.path);
            pane.refilter();
        }
    }

    /// Close the folder under the cursor. `zc` closes only that fold and keeps
    /// nested state; `zC` forgets every descendant fold too.
    pub fn close_fold(&mut self, recursive: bool) {
        let Some(entry) = self.pane().current().cloned() else {
            return;
        };
        if !entry.is_dir() || !self.pane().expanded.contains(&entry.path) {
            return;
        }
        let pane = self.pane_mut();
        if recursive {
            pane.expanded.retain(|path| !path.starts_with(&entry.path));
        } else {
            pane.expanded.remove(&entry.path);
        }
        pane.refilter();
    }

    /// Close every fold in the active pane (`zM`).
    pub fn close_all_folds(&mut self) {
        let pane = self.pane_mut();
        pane.expanded.clear();
        pane.refilter();
    }

    /// Open every folder reachable from this pane's roots (`zR`). Canonical
    /// directory identities prevent symlink cycles from making this traversal
    /// unbounded.
    pub fn open_all_folds(&mut self) {
        let roots = self
            .pane()
            .entries
            .iter()
            .filter(|entry| entry.depth == 0 && entry.is_dir())
            .cloned()
            .collect();
        let (folders, first_error) = recursive_folders(roots);
        let pane = self.pane_mut();
        pane.expanded.extend(folders);
        pane.refilter();
        if let Some((path, error)) = first_error {
            self.error(format!("Cannot read {}: {error}", path.display()));
        }
    }
}

/// Files under `root` modified within `days`, shallow-recursive like Dolphin's
/// baloo-free fallback. Depth is capped so `Recent` cannot walk a whole disk.
fn recent(root: &Path, days: u32) -> Vec<Entry> {
    let cutoff = fs::now_epoch() - (days as i64) * 86400;
    let mut out = Vec::new();
    let mut queue = vec![(root.to_path_buf(), 0u32)];
    while let Some((dir, depth)) = queue.pop() {
        if depth > config::RECENT_MAX_DEPTH || out.len() > config::RECENT_MAX_ITEMS {
            break;
        }
        let Ok(kids) = fs::read_dir(&dir, 0) else {
            continue;
        };
        for entry in kids {
            if entry.hidden {
                continue;
            }
            if entry.is_dir() {
                queue.push((entry.path.clone(), depth + 1));
            } else if entry.mtime >= cutoff {
                out.push(entry);
            }
        }
    }
    out.sort_by_key(|e| std::cmp::Reverse(e.mtime));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_with(names: &[&str]) -> Pane {
        let mut pane = Pane::new(PathBuf::from("/tmp"));
        pane.entries = entries_named(names);
        pane.refilter();
        pane
    }

    /// In the order given, unsorted — a fresh listing arrives in readdir order.
    fn entries_named(names: &[&str]) -> Vec<Entry> {
        names
            .iter()
            .map(|name| Entry {
                name: (*name).into(),
                path: PathBuf::from("/tmp").join(name),
                backing_path: None,
                link_target: None,
                kind: fs::Kind::File,
                size: 0,
                mtime: 0,
                mode: 0,
                readable: true,
                hidden: name.starts_with('.'),
                trash_id: None,
                depth: 0,
                expanded: false,
            })
            .collect()
    }

    #[test]
    fn center_cursor_uses_each_views_scroll_axis() {
        let mut pane = pane_with(&[]);
        pane.cursor = 12;
        pane.grid_rows = 5;

        pane.view = ViewMode::Details;
        pane.center_cursor();
        assert_eq!(pane.offset, 10);

        pane.view = ViewMode::Icons;
        pane.grid_cols = 3;
        pane.center_cursor();
        assert_eq!(pane.offset, 2);

        pane.view = ViewMode::Compact;
        pane.compact_width_avail = 30;
        pane.compact_widths = vec![10; 6];
        pane.center_cursor();
        assert_eq!(pane.offset, 1);
    }

    #[test]
    fn places_keep_independent_navigation_histories() {
        let root = std::env::temp_dir().join(format!(
            "dolvim-place-history-{}",
            NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let home = root.join("home");
        let home_child = home.join("child");
        let documents = root.join("documents");
        let documents_child = documents.join("child");
        std::fs::create_dir_all(&home_child).unwrap();
        std::fs::create_dir_all(&documents_child).unwrap();

        let mut app = App::new(home.clone());
        app.open_dir(home_child.clone());
        app.open_place(Target::Dir(documents.clone()));
        app.open_dir(documents_child);
        app.back();
        assert_eq!(app.pane().cwd, documents);
        app.back();
        assert_eq!(app.pane().cwd, documents, "history escaped into Home");

        app.open_place(Target::Dir(home));
        app.back();
        assert_eq!(app.pane().cwd, home_child, "Home history was not restored");

        drop(app);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A refresh delivers the listing in readdir order, which is nothing like
    /// the sorted order `visible` was built from. Reading the cursor's path
    /// after the swap therefore picks a random file, and the cursor jumps to it
    /// on every inotify tick.
    #[test]
    fn a_refresh_leaves_the_cursor_on_the_same_file() {
        let mut pane = pane_with(&["a", "b", "c", "d"]);
        pane.cursor = 2; // "c"
        pane.set_entries(entries_named(&["d", "b", "a", "c"]));
        assert_eq!(pane.current().unwrap().name, "c");
    }

    #[test]
    fn hidden_files_are_filtered_until_asked_for() {
        let mut pane = pane_with(&["a", ".b", "c"]);
        assert_eq!(pane.len(), 2);
        pane.show_hidden = true;
        pane.refilter();
        assert_eq!(pane.len(), 3);
    }

    #[test]
    fn filter_matches_substring_case_insensitively() {
        let mut pane = pane_with(&["Alpha", "beta", "GAMMA"]);
        pane.filter = "a".into();
        pane.refilter();
        assert_eq!(pane.len(), 3);
        pane.filter = "mm".into();
        pane.refilter();
        assert_eq!(pane.len(), 1);
    }

    #[test]
    fn refilter_keeps_the_cursor_on_the_same_file() {
        let mut pane = pane_with(&["a", "b", "c"]);
        pane.cursor = 2;
        let before = pane.current().unwrap().name.clone();
        pane.show_hidden = true;
        pane.refilter();
        assert_eq!(pane.current().unwrap().name, before);
    }

    #[test]
    fn refresh_rebuilds_the_expanded_tree() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-expand-{unique}"));
        let folder = base.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("first.txt"), b"first").unwrap();

        let mut pane = Pane::new(base.clone());
        pane.expanded.insert(folder.clone());
        pane.set_entries(fs::read_dir(&base, 0).unwrap());
        assert!(pane.entries.iter().any(|entry| entry.name == "first.txt"));

        std::fs::write(folder.join("second.txt"), b"second").unwrap();
        pane.set_entries(fs::read_dir(&base, 0).unwrap());
        assert!(pane.expanded.contains(&folder));
        assert!(pane.entries.iter().any(|entry| entry.name == "first.txt"));
        assert!(pane.entries.iter().any(|entry| entry.name == "second.txt"));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn duplicate_trash_paths_keep_distinct_selection_identity() {
        let path = PathBuf::from("/tmp/same.txt");
        let mut first = entries_named(&["same.txt"]).remove(0);
        first.path = path.clone();
        first.trash_id = Some("generation-1".into());
        let mut second = first.clone();
        second.trash_id = Some("generation-2".into());
        let mut pane = Pane::new(PathBuf::from("trash:/"));
        pane.entries = vec![first.clone(), second];
        pane.visible = vec![0, 1];
        pane.selected.insert(first.selection_key());

        let selected = pane.selected_trash_refs();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, std::ffi::OsString::from("generation-1"));
    }

    #[test]
    fn trash_children_are_not_independent_trash_generations() {
        let original = PathBuf::from("/gone/herdr");
        let mut root = entries_named(&["herdr"]).remove(0);
        root.path = original.clone();
        root.kind = fs::Kind::Dir;
        root.trash_id = Some("herdr.2.trashinfo".into());
        let mut child = entries_named(&["SKILL.md"]).remove(0);
        child.path = original.join("SKILL.md");
        child.depth = 1;

        let mut pane = Pane::new(PathBuf::from("trash:/"));
        pane.entries = vec![root.clone(), child.clone()];
        pane.visible = vec![0, 1];
        pane.selected.insert(root.selection_key());
        pane.selected.insert(child.selection_key());

        let selected = pane.selected_trash_refs();
        assert_eq!(selected.len(), 1);
        assert_eq!(
            selected[0].id,
            std::ffi::OsString::from("herdr.2.trashinfo")
        );
        assert_eq!(selected[0].original_path, original);

        pane.selected.clear();
        pane.selected.insert(child.selection_key());
        assert!(pane.selected_trash_refs().is_empty());
    }

    #[test]
    fn pending_focus_is_applied_by_the_next_listing() {
        let mut pane = pane_with(&["a"]);
        let wanted = PathBuf::from("/tmp/b");
        pane.focus_after_refresh(wanted.clone());
        pane.set_entries(entries_named(&["a", "b"]));
        assert_eq!(pane.current().map(|entry| &entry.path), Some(&wanted));
        assert!(pane.pending_focus.is_none());
    }

    #[test]
    fn a_location_focus_reopens_ancestors_and_finds_the_row() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-mark-{unique}"));
        let folder = base.join("folder");
        let wanted = folder.join("wanted");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(&wanted, b"wanted").unwrap();

        let mut pane = Pane::new(base.clone());
        let focus = EntryFocus {
            path: wanted.clone(),
            selection_key: wanted.clone(),
            retry_missing: false,
        };
        pane.reveal_ancestors(&focus);
        pane.pending_focus = Some(focus);
        pane.set_entries(fs::read_dir(&base, 0).unwrap());

        assert!(pane.expanded.contains(&folder));
        assert_eq!(pane.current().map(|entry| &entry.path), Some(&wanted));
        assert!(pane.pending_focus.is_none());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn a_missing_location_focus_is_consumed_after_one_listing() {
        let mut pane = pane_with(&["a"]);
        pane.pending_focus = Some(EntryFocus {
            path: PathBuf::from("/tmp/missing"),
            selection_key: PathBuf::from("/tmp/missing"),
            retry_missing: false,
        });

        pane.set_entries(entries_named(&["a"]));

        assert!(pane.pending_focus.is_none());
    }

    #[test]
    fn compact_visual_line_selects_entries_one_line_at_a_time() {
        let mut app = App::new(PathBuf::from("/tmp"));
        let mut pane = pane_with(&["a", "b", "c", "d"]);
        pane.view = ViewMode::Compact;
        pane.grid_rows = 4;
        pane.cursor = 1;
        pane.anchor = 1;
        *app.pane_mut() = pane;
        app.mode = Mode::VisualLine;

        app.move_cursor(0, true);
        assert_eq!(app.pane().selected.len(), 1);
        app.move_cursor(1, true);
        assert_eq!(app.pane().selected.len(), 2);
    }

    #[test]
    fn icon_visual_block_selects_a_rectangle() {
        let mut app = App::new(PathBuf::from("/tmp"));
        let mut pane = pane_with(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
        pane.view = ViewMode::Icons;
        pane.grid_cols = 3;
        pane.cursor = 0;
        pane.anchor = 0;
        *app.pane_mut() = pane;
        app.mode = Mode::VisualBlock;

        app.move_cursor(4, true);
        let selected: HashSet<String> = app
            .pane()
            .selected
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            selected,
            HashSet::from(["a".into(), "b".into(), "d".into(), "e".into()])
        );
    }

    #[test]
    fn leaving_icons_ends_visual_block() {
        let mut app = App::new(PathBuf::from("/tmp"));
        app.pane_mut().view = ViewMode::Icons;
        app.mode = Mode::VisualBlock;
        app.pane_mut().selected.insert(PathBuf::from("selected"));

        app.set_view(ViewMode::Compact);

        assert_eq!(app.mode, Mode::Normal);
        assert!(app.pane().selected.is_empty());
    }

    #[test]
    fn icon_visual_line_still_selects_a_whole_grid_row() {
        let mut app = App::new(PathBuf::from("/tmp"));
        let mut pane = pane_with(&["a", "b", "c", "d"]);
        pane.view = ViewMode::Icons;
        pane.grid_cols = 2;
        pane.cursor = 1;
        pane.anchor = 1;
        *app.pane_mut() = pane;
        app.mode = Mode::VisualLine;

        app.move_cursor(0, true);
        assert_eq!(app.pane().selected.len(), 2);
    }

    #[test]
    fn counts_split_dirs_and_files() {
        let pane = pane_with(&["a", "b"]);
        let counts = pane.counts();
        assert_eq!((counts.dirs, counts.files, counts.bytes), (0, 2, 0));
    }

    /// `5dd` on the second-to-last file must take what is there and stop, not
    /// panic on the range and not wrap to the top.
    #[test]
    fn a_delete_range_clamps_to_the_listing() {
        let pane = pane_with(&["a", "b", "c"]);
        let file_names_of = |paths: Vec<PathBuf>| -> Vec<String> {
            paths
                .iter()
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(file_names_of(pane.paths_in(1, 1)), ["b"]);
        assert_eq!(file_names_of(pane.paths_in(1, 99)), ["b", "c"]);
        assert_eq!(file_names_of(pane.paths_in(0, 2)), ["a", "b", "c"]);
        // Past the end entirely: clamped to the last item, never empty.
        assert_eq!(file_names_of(pane.paths_in(9, 9)), ["c"]);
        assert!(pane_with(&[]).paths_in(0, 3).is_empty());
    }
    #[test]
    fn toggle_fold_from_a_child_targets_its_innermost_containing_folder() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-toggle-child-{unique}"));
        let outer = base.join("outer");
        let inner = outer.join("inner");
        let leaf = inner.join("leaf");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::write(&leaf, b"leaf").unwrap();

        let mut app = App::new(base.clone());
        app.pane_mut().set_entries(fs::read_dir(&base, 0).unwrap());
        app.open_all_folds();
        app.pane_mut().cursor = app
            .pane()
            .visible
            .iter()
            .position(|&index| app.pane().entries[index].path == leaf)
            .unwrap();

        app.toggle_expand();

        assert!(app.pane().expanded.contains(&outer));
        assert!(!app.pane().expanded.contains(&inner));
        assert_eq!(app.pane().current().map(|entry| &entry.path), Some(&inner));
        assert!(!app.pane().entries.iter().any(|entry| entry.path == leaf));

        // On the folder row itself, `za` still toggles that folder rather than
        // its containing outer fold.
        app.toggle_expand();
        assert!(app.pane().expanded.contains(&outer));
        assert!(app.pane().expanded.contains(&inner));
        assert!(app.pane().entries.iter().any(|entry| entry.path == leaf));

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn fold_commands_distinguish_one_level_recursive_and_all() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("dolvim-folds-{unique}"));
        let a = base.join("a");
        let b = a.join("b");
        let x = base.join("x");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::create_dir_all(x.join("y")).unwrap();
        std::fs::write(b.join("leaf"), b"leaf").unwrap();

        let mut app = App::new(base.clone());
        app.pane_mut().set_entries(fs::read_dir(&base, 0).unwrap());
        app.pane_mut().cursor = app
            .pane()
            .visible
            .iter()
            .position(|&index| app.pane().entries[index].path == a)
            .unwrap();

        app.open_fold(true);
        assert!(app.pane().expanded.contains(&a));
        assert!(app.pane().expanded.contains(&b));
        assert!(app.pane().entries.iter().any(|entry| entry.name == "leaf"));

        app.close_fold(false);
        assert!(!app.pane().expanded.contains(&a));
        assert!(app.pane().expanded.contains(&b));
        assert!(!app.pane().entries.iter().any(|entry| entry.name == "leaf"));
        app.open_fold(false);
        assert!(app.pane().entries.iter().any(|entry| entry.name == "leaf"));

        app.close_fold(true);
        assert!(!app.pane().expanded.contains(&a));
        assert!(!app.pane().expanded.contains(&b));
        app.open_all_folds();
        assert!(app.pane().expanded.contains(&a));
        assert!(app.pane().expanded.contains(&b));
        assert!(app.pane().expanded.contains(&x));
        assert!(app.pane().expanded.contains(&x.join("y")));
        app.close_all_folds();
        assert!(app.pane().expanded.is_empty());
        assert!(app.pane().entries.iter().all(|entry| entry.depth == 0));

        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn integrated_file_open_persists_path_without_external_launch() {
        let root = std::env::temp_dir().join(format!(
            "dolvim-editor-open-{}",
            NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let first = root.join("first file.rs");
        let second = root.join("second.rs");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let mut app = App::new(root.clone());
        app.pane_mut().set_entries(fs::read_dir(&root, 0).unwrap());
        app.enable_editor(root.clone(), editor::test_handle());
        app.select_by_path(&first);
        app.activate();
        assert_eq!(
            app.editor
                .as_ref()
                .and_then(|state| state.selected_path.as_ref()),
            Some(&first)
        );
        assert!(app.suspend.is_none());
        assert!(app.external_launches.is_empty());

        app.select_by_path(&second);
        app.activate();
        app.editor_opened(1);
        assert_eq!(
            app.editor
                .as_ref()
                .and_then(|state| state.selected_path.as_ref()),
            Some(&second),
            "a stale acknowledgement displaced the newer selection"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn editor_reveal_expands_ancestors_and_survives_layout_and_refresh() {
        let root = std::env::temp_dir().join(format!(
            "dolvim-editor-reveal-{}",
            NEXT_PANE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let nested = root.join("one").join("two");
        let wanted = nested.join("name with space.rs");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(&wanted, b"wanted").unwrap();
        let mut app = App::new(root.clone());
        app.pane_mut().set_entries(fs::read_dir(&root, 0).unwrap());
        app.enable_editor(root.clone(), editor::test_handle());

        app.reveal_editor_path(wanted.clone(), false);
        assert!(app.pane().expanded.contains(&root.join("one")));
        assert!(app.pane().expanded.contains(&nested));
        assert_eq!(app.pane().current().map(|entry| &entry.path), Some(&wanted));
        app.set_editor_layout(editor::Layout::Sidebar);
        app.pane_mut().set_entries(fs::read_dir(&root, 0).unwrap());
        app.reconcile_editor_selection();
        assert_eq!(app.pane().current().map(|entry| &entry.path), Some(&wanted));
        assert_eq!(app.editor_layout(), Some(editor::Layout::Sidebar));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_recursive_operand_clears_descendant_ui_state() {
        let mut app = App::new(PathBuf::from("/tmp"));
        let pane_id = app.pane().id;
        let root = PathBuf::from("/tmp/tree");
        let child = root.join("child");
        let sibling = PathBuf::from("/tmp/sibling");
        app.pane_mut()
            .selected
            .extend([root.clone(), child.clone(), sibling.clone()]);
        app.pane_mut()
            .expanded
            .extend([root.clone(), child.clone(), sibling.clone()]);

        app.remove_operation_paths(pane_id, &HashSet::from([root]), true);

        assert_eq!(app.pane().selected, HashSet::from([sibling.clone()]));
        assert_eq!(app.pane().expanded, HashSet::from([sibling]));
    }
}
