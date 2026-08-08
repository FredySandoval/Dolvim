//! User configuration. Edit and recompile.
//! Format with `cargo +nightly fmt`; table alignment requires nightly rustfmt.

use ratatui::style::Color;

use crate::fs::TimeStyle;

/* colors: Breeze Light 24-bit samples; see docs/UI_SPEC.md */
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
    pub const ERROR     :Color = Color::Rgb(218,  68,  83); /* status bar errors               */
    pub const GAUGE_FULL:Color = Color::Rgb(200, 205, 210); /* used part of a device capacity  */
    pub const OFFLINE   :Color = Color::Rgb(246, 116,   0); /* unmounted device, locked folder */
}

/* icons / glyphs; private-use glyphs require a Nerd Font */
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

/* layout, panels, and input */
pub const PLACES_WIDTH        :  u16 =   22; /* Places panel width in terminal columns          */
pub const INFO_WIDTH          :  u16 =   30; /* Information panel columns (F11)                 */
pub const TYPEAHEAD_TIMEOUT_MS:  u64 = 1000; /* type-ahead buffer life without a keystroke      */

/* mouse */
pub const DOUBLE_CLICK_MS     :  u64 =  400; /* second click inside this counts as a double     */

/* filesystem and event loop */
pub const WATCH_DEBOUNCE_MS   :  u64 =  120; /* refresh delay after changes                     */
pub const TICK_MS             :  u64 =   40; /* event loop tick; listing/thumbnail poll rate    */
pub const DRAG_THRESHOLD      :  u16 =    1; /* cells of movement before a drag begins          */

/* thumbnails and file view */
pub const THUMB_CACHE_CAP     :usize =  512; /* decoded thumbnails held in memory               */
pub const THUMB_MAX_INFLIGHT  :usize =   32; /* decodes queued at once; the rest wait           */
pub const CELL_GAP            :  u16 =    2; /* blank columns between icon-view tiles           */
pub const VIEW_MARGIN         :  u16 =    1; /* blank columns left of Compact and Details rows  */
pub const NAME_LINES          :  u16 =    3; /* rows a name may wrap over in the icon view      */

/* status, transfers, and Recent files */
pub const DISK_POLL_MS        :  u64 =   2000; /* how often the status bar re-measures free space */
pub const COPY_CHUNK_BYTES    :usize = 262144; /* streaming copy buffer; cancel granularity       */
pub const RECENT_MAX_DEPTH    :  u32 =      3; /* how deep a Recent search walks                  */
pub const RECENT_MAX_ITEMS    :usize =   2000; /* results a Recent search stops at                */

/* Details timestamp*/
pub const TIME_STYLE          :TimeStyle = TimeStyle::Short; /* options: Short | Iso*/

/* Details columns */
pub const TIME_WIDTH : u16 = match TIME_STYLE {
    TimeStyle::Short => 20,
    TimeStyle::Iso   => 16,
};
pub const SIZE_WIDTH          :  u16 =   12; /* Details `Size` column                           */
pub const TYPE_WIDTH          :  u16 =   14; /* Details `Type` column                           */

/* copy/move progress popup, centered on screen */
pub const PROGRESS_POPUP_W    :  u16 =   60; /* transfer popup, columns                         */
pub const PROGRESS_POPUP_H    :  u16 =    6; /* transfer popup, rows                            */

/* transfer progress bar: popup width minus borders and padding */
pub const PROGRESS_BAR_WIDTH  :usize = PROGRESS_POPUP_W as usize - 4;

/* toolbar: space reserved right of the breadcrumb */
pub const TOOLBAR_RIGHT_WIDTH :  u16 =   22; /* right-hand toolbar controls, columns            */

/* icon view cell pitch; includes cursor-frame gaps */
pub const CELL_WIDTH          :  u16 =   15; /* icon-view cell pitch, columns                   */
pub const CELL_HEIGHT         :  u16 =    5; /* icon-view cell pitch, rows                      */

/* Places / XDG directories and home-folder badges */
pub struct XdgDir {
    pub env_key: &'static str,
    pub name   : &'static str,
    pub glyph  : &'static str,
}

/* XDG key, display name, glyph */
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

/* file types; SVG omitted because thumbnails are raster-only */
pub const IMAGE_EXTS   : &[&str] = &["png", "jpg", "jpeg", "gif", "bmp", "webp", "ico", "tif", "tiff"];
pub const ARCHIVE_EXTS : &[&str] = &["tar", "gz" , "tgz" , "bz2", "xz" , "zst" , "zip", "7z" , "rar"];

/* keybindings */

use crossterm::event::KeyCode::*;
use crossterm::event::KeyModifiers;

use crate::vim::{bind, Action, Bind};

/* key modifiers
 * SHIFT characters use their shifted spelling (`G`, not `g`).
 * CTRL_SHIFT characters require the kitty keyboard protocol.
 */
const NONE      : KeyModifiers = KeyModifiers::NONE;
const ALT       : KeyModifiers = KeyModifiers::ALT;
const CTRL      : KeyModifiers = KeyModifiers::CONTROL;
const SHIFT     : KeyModifiers = KeyModifiers::SHIFT;
const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

/* Dolphin keybindings */
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
    /* selection: Space is the chord leader; use `v` or the mouse */
    bind(Char('a') , CTRL      , Action::SelectAll      ),
    /* kitty keyboard protocol */
    bind(Char('A') , CTRL_SHIFT, Action::InvertSelect   ),
    bind(Char('c') , CTRL      , Action::Copy           ),
    bind(Char('x') , CTRL      , Action::Cut            ),
    bind(Char('v') , CTRL      , Action::Paste          ),
    bind(Delete    , NONE      , Action::Trash          ),
    bind(Delete    , SHIFT     , Action::DeletePerm     ),
    bind(F(2)      , NONE      , Action::Rename         ),
    bind(F(10)     , NONE      , Action::NewFolder      ),
    /* kitty keyboard protocol; F10 is the fallback */
    bind(Char('N') , CTRL_SHIFT, Action::NewFolder      ),
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
    /* previous tab: modern and legacy terminal encodings */
    bind(BackTab   , CTRL      , Action::PrevTab        ),
    bind(BackTab   , NONE      , Action::PrevTab        ),
    /* search: `/` or toolbar; Ctrl+F remains Vim page-down */
    bind(F(6)      , NONE      , Action::EnterPathEdit  ),
    bind(F(4)      , NONE      , Action::TerminalPanel  ),
    bind(F(4)      , SHIFT     , Action::TerminalHere   ),
    bind(F(1)      , NONE      , Action::Help           ),
    bind(Char('q') , CTRL      , Action::QuitAll        ),
];

/* Vim keybindings; checked before Dolphin, then type-ahead search */
pub const VIM_KEYS: &[Bind] = &[
    /*    key         mods    action                  */
    bind(Char('h') , NONE , Action::MoveLeft       ),
    bind(Char('j') , NONE , Action::MoveDown       ),
    bind(Char('k') , NONE , Action::MoveUp         ),
    bind(Char('l') , NONE , Action::MoveRight      ),
    bind(Char('G') , SHIFT, Action::Bottom         ),
    /* folder navigation; hidden files use `<Space>h` */
    bind(Char('H') , SHIFT, Action::NavigateUp     ),
    bind(Char('L') , SHIFT, Action::NavigateInto   ),
    bind(Char('d') , CTRL , Action::HalfPageDown   ),
    bind(Char('u') , CTRL , Action::HalfPageUp     ),
    bind(Char('f') , CTRL , Action::PageDown       ),
    bind(Char('b') , CTRL , Action::PageUp         ),
    bind(Char('0') , NONE , Action::RowStart       ),
    bind(Char('$') , NONE , Action::RowEnd         ),
    bind(Char('v') , NONE , Action::EnterVisual    ),
    bind(Char('V') , SHIFT, Action::EnterVisualLine),
    /* unbound: `y` reserved for a future Vim operator; copy is Ctrl+C */
    bind(Char('d') , NONE , Action::DeleteOp       ),
    bind(Char('p') , NONE , Action::Paste          ),
    /* unbound: `P` reserved for Vim paste-before */
    bind(Char('x') , NONE , Action::Trash          ),
    /* unbound: `r` reserved for Vim replace; rename is `cw` or F2 */
    bind(Char('o') , NONE , Action::NewFile        ),
    bind(Char('O') , SHIFT, Action::NewFolder      ),
    bind(Char('u') , NONE , Action::Undo           ),
    /* unbound: `D` reserved for Vim `d$` */
    bind(Char('/') , NONE , Action::EnterSearch    ),
    bind(Char('n') , NONE , Action::SearchNext     ),
    bind(Char('N') , SHIFT, Action::SearchPrev     ),
    bind(Char(':') , NONE , Action::EnterCommand   ),
    bind(Enter     , CTRL , Action::OpenInNewTab   ),
    /* marks: `ma` saves a folder; `'a` returns to it */
    bind(Char('m') , NONE , Action::SetMark        ),
    bind(Char('\''), NONE , Action::JumpMark       ),
    bind(Char('h') , CTRL , Action::FocusLeft      ),
    bind(Char('l') , CTRL , Action::FocusRight     ),
    bind(Char('k') , CTRL , Action::EnterCrumbs    ),
    /* unbound: `?` reserved for backward search; help is F1 */
];

/* toolbar buttons: navigation, breadcrumb, right controls */
pub const NAV_BUTTONS: &[Action] = &[
    Action::Back,
    Action::Forward,
    Action::OpenViewMenu,
];
pub const RIGHT_BUTTONS: &[Action] = &[
    Action::ToggleSplit,
    Action::EnterSearch,
    Action::OpenMenu,
];

/* chord keybindings */
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
    /* Space leader; selection moved to `v` */
    chord(' '  , 'h'     , Action::ToggleHidden),
];

/* sanity tests */
#[cfg(test)]
mod sanity {
    use super::*;
    use crossterm::event::KeyCode;
    use crate::vim::{menu_owner, normalize_mods, toolbar_buttons, MENU_BUTTONS};

    /* allowlist: Vim/Dolphin keybinding collisions */
    const SHADOWS: &[(KeyCode, KeyModifiers)] = &[];

    /* normalized keybindings */
    fn normalized(table: &[Bind]) -> Vec<(KeyCode, KeyModifiers)> {
        table
            .iter()
            .map(|b| (b.code, normalize_mods(b.code, b.mods)))
            .collect()
    }

    #[test]
    fn no_key_is_bound_twice() {
        let mut seen: Vec<(KeyCode, KeyModifiers)> = Vec::new();
        for key in normalized(VIM_KEYS).into_iter().chain(normalized(DOLPHIN_KEYS)) {
            assert!(
                !seen.contains(&key) || SHADOWS.contains(&key),
                "{key:?} is bound twice; only the first row can ever fire"
            );
            seen.push(key);
        }
    }

    /* SHIFT keybindings use uppercase characters. */
    #[test]
    fn shift_rows_spell_the_shifted_character() {
        for b in VIM_KEYS.iter().chain(DOLPHIN_KEYS) {
            if let KeyCode::Char(c) = b.code {
                assert!(
                    !(b.mods.contains(SHIFT) && c.is_lowercase()),
                    "{c:?} is bound with SHIFT but spelled lowercase"
                );
            }
        }
    }

    /* chord leaders must be unbound. */
    #[test]
    fn chord_leaders_are_not_bindings() {
        for chord in CHORDS {
            let bound = VIM_KEYS
                .iter()
                .chain(DOLPHIN_KEYS)
                .any(|b| b.code == KeyCode::Char(chord.leader) && b.mods == NONE);
            assert!(!bound, "chord leader {:?} is also a binding", chord.leader);
        }
    }

    #[test]
    fn no_chord_is_written_twice() {
        let mut seen: Vec<(char, char)> = Vec::new();
        for chord in CHORDS {
            let pair = (chord.leader, chord.follower);
            assert!(!seen.contains(&pair), "chord {pair:?} is written twice");
            seen.push(pair);
        }
    }

    /* each toolbar menu has one owning button. */
    #[test]
    fn each_menu_hangs_from_exactly_one_button() {
        for (kind, menu_action) in MENU_BUTTONS {
            let n = toolbar_buttons().filter(|a| a == menu_action).count();
            assert_eq!(n, 1, "{menu_action:?} appears {n} times in the toolbar");
            assert!(
                menu_owner(kind).is_some(),
                "{kind:?} names a button that is not in the row"
            );
        }
    }

    /* toolbar navigation requires both button groups. */
    #[test]
    fn both_toolbar_groups_are_inhabited() {
        assert!(!NAV_BUTTONS.is_empty() && !RIGHT_BUTTONS.is_empty());
    }
}
