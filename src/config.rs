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
    pub const FOLDER_OPEN  :&str = "\u{1f5c1}"; /* 🗁 expanded in Details        */
    pub const FOLDER_LOCKED:&str = "\u{f1aa8}"; /* 󱪨 no permission to enter     */
    pub const FILE         :&str = "\u{1f5cb}"; /* 🗋 regular file               */
    pub const SYMLINK      :&str = "\u{2937}" ; /* ⤷ link                       */
    pub const HOME         :&str = "\u{f02dc}"; /* 󰋜 Places: Home               */
    pub const TRASH        :&str = "\u{f014}" ; /*   Places: Trash              */
    pub const NETWORK      :&str = "\u{f0556}"; /* 󰕖 Places: Network            */
    pub const DEVICE       :&str = "\u{f02ca}"; /* 󰋊 a mounted partition        */
    pub const DEVICE_OFF   :&str = "\u{f104c}"; /* 󱁌 unmounted, unreachable     */
    pub const DEVICE_USB   :&str = "\u{f11f0}"; /* 󱇰 removable media            */
    pub const EJECT        :&str = "\u{f01ea}"; /* 󰇪 unmount a removable        */
    pub const CLOCK        :&str = "\u{f0e17}"; /* 󰸗 Places: Recent             */
    pub const DOWNLOAD     :&str = "\u{f409}" ; /*   Downloads                  */
    pub const DOCUMENT     :&str = "\u{eaf0}" ; /*   Documents                  */
    pub const MUSIC        :&str = "\u{266a}" ; /* ♪ Places: Music              */
    pub const PICTURE      :&str = "\u{1f5bb}"; /* 🖻  Places: Pictures          */
    pub const VIDEO        :&str = "\u{1f5b7}"; /* 🖷  Places: Videos            */
    pub const ARCHIVE      :&str = "\u{1f5c3}"; /* 🗃 archive file              */
    pub const BACK         :&str = "\u{efc3}" ; /*   toolbar back               */
    pub const FORWARD      :&str = "\u{edfb}" ; /*   toolbar forward            */
    pub const VIEW_ICONS   :&str = "\u{f0570}"; /* 󰕰 toolbar view button        */
    pub const VIEW_COMPACT :&str = "\u{f02be}"; /* 󰊾 toolbar view button        */
    pub const VIEW_DETAILS :&str = "\u{ef81}" ; /*   toolbar view button        */
    pub const SPLIT        :&str = "\u{f4b4}" ; /*   toolbar Split              */
    pub const SEARCH       :&str = "\u{1f50d}"; /* 🔍 toolbar Search            */
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
pub const NAME_LINES          :  u16 =    3; /* rows a name may wrap over in the icon view      */
pub const DISK_POLL_MS        :  u64 = 2000; /* how often the status bar re-measures free space */

/* Icon cell *pitch*: each cell keeps a blank row on top and CELL_GAP blank
 * columns, which is where the cursor frame is drawn. Content is therefore
 * (CELL_W - CELL_GAP) by (CELL_H - 1) — one icon row and three of name, the
 * 13x4 the layout was designed around. Compact derives its width from this. */
pub const CELL_W              :  u16 =   15; /* icon-view cell pitch, columns                    */
pub const CELL_H              :  u16 =    5; /* icon-view cell pitch, rows                       */

/* Dolphin badges the XDG user directories in the view, not just in Places.
 * Matched by name and only directly under $HOME, so a Downloads you made
 * somewhere else stays a plain folder. */
pub const HOME_FOLDER_ICONS: &[(&str, &str)] = &[
    /* name         glyph          */
      ("Documents", glyph::DOCUMENT),
      ("Downloads", glyph::DOWNLOAD),
      ("Music"    , glyph::MUSIC   ),
      ("Pictures" , glyph::PICTURE ),
      ("Videos"   , glyph::VIDEO   ),
];

/* File classification by extension. */
pub const IMAGE_EXTS   : &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff"];
pub const ARCHIVE_EXTS : &[&str] = &["tar", "gz" , "tgz" , "bz2", "xz" , "zst" , "zip", "7z" , "rar"];

// ---------------------------------------------------------------------------
// Keymap
// ---------------------------------------------------------------------------

use crossterm::event::{KeyCode as K, KeyModifiers as M};

use crate::vim::{b, Action, Bind};

/* Dolphin's native shortcuts. Active in every non-text mode, alongside vim. */
pub const DOLPHIN_KEYS: &[Bind] = &[
    /* key          modifiers                   action                 */
    b(K::Left     , M::ALT                    , Action::Back           ),
    b(K::Right    , M::ALT                    , Action::Forward        ),
    b(K::Up       , M::ALT                    , Action::GoUp           ),
    b(K::Backspace, M::NONE                   , Action::GoUp           ),
    b(K::Home     , M::ALT                    , Action::GoHome         ),
    b(K::F(5)     , M::NONE                   , Action::Refresh        ),
    b(K::Enter    , M::NONE                   , Action::Open           ),
    b(K::Down     , M::NONE                   , Action::MoveDown       ),
    b(K::Up       , M::NONE                   , Action::MoveUp         ),
    b(K::Left     , M::NONE                   , Action::MoveLeft       ),
    b(K::Right    , M::NONE                   , Action::MoveRight      ),
    b(K::Home     , M::NONE                   , Action::Top            ),
    b(K::End      , M::NONE                   , Action::Bottom         ),
    b(K::PageDown , M::NONE                   , Action::PageDown       ),
    b(K::PageUp   , M::NONE                   , Action::PageUp         ),
    b(K::Char(' '), M::NONE                   , Action::ToggleSelect   ),
    b(K::Char('a'), M::CONTROL                , Action::SelectAll      ),
    b(K::Char('A'), M::CONTROL.union(M::SHIFT), Action::InvertSelect   ),
    b(K::Char('c'), M::CONTROL                , Action::Copy           ),
    b(K::Char('x'), M::CONTROL                , Action::Cut            ),
    b(K::Char('v'), M::CONTROL                , Action::Paste          ),
    b(K::Delete   , M::NONE                   , Action::Trash          ),
    b(K::Delete   , M::SHIFT                  , Action::DeletePerm     ),
    b(K::F(2)     , M::NONE                   , Action::Rename         ),
    b(K::F(10)    , M::NONE                   , Action::NewFolder      ),
    b(K::Char('n'), M::CONTROL.union(M::SHIFT), Action::NewFolder      ),
    b(K::Enter    , M::ALT                    , Action::Properties     ),
    b(K::Char('1'), M::CONTROL                , Action::ViewIcons      ),
    b(K::Char('2'), M::CONTROL                , Action::ViewCompact    ),
    b(K::Char('3'), M::CONTROL                , Action::ViewDetails    ),
    b(K::F(3)     , M::NONE                   , Action::ToggleSplit    ),
    b(K::Tab      , M::NONE                   , Action::SwapPane       ),
    b(K::F(9)     , M::NONE                   , Action::TogglePlaces   ),
    b(K::F(11)    , M::NONE                   , Action::ToggleInfo     ),
    b(K::Char('i'), M::CONTROL                , Action::ToggleFilterBar),
    b(K::Char('t'), M::CONTROL                , Action::NewTab         ),
    b(K::Char('w'), M::CONTROL                , Action::CloseTab       ),
    b(K::Tab      , M::CONTROL                , Action::NextTab        ),
    b(K::BackTab  , M::CONTROL.union(M::SHIFT), Action::PrevTab        ),
    b(K::Char('f'), M::CONTROL                , Action::EnterSearch    ),
    b(K::F(6)     , M::NONE                   , Action::EnterPathEdit  ),
    b(K::F(4)     , M::NONE                   , Action::TerminalPanel  ),
    b(K::F(4)     , M::SHIFT                  , Action::TerminalHere   ),
    b(K::F(1)     , M::NONE                   , Action::Help           ),
    b(K::Char('q'), M::CONTROL                , Action::QuitAll        ),
];

/* The vim layer. Consulted first, so `h`/`j`/`k`/`l` are motions; any
 * printable key that lands in neither table falls through to Dolphin's
 * type-ahead jump-to-name. */
pub const VIM_KEYS: &[Bind] = &[
    /* key          modifiers   action                 */
    b(K::Char('h'), M::NONE   , Action::MoveLeft       ),
    b(K::Char('j'), M::NONE   , Action::MoveDown       ),
    b(K::Char('k'), M::NONE   , Action::MoveUp         ),
    b(K::Char('l'), M::NONE   , Action::MoveRight      ),
    b(K::Char('G'), M::SHIFT  , Action::Bottom         ),
    b(K::Char('H'), M::SHIFT  , Action::ToggleHidden   ),
    b(K::Char('d'), M::CONTROL, Action::HalfPageDown   ),
    b(K::Char('u'), M::CONTROL, Action::HalfPageUp     ),
    b(K::Char('f'), M::CONTROL, Action::PageDown       ),
    b(K::Char('b'), M::CONTROL, Action::PageUp         ),
    b(K::Char('0'), M::NONE   , Action::RowStart       ),
    b(K::Char('$'), M::NONE   , Action::RowEnd         ),
    b(K::Char('v'), M::NONE   , Action::EnterVisual    ),
    b(K::Char('V'), M::SHIFT  , Action::EnterVisualLine),
    b(K::Char('y'), M::NONE   , Action::Copy           ),
    b(K::Char('d'), M::NONE   , Action::DeleteOp       ),
    b(K::Char('p'), M::NONE   , Action::Paste          ),
    b(K::Char('P'), M::SHIFT  , Action::DropIn         ),
    b(K::Char('x'), M::NONE   , Action::Trash          ),
    b(K::Char('r'), M::NONE   , Action::Rename         ),
    b(K::Char('o'), M::NONE   , Action::NewFile        ),
    b(K::Char('O'), M::SHIFT  , Action::NewFolder      ),
    b(K::Char('u'), M::NONE   , Action::Undo           ),
    b(K::Char('D'), M::SHIFT  , Action::DragOut        ),
    b(K::Char('/'), M::NONE   , Action::EnterSearch    ),
    b(K::Char('n'), M::NONE   , Action::SearchNext     ),
    b(K::Char('N'), M::SHIFT  , Action::SearchPrev     ),
    b(K::Char(':'), M::NONE   , Action::EnterCommand   ),
    b(K::Enter    , M::CONTROL, Action::OpenInNewTab   ),
    b(K::Char('m'), M::NONE   , Action::OpenMenu       ),
    b(K::Char('h'), M::CONTROL, Action::FocusLeft      ),
    b(K::Char('l'), M::CONTROL, Action::FocusRight     ),
    b(K::Char('k'), M::CONTROL, Action::EnterCrumbs    ),
    b(K::Char('?'), M::SHIFT  , Action::Help           ),
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
