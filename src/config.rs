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
pub const WATCH_DEBOUNCE_MS   :  u64 =  120; /* coalescing window for inotify storms            */
pub const TICK_MS             :  u64 =   40; /* event loop tick; listing/thumbnail poll rate    */
pub const DRAG_THRESHOLD      :  u16 =    1; /* cells of movement before a drag begins          */
pub const THUMB_CACHE_CAP     :usize =  512; /* decoded thumbnails held in memory               */
pub const CELL_GAP            :  u16 =    2; /* blank columns between icon-view tiles           */
pub const VIEW_MARGIN         :  u16 =    1; /* blank columns left of Compact and Details rows  */
pub const NAME_LINES          :  u16 =    3; /* rows a name may wrap over in the icon view      */
pub const DISK_POLL_MS        :  u64 = 2000; /* how often the status bar re-measures free space */

/* Icon cell *pitch*: each cell keeps a blank row on top and CELL_GAP blank
 * columns, which is where the cursor frame is drawn. Content is therefore
 * (CELL_W - CELL_GAP) by (CELL_H - 1) — one icon row and three of name, the
 * 13x4 the layout was designed around. Compact sizes its own columns. */
pub const CELL_W              :  u16 =   15; /* icon-view cell pitch, columns                    */
pub const CELL_H              :  u16 =    5; /* icon-view cell pitch, rows                       */

/* The XDG user directories, as xdg-user-dirs defaults them. Places lists these
 * rows and the view badges the same folders, so both read this one table — a
 * second spelling of "Downloads" is how the Places row went missing once.
 * The glyph is also the view's badge, painted only on a folder directly under
 * $HOME, so a Downloads you made elsewhere stays a plain folder. */
pub const XDG_DIRS: &[(&str, &str, &str)] = &[
    /* user-dirs.dirs key    name         glyph          */
      ("XDG_DESKTOP_DIR"  , "Desktop"  , glyph::DESKTOP ),
      ("XDG_DOCUMENTS_DIR", "Documents", glyph::DOCUMENT),
      ("XDG_DOWNLOAD_DIR" , "Downloads", glyph::DOWNLOAD),
      ("XDG_MUSIC_DIR"    , "Music"    , glyph::MUSIC   ),
      ("XDG_PICTURES_DIR" , "Pictures" , glyph::PICTURE ),
      ("XDG_VIDEOS_DIR"   , "Videos"   , glyph::VIDEO   ),
];

/* File classification by extension. */
pub const IMAGE_EXTS   : &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff"];
pub const ARCHIVE_EXTS : &[&str] = &["tar", "gz" , "tgz" , "bz2", "xz" , "zst" , "zip", "7z" , "rar"];

// ---------------------------------------------------------------------------
// Keymap
// ---------------------------------------------------------------------------

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::vim::{bind, Action, Bind};

/* Dolphin's native shortcuts. Active in every non-text mode, alongside vim. */
pub const DOLPHIN_KEYS: &[Bind] = &[
    /*    key          modifiers                   action                 */
    bind(K::Left     , M::ALT                    , Action::Back           ),
    bind(K::Right    , M::ALT                    , Action::Forward        ),
    bind(K::Up       , M::ALT                    , Action::GoUp           ),
    bind(K::Backspace, M::NONE                   , Action::GoUp           ),
    bind(K::Home     , M::ALT                    , Action::GoHome         ),
    bind(K::F(5)     , M::NONE                   , Action::Refresh        ),
    bind(K::Enter    , M::NONE                   , Action::Open           ),
    bind(K::Down     , M::NONE                   , Action::MoveDown       ),
    bind(K::Up       , M::NONE                   , Action::MoveUp         ),
    bind(K::Left     , M::NONE                   , Action::MoveLeft       ),
    bind(K::Right    , M::NONE                   , Action::MoveRight      ),
    bind(K::Home     , M::NONE                   , Action::Top            ),
    bind(K::End      , M::NONE                   , Action::Bottom         ),
    bind(K::PageDown , M::NONE                   , Action::PageDown       ),
    bind(K::PageUp   , M::NONE                   , Action::PageUp         ),
    bind(K::Char(' '), M::NONE                   , Action::ToggleSelect   ),
    bind(K::Char('a'), M::CONTROL                , Action::SelectAll      ),
    bind(K::Char('A'), M::CONTROL.union(M::SHIFT), Action::InvertSelect   ),
    bind(K::Char('c'), M::CONTROL                , Action::Copy           ),
    bind(K::Char('x'), M::CONTROL                , Action::Cut            ),
    bind(K::Char('v'), M::CONTROL                , Action::Paste          ),
    bind(K::Delete   , M::NONE                   , Action::Trash          ),
    bind(K::Delete   , M::SHIFT                  , Action::DeletePerm     ),
    bind(K::F(2)     , M::NONE                   , Action::Rename         ),
    bind(K::F(10)    , M::NONE                   , Action::NewFolder      ),
    bind(K::Char('n'), M::CONTROL.union(M::SHIFT), Action::NewFolder      ),
    bind(K::Enter    , M::ALT                    , Action::Properties     ),
    bind(K::Char('1'), M::CONTROL                , Action::ViewIcons      ),
    bind(K::Char('2'), M::CONTROL                , Action::ViewCompact    ),
    bind(K::Char('3'), M::CONTROL                , Action::ViewDetails    ),
    bind(K::F(3)     , M::NONE                   , Action::ToggleSplit    ),
    bind(K::Tab      , M::NONE                   , Action::SwapPane       ),
    bind(K::F(9)     , M::NONE                   , Action::TogglePlaces   ),
    bind(K::F(11)    , M::NONE                   , Action::ToggleInfo     ),
    bind(K::Char('i'), M::CONTROL                , Action::ToggleFilterBar),
    bind(K::Char('t'), M::CONTROL                , Action::NewTab         ),
    bind(K::Char('w'), M::CONTROL                , Action::CloseTab       ),
    bind(K::Tab      , M::CONTROL                , Action::NextTab        ),
    bind(K::BackTab  , M::CONTROL.union(M::SHIFT), Action::PrevTab        ),
    bind(K::Char('f'), M::CONTROL                , Action::EnterSearch    ),
    bind(K::F(6)     , M::NONE                   , Action::EnterPathEdit  ),
    bind(K::F(4)     , M::NONE                   , Action::TerminalPanel  ),
    bind(K::F(4)     , M::SHIFT                  , Action::TerminalHere   ),
    bind(K::F(1)     , M::NONE                   , Action::Help           ),
    bind(K::Char('q'), M::CONTROL                , Action::QuitAll        ),
];

/* The vim layer. Consulted first, so `h`/`j`/`k`/`l` are motions; any
 * printable key that lands in neither table falls through to Dolphin's
 * type-ahead jump-to-name. */
pub const VIM_KEYS: &[Bind] = &[
    /*    key          modifiers   action                 */
    bind(K::Char('h'), M::NONE   , Action::MoveLeft       ),
    bind(K::Char('j'), M::NONE   , Action::MoveDown       ),
    bind(K::Char('k'), M::NONE   , Action::MoveUp         ),
    bind(K::Char('l'), M::NONE   , Action::MoveRight      ),
    bind(K::Char('G'), M::SHIFT  , Action::Bottom         ),
    bind(K::Char('H'), M::SHIFT  , Action::ToggleHidden   ),
    bind(K::Char('d'), M::CONTROL, Action::HalfPageDown   ),
    bind(K::Char('u'), M::CONTROL, Action::HalfPageUp     ),
    bind(K::Char('f'), M::CONTROL, Action::PageDown       ),
    bind(K::Char('b'), M::CONTROL, Action::PageUp         ),
    bind(K::Char('0'), M::NONE   , Action::RowStart       ),
    bind(K::Char('$'), M::NONE   , Action::RowEnd         ),
    bind(K::Char('v'), M::NONE   , Action::EnterVisual    ),
    bind(K::Char('V'), M::SHIFT  , Action::EnterVisualLine),
    bind(K::Char('y'), M::NONE   , Action::Copy           ),
    bind(K::Char('d'), M::NONE   , Action::DeleteOp       ),
    bind(K::Char('p'), M::NONE   , Action::Paste          ),
    bind(K::Char('P'), M::SHIFT  , Action::DropIn         ),
    bind(K::Char('x'), M::NONE   , Action::Trash          ),
    bind(K::Char('r'), M::NONE   , Action::Rename         ),
    bind(K::Char('o'), M::NONE   , Action::NewFile        ),
    bind(K::Char('O'), M::SHIFT  , Action::NewFolder      ),
    bind(K::Char('u'), M::NONE   , Action::Undo           ),
    bind(K::Char('D'), M::SHIFT  , Action::DragOut        ),
    bind(K::Char('/'), M::NONE   , Action::EnterSearch    ),
    bind(K::Char('n'), M::NONE   , Action::SearchNext     ),
    bind(K::Char('N'), M::SHIFT  , Action::SearchPrev     ),
    bind(K::Char(':'), M::NONE   , Action::EnterCommand   ),
    bind(K::Enter    , M::CONTROL, Action::OpenInNewTab   ),
    bind(K::Char('m'), M::NONE   , Action::OpenMenu       ),
    bind(K::Char('h'), M::CONTROL, Action::FocusLeft      ),
    bind(K::Char('l'), M::CONTROL, Action::FocusRight     ),
    bind(K::Char('k'), M::CONTROL, Action::EnterCrumbs    ),
    bind(K::Char('?'), M::SHIFT  , Action::Help           ),
];

/* The toolbar buttons, left to right, with the breadcrumb between the nav
   group and the rest. Ctrl+h / Ctrl+l step across the three groups; h and l
   walk the buttons inside one. */
pub const NAV_BTNS: usize = 3;
pub const TOOLBAR_BTNS: &[Action] = &[
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
pub const CHORDS: &[(char, char, Action)] = &[
   /* lead  then action               */
      ('g', 'g', Action::Top         ),
      ('g', 'h', Action::GoHome      ),
      ('g', 't', Action::NextTab     ),
      ('g', 'T', Action::PrevTab     ),
      ('g', 'u', Action::GoUp        ),
      ('z', 'a', Action::ToggleExpand),
      ('z', 'v', Action::CycleView   ),
      ('c', 'w', Action::Rename      ),
];

/* Leaders that must wait for a second key rather than acting immediately. */
pub const CHORD_LEADERS: &[char] = &['g', 'z', 'c'];
