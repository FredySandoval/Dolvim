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
    pub const ERROR     :Color = Color::Rgb(218,  68,  83); /* status bar errors               */
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
pub const TYPEAHEAD_TIMEOUT_MS:  u64 = 1000; /* type-ahead buffer life without a keystroke      */
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
 * The column width follows the style rather than being restated beside it. */
pub const TIME_STYLE          :TimeStyle = TimeStyle::Short;
/* `Modified` at its widest: `Sep 30 2025, 12:22pm` / `2026-08-02 20:22`. */
pub const TIME_WIDTH          :  u16 = match TIME_STYLE {
    TimeStyle::Short => 20,
    TimeStyle::Iso   => 16,
};
pub const SIZE_WIDTH          :  u16 =   12; /* Details `Size` column                           */
pub const TYPE_WIDTH          :  u16 =   14; /* Details `Type` column                           */
pub const PROGRESS_POPUP_W    :  u16 =   60; /* transfer popup, columns                         */
pub const PROGRESS_POPUP_H    :  u16 =    6; /* transfer popup, rows                            */
/* The meter inside the popup: its width less the borders and their padding. */
pub const PROGRESS_BAR_WIDTH  :usize = PROGRESS_POPUP_W as usize - 4;
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

/* File classification by extension. No `svg`: thumbnails come from the `image`
 * crate, which decodes rasters only, so an svg would badge as an image and then
 * never produce one. */
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
 * the combinations here keeps that spelling out of every row.
 *
 * SHIFT on a `Char` row is documentation: `vim::normalize` strips it from the
 * event and from the row alike, because terminals disagree about reporting it
 * and the character's own case says it anyway. Spell the character shifted —
 * `G`, `N` — or the row says one thing and matches another.
 *
 * CTRL is the same disagreement one step further on. The legacy encoding sends
 * Ctrl+letter as a bare control byte — 0x01 for Ctrl+A — which has no room for
 * case, so Ctrl+Shift+A arrives indistinguishable from Ctrl+A and Ctrl+I from
 * Tab. `main::enter_raw_screen` asks for the kitty keyboard protocol, which
 * reports the real event; the two CTRL_SHIFT rows below need it, and on a
 * terminal without it they simply never fire. Everything else is unaffected. */
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
    /* Space was ToggleSelect; it is the leader now (see CHORDS) and a leader
       cannot also be a binding. Toggling a row is `v`, or the mouse. */
    bind(Char('a') , CTRL      , Action::SelectAll      ),
    /* Needs the kitty keyboard protocol; without it this key is Ctrl+A and
       selects all. See the modifier note above. */
    bind(Char('A') , CTRL_SHIFT, Action::InvertSelect   ),
    bind(Char('c') , CTRL      , Action::Copy           ),
    bind(Char('x') , CTRL      , Action::Cut            ),
    bind(Char('v') , CTRL      , Action::Paste          ),
    bind(Delete    , NONE      , Action::Trash          ),
    bind(Delete    , SHIFT     , Action::DeletePerm     ),
    bind(F(2)      , NONE      , Action::Rename         ),
    bind(F(10)     , NONE      , Action::NewFolder      ),
    /* Likewise; F10 is the one that works everywhere. */
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
    /* The shift that produced BackTab is spent on it; what reaches us is
       Ctrl+BackTab, so that is what the row says. The legacy encoding has no
       room for the Ctrl either and sends a bare BackTab, hence the second row —
       Shift+Tab means nothing else here, so it can mean this everywhere. */
    bind(BackTab   , CTRL      , Action::PrevTab        ),
    bind(BackTab   , NONE      , Action::PrevTab        ),
    /* Dolphin's Ctrl+F is Find, but the vim table is consulted first and there
       Ctrl+F is a page down, which wins in a program called Dolvim. Search is
       `/`, the toolbar button, or Ctrl+I for the filter bar. */
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
    /* Vimium's back/forward pair, pointed at the folder tree. `H` used to be
       ToggleHidden, which is `<Space>h` now. */
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
    /* `y` is unbound: in vim it is an operator, and here it was not, which made
       it the odd key out beside `d`. Copy is Ctrl+C until `y{motion}` exists. */
    bind(Char('d') , NONE , Action::DeleteOp       ),
    bind(Char('p') , NONE , Action::Paste          ),
    /* `P` is unbound: it held DropIn, which is not a paste, and in vim `p`/`P`
       differ only in where the same paste lands. */
    bind(Char('x') , NONE , Action::Trash          ),
    /* `r` is unbound: vim's `r` replaces a character and waits for one. Rename
       is `cw`, which is what vim calls it anyway, or F2. */
    bind(Char('o') , NONE , Action::NewFile        ),
    bind(Char('O') , SHIFT, Action::NewFolder      ),
    bind(Char('u') , NONE , Action::Undo           ),
    /* `D` is unbound: vim's `D` is `d$`, not a drag. */
    bind(Char('/') , NONE , Action::EnterSearch    ),
    bind(Char('n') , NONE , Action::SearchNext     ),
    bind(Char('N') , SHIFT, Action::SearchPrev     ),
    bind(Char(':') , NONE , Action::EnterCommand   ),
    bind(Enter     , CTRL , Action::OpenInNewTab   ),
    /* Vim's marks. A file manager's "line" is its folder, so `ma` remembers a
       folder and `'a` returns to it. The menu `m` used to open is still the
       hamburger button — Ctrl+k into the toolbar row, or the mouse. */
    bind(Char('m') , NONE , Action::SetMark        ),
    bind(Char('\''), NONE , Action::JumpMark       ),
    bind(Char('h') , CTRL , Action::FocusLeft      ),
    bind(Char('l') , CTRL , Action::FocusRight     ),
    bind(Char('k') , CTRL , Action::EnterCrumbs    ),
    /* `?` is unbound: it is search-backward in vim, and this program has no
       backward search yet. Help is F1. */
];

/* The toolbar buttons, left to right, with the breadcrumb between the two
   groups. Ctrl+h / Ctrl+l step across the three panes of the row; h and l walk
   the buttons inside one. The group boundary is `NAV_BUTTONS.len()` — a button
   moved between the tables moves the boundary with it. */
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
    /* Space is the leader. It starts sequences and so can bind nothing on its
       own — the row it used to toggle is `v`'s job now. */
    chord(' '  , 'h'     , Action::ToggleHidden),
];

/// The tables above are data, and data drifts: a row added twice, a leader that
/// is also a binding, a menu whose owning button moved. None of that is a
/// compile error, so it is a test — the price of configuration being source.
#[cfg(test)]
mod sanity {
    use super::*;
    use crossterm::event::KeyCode;
    use crate::vim::{menu_owner, normalize_mods, toolbar_buttons, MENU_BUTTONS};

    /// Bindings the vim table knowingly takes over from Dolphin's. An entry
    /// here is a decision someone made; anything else is a collision.
    const SHADOWS: &[(KeyCode, KeyModifiers)] = &[];

    /// Every row as the lookup will actually see it.
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

    /// A `Char` row whose modifiers still name SHIFT after normalization is a
    /// row that cannot match: the shift went into the character. Spell it
    /// shifted — `N`, not `n` — or drop the modifier.
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

    /// A leader that is also a binding acts on the first press, so its chords
    /// can never start.
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

    /// `vim::menu_owner` finds a button by its action, so an action that names
    /// a menu has to sit at exactly one place in the row — there once, and no
    /// more than once. The menus are read from `vim::MENU_BUTTONS`, the one
    /// place that says which they are, so a third menu cannot pass untested.
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

    /// Both groups have to be inhabited: crossing the breadcrumb lands on the
    /// first or last button of the group opposite, and an empty one has none.
    #[test]
    fn both_toolbar_groups_are_inhabited() {
        assert!(!NAV_BUTTONS.is_empty() && !RIGHT_BUTTONS.is_empty());
    }
}
