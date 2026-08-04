//! Configuration is source code. Edit, recompile, done.
//!
//! Every table below is data: a row is one decision. Colors are the Breeze
//! light values measured off `Dolphin_screenshot.png`; see `docs/UI_SPEC.md`
//! for the sample points.
//!
//! The tables are hand-aligned, and `rustfmt.toml` ignores this file so they
//! stay that way — but `ignore` is a nightly option, so format with
//! `cargo +nightly fmt`. Stable `cargo fmt` warns and reformats anyway.
//! Keep the columns lined up when you add a row; that is the whole point.

use ratatui::style::Color;

use crate::fs::TimeStyle;

/* Breeze palette. Truecolor: terminals that cannot do 24-bit will degrade,
 * which is their business, not ours. */
pub mod color {
    use super::Color;
    /*        name                           r    g    b       used for                        */
    pub const TOOLBAR_BG:Color = Color::Rgb(244, 245, 246); /* toolbar, breadcrumb, tab strip  */
    pub const PANEL_BG  :Color = Color::Rgb(239, 240, 241); /* Places and Information panels   */
    pub const VIEW_BG   :Color = Color::Rgb(255, 255, 255); /* the file view itself            */
    pub const SELECTION :Color = Color::Rgb(194, 224, 245); /* selected row / cell fill        */
    pub const ACCENT    :Color = Color::Rgb( 61, 174, 233); /* focus bar, active tab underline */
    pub const TEXT      :Color = Color::Rgb( 35,  38,  41); /* default foreground              */
    pub const DIM       :Color = Color::Rgb(127, 140, 141); /* headings, disabled toolbar keys */
    pub const SEPARATOR :Color = Color::Rgb(220, 220, 220); /* the 1 px splitter lines         */
    pub const FOLDER    :Color = Color::Rgb( 61, 174, 233); /* directory entries               */
    pub const FILE      :Color = Color::Rgb( 99, 104, 109); /* regular file entries            */
    pub const SYMLINK   :Color = Color::Rgb( 26, 138, 190); /* symbolic links                  */
    pub const EXEC      :Color = Color::Rgb( 58, 156,  74); /* executables                     */
    pub const CUT       :Color = Color::Rgb(160, 165, 170); /* cut items, ghosted as in Dolphin*/
    pub const ERROR     :Color = Color::Rgb(218,  68 , 83); /* status bar errors               */
    pub const GAUGE_FULL:Color = Color::Rgb(200, 205, 210); /* used part of a device capacity  */
    pub const OFFLINE   :Color = Color::Rgb(246, 116,   0); /* unreachable: unmounted device   */
                                                            /* or locked folder. Breeze carrot */
}

/* Icon stand-ins. A terminal cell is not 48x48 px; these are the closest
 * unambiguous glyphs available. The Private Use Area ones need a Nerd Font
 * patched terminal font; without one they render as tofu — swap them for a
 * plain Unicode glyph and recompile, that is the whole fallback story. */
pub mod glyph {
    /*        name          escape                 glyph  where it appears      */
    pub const FOLDER       :&str = "\u{ea83}" ; /*   directory entry            */
    pub const FOLDER_EMPTY :&str = "\u{eaf7}" ; /*   directory with no children */
    pub const FOLDER_OPEN  :&str = "\u{f0770}"; /* 󰝰 expanded in Details        */
    pub const FOLDER_LOCKED:&str = "\u{f1aa8}"; /* 󱪨 no permission to enter     */
    pub const FILE         :&str = "\u{f016}" ; /*  regular file                */
    pub const SYMLINK      :&str = "\u{2937}" ; /* ⤷ link                       */
    pub const HOME         :&str = "\u{f02dc}"; /* 󰋜 Places: Home               */
    pub const TRASH        :&str = "\u{f014}" ; /*   Places: Trash              */
    pub const NETWORK      :&str = "\u{f0556}"; /* 󰕖 Places: Network            */
    pub const DEVICE       :&str = "\u{f02ca}"; /* 󰋊 a mounted partition        */
    pub const DEVICE_OFF   :&str = "\u{f104c}"; /* 󱁌 unmounted, unreachable     */
    pub const DEVICE_USB   :&str = "\u{f11f0}"; /* 󱇰 removable media            */
    pub const EJECT        :&str = "\u{f01ea}"; /* 󰇪 unmount a removable        */
    pub const CLOCK        :&str = "\u{f0e17}"; /* 󰸗 Places: Recent             */
    pub const DESKTOP      :&str = "\u{f108}" ; /*   Desktop                    */
    pub const DOWNLOAD     :&str = "\u{f409}" ; /*   Downloads                  */
    pub const DOCUMENT     :&str = "\u{eaf0}" ; /*   Documents                  */
    pub const MUSIC        :&str = "\u{266a}" ; /* ♪ Places: Music              */
    pub const PICTURE      :&str = "\u{f0976}"; /* 󰥶 Places: Pictures           */
    pub const VIDEO        :&str = "\u{f0567}"; /* 󰕧 Places: Videos             */
    pub const ARCHIVE      :&str = "\u{f05c4}"; /* 󰗄 archive file               */
    pub const BACK         :&str = "\u{efc3}" ; /*   toolbar back               */
    pub const FORWARD      :&str = "\u{edfb}" ; /*   toolbar forward            */
    pub const VIEW_ICONS   :&str = "\u{f0570}"; /* 󰕰 toolbar view button        */
    pub const VIEW_COMPACT :&str = "\u{f02be}"; /* 󰊾 toolbar view button        */
    pub const VIEW_DETAILS :&str = "\u{ef81}" ; /*   toolbar view button        */
    pub const SPLIT        :&str = "\u{f4b4}" ; /*   toolbar Split              */
    pub const SEARCH       :&str = "\u{ea6d}" ; /*  toolbar Search              */
    pub const MENU         :&str = "\u{2630}" ; /* ☰ toolbar hamburger         */
    pub const DROPDOWN     :&str = "\u{25be}" ; /* ▾  split-button, open crumb  */
    pub const CRUMB_SHUT   :&str = "\u{2bc8}" ; /* ⯈  crumb whose menu is shut  */
    pub const CRUMB_SEP    :&str = "\u{203a}" ; /* ›  breadcrumb separator      */
    pub const SORT_ASC     :&str = "\u{25b4}" ; /* ▴  Details column head       */
    pub const SORT_DESC    :&str = "\u{25be}" ; /* ▾  Details column head       */
    pub const EXPAND_CLOSED:&str = "\u{25b8}" ; /* ▸  collapsed folder          */
    pub const EXPAND_OPEN  :&str = "\u{25be}" ; /* ▾  expanded folder           */
}

/* Panel geometry and tunables. */
pub const PLACES_WIDTH        :  u16 =   22; /* Places panel columns, the screenshot's 150 px   */
pub const INFO_WIDTH          :  u16 =   30; /* Information panel columns (F11)                 */
pub const TYPEAHEAD_TIMEOUT_MS: u128 = 1000; /* type-ahead buffer life without a keystroke      */
/* Double-click window. Dolphin selects on the first click and opens on the
 * second — verified on a stock install with no `SingleClick` key set in
 * kdeglobals. We do the same. */
pub const DOUBLE_CLICK_MS     :  u64 =  400; /* second click inside this counts as a double     */
pub const WATCH_DEBOUNCE_MS   :  u64 =  120; /* coalescing window for inotify storms            */
pub const TICK_MS             :  u64 =   40; /* event loop tick; listing/thumbnail poll rate    */
pub const DRAG_THRESHOLD      :  u16 =    1; /* cells of movement before a drag begins          */
pub const THUMB_CACHE_CAP     :usize =  512; /* decoded thumbnails held in memory               */
pub const THUMB_MAX_INFLIGHT  :usize =   32; /* decodes queued at once; the rest wait           */
pub const CELL_GAP            :  u16 =    2; /* blank columns between icon-view tiles           */
pub const VIEW_MARGIN         :  u16 =    1; /* blank columns left of Compact and Details rows  */
pub const NAME_LINES          :  u16 =    3; /* rows a name may wrap over in the icon view      */
pub const DISK_POLL_MS        :  u64 = 2000; /* how often the status bar re-measures free space */
pub const COPY_CHUNK_BYTES    :usize = 256 * 1024; /* streaming copy buffer; cancel granularity  */
pub const RECENT_MAX_DEPTH    :  u32 =    3; /* how deep a Recent search walks                  */
pub const RECENT_MAX_ITEMS    :usize = 2000; /* results a Recent search stops at                */

/* How the Details `Modified` column spells a timestamp. `Short` is Dolphin's
 * `Aug 2, 8:22pm`, dropping the year while it is the current one; `Iso` is
 * `2026-08-02 20:22`, which sorts as it reads. Month names are English: the
 * locale's are the C library's to know, and this crate forbids `unsafe`.
 * Widen TIME_WIDTH to match if you change the style. */
pub const TIME_STYLE          :TimeStyle = TimeStyle::Short;
pub const TIME_WIDTH          :  u16 =   20; /* `Modified` width at its widest, with a year     */
pub const SIZE_WIDTH          :  u16 =   12; /* Details `Size` column                           */
pub const TYPE_WIDTH          :  u16 =   14; /* Details `Type` column                           */
pub const PROGRESS_POPUP_W    :  u16 =   60; /* transfer popup, columns                         */
pub const PROGRESS_POPUP_H    :  u16 =    6; /* transfer popup, rows                            */
pub const PROGRESS_BAR_WIDTH  :usize =   56; /* the meter inside it: popup width less borders   */
/* Columns the toolbar keeps clear on the right for its own controls: the
 * hamburger, Search and the Split button, with the blanks between them.
 * Widen it if you add a button, or the breadcrumb will run underneath. */
pub const TOOLBAR_RIGHT_WIDTH :  u16 =   22; /* right-hand toolbar controls, columns            */

/* Icon cell *pitch*: each cell keeps a blank row on top and CELL_GAP blank
 * columns, which is where the cursor frame is drawn. Content is therefore
 * (CELL_WIDTH - CELL_GAP) by (CELL_HEIGHT - 1) — one icon row and three of name, the
 * 13x4 the layout was designed around. Compact sizes its own columns. */
pub const CELL_WIDTH          :  u16 =   15; /* icon-view cell pitch, columns                   */
pub const CELL_HEIGHT         :  u16 =    5; /* icon-view cell pitch, rows                      */

/* The XDG user directories, as xdg-user-dirs defaults them. Places lists these
 * rows and the view badges the same folders, so both read this one table — a
 * second spelling of "Downloads" is how the Places row went missing once.
 * The glyph is also the view's badge, painted only on a folder directly under
 * $HOME, so a Downloads you made elsewhere stays a plain folder. */
pub struct XdgDir {
    pub env_key: &'static str,
    pub name   : &'static str,
    pub glyph  : &'static str,
}

/* One row per directory; `xdg_dir` reads the key, Places shows the name. */
const fn xdg(env_key: &'static str, name: &'static str, glyph: &'static str) -> XdgDir {
    XdgDir { env_key, name, glyph }
}

pub const XDG_DIRS: &[XdgDir] = &[
    /* user-dirs.dirs key        name         glyph          */
    xdg("XDG_DESKTOP_DIR"  , "Desktop"  , glyph::DESKTOP ),
    xdg("XDG_DOCUMENTS_DIR", "Documents", glyph::DOCUMENT),
    xdg("XDG_DOWNLOAD_DIR" , "Downloads", glyph::DOWNLOAD),
    xdg("XDG_MUSIC_DIR"    , "Music"    , glyph::MUSIC   ),
    xdg("XDG_PICTURES_DIR" , "Pictures" , glyph::PICTURE ),
    xdg("XDG_VIDEOS_DIR"   , "Videos"   , glyph::VIDEO   ),
];

/* File classification by extension. */
pub const IMAGE_EXTS   : &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff"];
pub const ARCHIVE_EXTS : &[&str] = &["tar", "gz" , "tgz" , "bz2", "xz" , "zst" , "zip", "7z" , "rar"];

// ---------------------------------------------------------------------------
// Keymap
// ---------------------------------------------------------------------------

use crossterm::event::KeyCode::*;
use crossterm::event::KeyModifiers;

use crate::vim::{bind, Action, Bind};

/* Modifier names for the tables below. A const context has no `|` operator —
 * that is a trait call — so a combination spells itself with `.union()`. Naming
 * the combinations here keeps that spelling out of every row. */
const NONE      : KeyModifiers = KeyModifiers::NONE;
const ALT       : KeyModifiers = KeyModifiers::ALT;
const CTRL      : KeyModifiers = KeyModifiers::CONTROL;
const SHIFT     : KeyModifiers = KeyModifiers::SHIFT;
const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

/* Dolphin's native shortcuts. Active in every non-text mode, alongside vim. */
pub const DOLPHIN_KEYS: &[Bind] = &[
/*  key              mods        action                  */
    bind(Left      , ALT       , Action::Back           ),
    bind(Right     , ALT       , Action::Forward        ),
    bind(Up        , ALT       , Action::GoUp           ),
    bind(Backspace , NONE      , Action::GoUp           ),
    bind(Home      , ALT       , Action::GoHome         ),
    bind(F(5)      , NONE      , Action::Refresh        ),
    bind(Enter     , NONE      , Action::Open           ),
    bind(Down      , NONE      , Action::MoveDown       ),
    bind(Up        , NONE      , Action::MoveUp         ),
    bind(Left      , NONE      , Action::MoveLeft       ),
    bind(Right     , NONE      , Action::MoveRight      ),
    bind(Home      , NONE      , Action::Top            ),
    bind(End       , NONE      , Action::Bottom         ),
    bind(PageDown  , NONE      , Action::PageDown       ),
    bind(PageUp    , NONE      , Action::PageUp         ),
    bind(Char(' ') , NONE      , Action::ToggleSelect   ),
    bind(Char('a') , CTRL      , Action::SelectAll      ),
    bind(Char('A') , CTRL_SHIFT, Action::InvertSelect   ),
    bind(Char('c') , CTRL      , Action::Copy           ),
    bind(Char('x') , CTRL      , Action::Cut            ),
    bind(Char('v') , CTRL      , Action::Paste          ),
    bind(Delete    , NONE      , Action::Trash          ),
    bind(Delete    , SHIFT     , Action::DeletePerm     ),
    bind(F(2)      , NONE      , Action::Rename         ),
    bind(F(10)     , NONE      , Action::NewFolder      ),
    bind(Char('n') , CTRL_SHIFT, Action::NewFolder      ),
    bind(Enter     , ALT       , Action::Properties     ),
    bind(Char('1') , CTRL      , Action::ViewIcons      ),
    bind(Char('2') , CTRL      , Action::ViewCompact    ),
    bind(Char('3') , CTRL      , Action::ViewDetails    ),
    bind(F(3)      , NONE      , Action::ToggleSplit    ),
    bind(Tab       , NONE      , Action::SwapPane       ),
    bind(F(9)      , NONE      , Action::TogglePlaces   ),
    bind(F(11)     , NONE      , Action::ToggleInfo     ),
    bind(Char('i') , CTRL      , Action::ToggleFilterBar),
    bind(Char('t') , CTRL      , Action::NewTab         ),
    bind(Char('w') , CTRL      , Action::CloseTab       ),
    bind(Tab       , CTRL      , Action::NextTab        ),
    bind(BackTab   , CTRL_SHIFT, Action::PrevTab        ),
    bind(Char('f') , CTRL      , Action::EnterSearch    ),
    bind(F(6)      , NONE      , Action::EnterPathEdit  ),
    bind(F(4)      , NONE      , Action::TerminalPanel  ),
    bind(F(4)      , SHIFT     , Action::TerminalHere   ),
    bind(F(1)      , NONE      , Action::Help           ),
    bind(Char('q') , CTRL      , Action::QuitAll        ),
];

/* The vim layer. Consulted first, so `h`/`j`/`k`/`l` are motions; any
 * printable key that lands in neither table falls through to Dolphin's
 * type-ahead jump-to-name. */
pub const VIM_KEYS: &[Bind] = &[
    /*    key         mods    action                  */
    bind(Char('h') , NONE , Action::MoveLeft       ),
    bind(Char('j') , NONE , Action::MoveDown       ),
    bind(Char('k') , NONE , Action::MoveUp         ),
    bind(Char('l') , NONE , Action::MoveRight      ),
    bind(Char('G') , SHIFT, Action::Bottom         ),
    bind(Char('H') , SHIFT, Action::ToggleHidden   ),
    bind(Char('d') , CTRL , Action::HalfPageDown   ),
    bind(Char('u') , CTRL , Action::HalfPageUp     ),
    bind(Char('f') , CTRL , Action::PageDown       ),
    bind(Char('b') , CTRL , Action::PageUp         ),
    bind(Char('0') , NONE , Action::RowStart       ),
    bind(Char('$') , NONE , Action::RowEnd         ),
    bind(Char('v') , NONE , Action::EnterVisual    ),
    bind(Char('V') , SHIFT, Action::EnterVisualLine),
    bind(Char('y') , NONE , Action::Copy           ),
    bind(Char('d') , NONE , Action::DeleteOp       ),
    bind(Char('p') , NONE , Action::Paste          ),
    bind(Char('P') , SHIFT, Action::DropIn         ),
    bind(Char('x') , NONE , Action::Trash          ),
    bind(Char('r') , NONE , Action::Rename         ),
    bind(Char('o') , NONE , Action::NewFile        ),
    bind(Char('O') , SHIFT, Action::NewFolder      ),
    bind(Char('u') , NONE , Action::Undo           ),
    bind(Char('D') , SHIFT, Action::DragOut        ),
    bind(Char('/') , NONE , Action::EnterSearch    ),
    bind(Char('n') , NONE , Action::SearchNext     ),
    bind(Char('N') , SHIFT, Action::SearchPrev     ),
    bind(Char(':') , NONE , Action::EnterCommand   ),
    bind(Enter     , CTRL , Action::OpenInNewTab   ),
    bind(Char('m') , NONE , Action::OpenMenu       ),
    bind(Char('h') , CTRL , Action::FocusLeft      ),
    bind(Char('l') , CTRL , Action::FocusRight     ),
    bind(Char('k') , CTRL , Action::EnterCrumbs    ),
    bind(Char('?') , SHIFT, Action::Help           ),
];

/* The toolbar buttons, left to right, with the breadcrumb between the nav
   group and the rest. Ctrl+h / Ctrl+l step across the three groups; h and l
   walk the buttons inside one. */
pub const NAV_BUTTON_COUNT: usize = 3;
pub const TOOLBAR_BUTTONS: &[Action] = &[
    /* nav group, left of the breadcrumb */
    Action::Back,
    Action::Forward,
    Action::OpenViewMenu,
    /* right group */
    Action::ToggleSplit,
    Action::EnterSearch,
    Action::OpenMenu,
];

/* Two-key sequences: press the leader, then the follower. */
pub struct Chord {
    pub leader  : char,
    pub follower: char,
    pub action  : Action,
}

const fn chord(leader: char, follower: char, action: Action) -> Chord {
    Chord { leader, follower, action }
}

pub const CHORDS: &[Chord] = &[
     /* leader  follower  action              */
    chord('g'  , 'g'     , Action::Top         ),
    chord('g'  , 'h'     , Action::GoHome      ),
    chord('g'  , 't'     , Action::NextTab     ),
    chord('g'  , 'T'     , Action::PrevTab     ),
    chord('g'  , 'u'     , Action::GoUp        ),
    chord('z'  , 'a'     , Action::ToggleExpand),
    chord('z'  , 'v'     , Action::CycleView   ),
    chord('c'  , 'w'     , Action::Rename      ),
];

/* Leaders that must wait for a second key rather than acting immediately. */
pub const CHORD_LEADERS: &[char] = &['g', 'z', 'c'];
