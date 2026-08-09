//! Application state: tabs, panes, selection, navigation, modes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use ratatui::layout::Rect;

use crate::config;
use crate::fs::{self, Entry, Lister, Sort, SortKey};
use crate::ops::{self, Clipboard, Progress, UndoOp};
use crate::places::{self, Row, Target};
use crate::thumbs::Thumbs;
use crate::watch::Watcher;

/// Work that needs the terminal to itself. Dolvim leaves the alternate
/// screen, runs it to completion, and comes back.
pub enum Suspend {
    /// F4: a shell in this directory.
    Shell(PathBuf),
    /// A file whose handler is a terminal program.
    Open(PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Icons,
    Compact,
    Details,
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
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Mode {
    Normal,
    Visual,
    /// `V`. Linewise: the range grows in whole rows of the grid. In Details a
    /// row holds one item, so there it is the same thing as `Visual`.
    VisualLine,
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
    NewFolder,
    /// `o`; the payload is the directory where the file will be created.
    NewFile(PathBuf),
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
    /// Either visual: a range is being dragged and motions extend it.
    pub fn is_visual(&self) -> bool {
        matches!(self, Mode::Visual | Mode::VisualLine)
    }

    /// What the status bar calls this mode. The match is exhaustive so that a
    /// new variant cannot ship without deciding what the user is told it is.
    pub fn name(&self) -> &'static str {
        match self {
            Mode::Normal => "NORMAL",
            Mode::Visual => "VISUAL",
            Mode::VisualLine => "V-LINE",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
            Mode::Filter => "FILTER",
            Mode::PathEdit => "PATH",
            Mode::Rename(_) | Mode::BatchRename => "RENAME",
            Mode::NewFolder => "NEW FOLDER",
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Confirm {
    DeletePermanently(Vec<PathBuf>),
    /// The same question for items already in the Trash, which have to be
    /// purged through the trash index rather than unlinked where they lie.
    PurgeFromTrash(Vec<PathBuf>),
    EmptyTrash,
}

/// One file view. Two of these exist when the split view is on.
pub struct Pane {
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
    pub history: Vec<PathBuf>,
    pub history_pos: usize,
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

impl Pane {
    pub fn new(cwd: PathBuf) -> Pane {
        Pane {
            target: Target::Dir(cwd.clone()),
            history: vec![cwd.clone()],
            cwd,
            entries: Vec::new(),
            visible: Vec::new(),
            cursor: 0,
            anchor: 0,
            offset: 0,
            selected: HashSet::new(),
            view: ViewMode::Icons,
            sort: Sort::default(),
            show_hidden: false,
            filter: String::new(),
            expanded: HashSet::new(),
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
            grid_x: 0,
            last_reveal: (0, ViewMode::Icons),
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
        let keep = self.current().map(|e| e.path.clone());
        self.entries = entries;
        self.refilter_keeping(keep);
    }

    /// Recompute `visible` from `entries` honouring hidden, filter and sort,
    /// keeping the cursor on the same file where possible.
    pub fn refilter(&mut self) {
        let keep = self.current().map(|e| e.path.clone());
        self.refilter_keeping(keep);
    }

    fn refilter_keeping(&mut self, keep: Option<PathBuf>) {
        fs::sort_entries(&mut self.entries, self.sort);
        self.revisible();
        self.cursor = keep
            .and_then(|p| {
                self.visible
                    .iter()
                    .position(|&entry_index| self.entries[entry_index].path == p)
            })
            .unwrap_or_else(|| self.cursor.min(self.visible.len().saturating_sub(1)));
        self.clamp();
    }

    /// Rebuild `visible` only. Kept separate from `refilter` because the
    /// Details tree is ordered positionally and must not be re-sorted.
    fn revisible(&mut self) {
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

    /// Items per row for the current view mode; 1 for Details.
    pub fn stride(&self) -> usize {
        match self.view {
            ViewMode::Icons => self.grid_cols.max(1) as usize,
            // Compact flows down columns, so a horizontal step is a full column.
            ViewMode::Compact => self.grid_rows.max(1) as usize,
            ViewMode::Details => 1,
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
                    self.selected.contains(&e.path).then(|| e.path.clone())
                })
                .collect()
        }
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
    pub position: CellPos,
    pub origin: CellPos,
    pub started: bool,
}

pub struct App {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    pub places: Vec<Row>,
    pub places_cursor: usize,
    pub places_visible: bool,
    pub info_visible: bool,
    pub filter_bar: bool,
    pub focus: Focus,
    /// Region below the toolbar, restored by cancellation or `Ctrl+j`.
    pub toolbar_return: FocusRegion,
    pub mode: Mode,
    /// Buffer for whichever text-entry mode is active.
    pub input: String,
    pub input_cursor: usize,
    pub clipboard: Clipboard,
    pub undo: Vec<UndoOp>,
    pub status: String,
    pub status_is_error: bool,
    pub typeahead: String,
    pub typeahead_at: Option<Instant>,
    /// Pending vim state: count prefix and chord leader (`g`, `z`, `c`).
    pub count: String,
    pub pending_chord_leader: Option<char>,
    /// A `d` waiting for its motion, holding the count typed before it (`3dd`).
    /// One operator exists, so this is that count and not an enum of operators.
    pub pending_delete: Option<usize>,
    /// An `m` or `'` waiting for the letter that names the mark.
    pub pending_mark: Option<MarkPending>,
    /// Vim's marks, at the granularity a file manager has: `ma` remembers where
    /// you are, `'a` goes back. Kept for the session only, as vim's lowercase
    /// marks are kept for the file.
    pub marks: HashMap<char, Target>,
    pub search_last: String,
    pub drag: Option<Drag>,
    pub hits: Hitboxes,
    pub menu_cursor: usize,
    /// Free/total bytes for the status bar, and when they were measured.
    disk: Option<DiskCache>,
    /// Last left-click (when, which item) — the double-click detector.
    pub last_click: Option<LastClick>,
    pub thumbs: Thumbs,
    pub transfer_progress: Option<Progress>,
    pub quit: bool,
    /// Set when something needs the terminal to itself; `main` hands it over.
    pub suspend: Option<Suspend>,
    lister: Lister,
    pub listing_rx: Receiver<fs::ListingMsg>,
    watcher: Watcher,
    /// Directory listings cached per pane `seq` sequence number, keyed while streaming.
    streaming: HashMap<StreamKey, Vec<Entry>>,
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
            info_visible: false,
            filter_bar: false,
            focus: Focus::View,
            toolbar_return: FocusRegion::View(0),
            mode: Mode::Normal,
            input: String::new(),
            input_cursor: 0,
            clipboard: Clipboard::default(),
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
            transfer_progress: None,
            quit: false,
            suspend: None,
            lister,
            listing_rx,
            watcher,
            streaming: HashMap::new(),
        };
        app.reload();
        app
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
    pub fn pump_fs_events(&mut self) {
        let listing_messages: Vec<fs::ListingMsg> = self.listing_rx.try_iter().collect();
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
        }
        let live_seqs: HashSet<u64> = self
            .tabs
            .iter()
            .flat_map(|t| t.panes.iter())
            .map(|pane| pane.seq)
            .collect();
        self.streaming.retain(|key, _| live_seqs.contains(&key.seq));
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

    /// inotify said something changed: relist without moving the cursor.
    pub fn refresh_in_place(&mut self) {
        let target = self.pane().target.clone();
        if let Target::Dir(d) = target {
            let seq = self.pane().seq + 1;
            self.pane_mut().seq = seq;
            self.lister.request(d, seq);
        } else {
            self.reload();
        }
    }

    // -- navigation --------------------------------------------------------

    pub fn goto(&mut self, target: Target, push_history: bool) {
        let cwd = match &target {
            Target::Dir(d) => d.clone(),
            Target::Trash => PathBuf::from("trash:/"),
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
        {
            let pane = self.pane_mut();
            if push_history && pane.cwd != cwd {
                pane.history.truncate(pane.history_pos + 1);
                pane.history.push(cwd.clone());
                pane.history_pos = pane.history.len() - 1;
            }
            pane.cwd = cwd;
            pane.target = target.clone();
            pane.cursor = 0;
            pane.offset = 0;
            pane.selected.clear();
            pane.filter.clear();
            pane.expanded.clear();
        }
        if let Some(i) = places::index_of(&self.places, &target) {
            self.places_cursor = i;
        }
        self.reload();
    }

    pub fn open_dir(&mut self, d: PathBuf) {
        self.goto(Target::Dir(d), true);
    }

    pub fn go_up(&mut self) {
        let cwd = self.pane().cwd.clone();
        if let Some(parent) = cwd.parent().map(Path::to_path_buf) {
            self.open_dir(parent.clone());
            // Dolphin leaves the cursor on the directory you came out of.
            self.select_by_path(&cwd);
        }
    }

    pub fn back(&mut self) {
        let pane = self.pane();
        if pane.history_pos == 0 {
            return;
        }
        let pos = pane.history_pos - 1;
        let d = pane.history[pos].clone();
        self.pane_mut().history_pos = pos;
        self.goto(Target::Dir(d), false);
    }

    pub fn forward(&mut self) {
        let pane = self.pane();
        if pane.history_pos + 1 >= pane.history.len() {
            return;
        }
        let pos = pane.history_pos + 1;
        let d = pane.history[pos].clone();
        self.pane_mut().history_pos = pos;
        self.goto(Target::Dir(d), false);
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
        if entry.is_dir() {
            self.open_dir(entry.path);
        } else {
            self.open_external(&entry.path);
        }
    }

    pub fn open_external(&mut self, path: &Path) {
        // A handler that wants a tty cannot share ours: both would sit in raw
        // mode redrawing over each other, which is the glitching. Step out for
        // the duration instead.
        if ops::opens_in_terminal(path) {
            self.suspend = Some(Suspend::Open(path.to_path_buf()));
            return;
        }
        // Detached, with its streams closed: a graphical handler that grumbles
        // on stderr would otherwise print into our alternate screen.
        match std::process::Command::new("xdg-open")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(_) => self.info(format!("Opening {}", ops::file_name_of(path))),
            Err(spawn_error) => self.error(format!("xdg-open failed: {spawn_error}")),
        }
    }

    // -- cursor motion -----------------------------------------------------

    pub fn move_cursor(&mut self, delta: isize, extend: bool) {
        let is_linewise = self.mode == Mode::VisualLine;
        let pane = self.pane_mut();
        if pane.visible.is_empty() {
            return;
        }
        let last = pane.visible.len() as isize - 1;
        let next = (pane.cursor as isize + delta).clamp(0, last) as usize;
        pane.cursor = next;
        if extend {
            let (mut range_start, mut range_end) = (pane.anchor.min(next), pane.anchor.max(next));
            // Linewise: grow the range out to the row edges on both sides.
            if is_linewise {
                let stride = pane.stride();
                range_start -= range_start % stride;
                range_end = (range_end - range_end % stride + stride - 1).min(last as usize);
            }
            let range: Vec<PathBuf> = (range_start..=range_end)
                .filter_map(|visible_index| {
                    pane.entry_at(visible_index).map(|entry| entry.path.clone())
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
        if !pane.selected.remove(&entry.path) {
            pane.selected.insert(entry.path);
        }
    }

    pub fn select_all(&mut self) {
        let pane = self.pane_mut();
        pane.selected = pane
            .visible
            .iter()
            .map(|&entry_index| pane.entries[entry_index].path.clone())
            .collect();
    }

    pub fn invert_selection(&mut self) {
        let pane = self.pane_mut();
        let all: HashSet<PathBuf> = pane
            .visible
            .iter()
            .map(|&entry_index| pane.entries[entry_index].path.clone())
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

    // -- view controls -----------------------------------------------------

    pub fn set_view(&mut self, v: ViewMode) {
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

    /// Dolphin's expandable folders: splice the child listing in beneath the
    /// folder row, at depth+1, or remove it again.
    pub fn toggle_expand(&mut self) {
        let Some(entry) = self.pane().current().cloned() else {
            return;
        };
        if !entry.is_dir() {
            return;
        }
        let collapsing = self.pane().expanded.contains(&entry.path);
        let child_entries = if collapsing {
            Vec::new()
        } else {
            match fs::read_dir(&entry.path, entry.depth + 1) {
                Ok(mut child_entries) => {
                    fs::sort_entries(&mut child_entries, self.pane().sort);
                    child_entries
                }
                Err(err) => {
                    self.error(format!("Cannot read {}: {err}", entry.name));
                    return;
                }
            }
        };
        let pane = self.pane_mut();
        let keep_path = pane
            .visible
            .get(pane.cursor)
            .map(|&entry_index| pane.entries[entry_index].path.clone());
        if collapsing {
            pane.expanded.remove(&entry.path);
            let prefix = entry.path.clone();
            pane.expanded.retain(|expanded_path| {
                !expanded_path.starts_with(&prefix) || *expanded_path == prefix
            });
            pane.entries.retain(|candidate_entry| {
                candidate_entry.path == prefix || !candidate_entry.path.starts_with(&prefix)
            });
        } else {
            let insert_index = pane
                .entries
                .iter()
                .position(|candidate_entry| candidate_entry.path == entry.path)
                .unwrap_or(0);
            pane.entries
                .splice(insert_index + 1..insert_index + 1, child_entries);
            pane.expanded.insert(entry.path.clone());
        }
        for candidate_entry in &mut pane.entries {
            if candidate_entry.path == entry.path {
                candidate_entry.expanded = !collapsing;
            }
        }
        // The tree order is positional, so re-sorting would destroy it; only
        // the filter is reapplied.
        pane.revisible();
        if let Some(keep_path) = keep_path {
            if let Some(i) = pane
                .visible
                .iter()
                .position(|&entry_index| pane.entries[entry_index].path == keep_path)
            {
                pane.cursor = i;
            }
        }
        pane.clamp();
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
                kind: fs::Kind::File,
                size: 0,
                mtime: 0,
                mode: 0,
                readable: true,
                hidden: name.starts_with('.'),
                depth: 0,
                expanded: false,
            })
            .collect()
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
}
