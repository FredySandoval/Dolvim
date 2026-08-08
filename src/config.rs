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

use crate::vim::{bind, chord, Action, Bind, BindMode, Chord};

/* key modifiers
 * SHIFT characters use their shifted spelling (`G`, not `g`).
 * CTRL_SHIFT characters require the kitty keyboard protocol.
 */
const NONE      : KeyModifiers = KeyModifiers::NONE;
const ALT       : KeyModifiers = KeyModifiers::ALT;
const CTRL      : KeyModifiers = KeyModifiers::CONTROL;
const SHIFT     : KeyModifiers = KeyModifiers::SHIFT;
const CTRL_SHIFT: KeyModifiers = KeyModifiers::CONTROL.union(KeyModifiers::SHIFT);

const NORMAL      : BindMode = BindMode::Normal;
const VISUAL      : BindMode = BindMode::Visual;
const VISUAL_LINE : BindMode = BindMode::VisualLine;
const PLACES      : BindMode = BindMode::Places;
const TABS        : BindMode = BindMode::Tabs;
const COMMAND     : BindMode = BindMode::Command;
const SEARCH      : BindMode = BindMode::Search;
const FILTER      : BindMode = BindMode::Filter;
const PATH_EDIT   : BindMode = BindMode::PathEdit;
const RENAME      : BindMode = BindMode::Rename;
const BATCH_RENAME: BindMode = BindMode::BatchRename;
const NEW_FOLDER  : BindMode = BindMode::NewFolder;
const NEW_FILE    : BindMode = BindMode::NewFile;
const CONFIRM     : BindMode = BindMode::Confirm;
const PROPERTIES  : BindMode = BindMode::Properties;
const HELP        : BindMode = BindMode::Help;
const CRUMB_MENU  : BindMode = BindMode::CrumbMenu;
const BUTTONS     : BindMode = BindMode::Buttons;
const MENU        : BindMode = BindMode::Menu;

/* Unified mode-aware keybindings. Origins are comments, not precedence. */
pub const KEY_BINDINGS: &[Bind] = &[
    /* Places bindings */
    /*   modifier    key           modes         action                   */
    bind(NONE      , Char('h')   , &[PLACES]   , Action::PlacesIgnore    ),
    bind(NONE      , Char('j')   , &[PLACES]   , Action::PlacesDown      ),
    bind(NONE      , Char('k')   , &[PLACES]   , Action::PlacesUp        ),
    bind(NONE      , Char('l')   , &[PLACES]   , Action::PlacesOpen      ),
    bind(NONE      , Left        , &[PLACES]   , Action::PlacesIgnore    ),
    bind(NONE      , Down        , &[PLACES]   , Action::PlacesDown      ),
    bind(NONE      , Up          , &[PLACES]   , Action::PlacesUp        ),
    bind(NONE      , Right       , &[PLACES]   , Action::PlacesOpen      ),
    bind(NONE      , Enter       , &[PLACES]   , Action::PlacesAccept    ),
    bind(NONE      , Tab         , &[PLACES]   , Action::PlacesLeave     ),

    /* Tabs pane bindings */
    bind(NONE      , Char('h')   , &[TABS]     , Action::PrevTab         ),
    bind(NONE      , Char('l')   , &[TABS]     , Action::NextTab         ),
    bind(NONE      , Char('j')   , &[TABS]     , Action::TabsIgnore      ),
    bind(NONE      , Char('k')   , &[TABS]     , Action::TabsIgnore      ),
    bind(NONE      , Left        , &[TABS]     , Action::PrevTab         ),
    bind(NONE      , Down        , &[TABS]     , Action::TabsIgnore      ),
    bind(NONE      , Up          , &[TABS]     , Action::TabsIgnore      ),
    bind(NONE      , Right       , &[TABS]     , Action::NextTab         ),
    bind(CTRL      , Char('h')   , &[TABS]     , Action::FocusLeft       ),
    bind(CTRL      , Char('j')   , &[TABS]     , Action::TabsLeave       ),
    bind(CTRL      , Char('k')   , &[TABS]     , Action::EnterCrumbs     ),
    bind(CTRL      , Char('l')   , &[TABS]     , Action::FocusRight      ),

    /* Dolphin-like bindings */
    /* modifier   key           modes                                   action                   */
    bind(ALT       , Left        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Back            ),
    bind(ALT       , Right       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Forward         ),
    bind(ALT       , Up          , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::GoUp            ),
    bind(NONE      , Backspace   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::GoUp            ),
    bind(ALT       , Home        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::GoHome          ),
    bind(NONE      , F(5)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Refresh         ),
    bind(NONE      , Enter       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Open            ),
    bind(NONE      , Down        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveDown        ),
    bind(NONE      , Up          , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveUp          ),
    bind(NONE      , Left        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveLeft        ),
    bind(NONE      , Right       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveRight       ),
    bind(NONE      , Home        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Top             ),
    bind(NONE      , End         , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Bottom          ),
    bind(NONE      , PageDown    , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::PageDown        ),
    bind(NONE      , PageUp      , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::PageUp          ),
    bind(CTRL      , Char('a')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::SelectAll       ),
    bind(CTRL_SHIFT, Char('A')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::InvertSelect    ),
    bind(CTRL      , Char('c')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Copy            ),
    bind(CTRL      , Char('x')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Cut             ),
    bind(CTRL      , Char('v')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Paste           ),
    bind(NONE      , Delete      , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Trash           ),
    bind(SHIFT     , Delete      , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::DeletePerm      ),
    bind(NONE      , F(2)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Rename          ),
    bind(NONE      , F(10)       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NewFolder       ),
    bind(CTRL_SHIFT, Char('N')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NewFolder       ),
    bind(ALT       , Enter       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Properties      ),
    bind(CTRL      , Char('1')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::ViewIcons       ),
    bind(CTRL      , Char('2')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::ViewCompact     ),
    bind(CTRL      , Char('3')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::ViewDetails     ),
    bind(NONE      , F(3)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::ToggleSplit     ),
    bind(NONE      , Tab         , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::SwapPane        ),
    bind(NONE      , F(9)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::TogglePlaces    ),
    bind(NONE      , F(11)       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::ToggleInfo      ),
    bind(CTRL      , Char('i')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::ToggleFilterBar ),
    bind(CTRL      , Char('t')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NewTab          ),
    bind(CTRL      , Char('w')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::CloseTab        ),
    bind(CTRL      , Tab         , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NextTab         ),
    bind(CTRL      , BackTab     , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::PrevTab         ),
    bind(NONE      , BackTab     , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::PrevTab         ),
    bind(NONE      , F(6)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::EnterPathEdit   ),
    bind(NONE      , F(4)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::TerminalPanel   ),
    bind(SHIFT     , F(4)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::TerminalHere    ),
    bind(NONE      , F(1)        , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Help            ),
    bind(CTRL      , Char('q')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::QuitAll         ),

    /* Vim-like bindings */
    /* modifier   key           modes                                   action                   */
    bind(NONE      , Char('h')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveLeft        ),
    bind(NONE      , Char('j')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveDown        ),
    bind(NONE      , Char('k')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveUp          ),
    bind(NONE      , Char('l')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::MoveRight       ),
    bind(SHIFT     , Char('G')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Bottom          ),
    bind(SHIFT     , Char('H')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NavigateUp      ),
    bind(SHIFT     , Char('L')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NavigateInto    ),
    bind(CTRL      , Char('d')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::HalfPageDown    ),
    bind(CTRL      , Char('u')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::HalfPageUp      ),
    bind(CTRL      , Char('f')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::PageDown        ),
    bind(CTRL      , Char('b')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::PageUp          ),
    bind(NONE      , Char('0')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::RowStart        ),
    bind(NONE      , Char('$')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::RowEnd          ),
    bind(NONE      , Char('v')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::EnterVisual     ),
    bind(SHIFT     , Char('V')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::EnterVisualLine ),
    bind(NONE      , Char('d')   , &[NORMAL]                              , Action::DeleteOp        ),
    bind(NONE      , Char('d')   , &[VISUAL, VISUAL_LINE]                 , Action::DeleteSelection ),
    bind(NONE      , Char('p')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Paste           ),
    bind(NONE      , Char('x')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Trash           ),
    bind(NONE      , Char('o')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NewFile         ),
    bind(SHIFT     , Char('O')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::NewFolder       ),
    bind(NONE      , Char('u')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::Undo            ),
    bind(NONE      , Char('/')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::EnterSearch     ),
    bind(NONE      , Char('n')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::SearchNext      ),
    bind(SHIFT     , Char('N')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::SearchPrev      ),
    bind(NONE      , Char(':')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::EnterCommand    ),
    bind(CTRL      , Enter       , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::OpenInNewTab    ),
    bind(NONE      , Char('m')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::SetMark         ),
    bind(NONE      , Char('\'')  , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::JumpMark        ),
    bind(CTRL      , Char('h')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::FocusLeft       ),
    bind(CTRL      , Char('l')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::FocusRight      ),
    bind(CTRL      , Char('k')   , &[NORMAL, VISUAL, VISUAL_LINE]         , Action::EnterCrumbs     ),

    /* Text-entry bindings */
    bind(NONE      , Esc         , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::Cancel              ),
    bind(NONE      , Enter       , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::CommitInput         ),
    bind(NONE      , Backspace   , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputBackspace      ),
    bind(NONE      , Delete      , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputDelete         ),
    bind(NONE      , Left        , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputLeft           ),
    bind(NONE      , Right       , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputRight          ),
    bind(NONE      , Home        , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputHome           ),
    bind(NONE      , End         , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputEnd            ),
    bind(CTRL      , Char('u')   , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputClear          ),
    bind(CTRL      , Char('w')   , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputDeleteWord     ),
    bind(NONE      , Tab         , &[PATH_EDIT]                                                                        , Action::CompletePath        ),

    /* Confirmation and overlay bindings */
    bind(NONE      , Char('y')   , &[CONFIRM]                                                                          , Action::ConfirmAccept       ),
    bind(NONE      , Char('Y')   , &[CONFIRM]                                                                          , Action::ConfirmAccept       ),
    bind(NONE      , Enter       , &[CONFIRM]                                                                          , Action::ConfirmAccept       ),
    bind(NONE      , Esc         , &[CONFIRM, PROPERTIES, HELP]                                                        , Action::Cancel              ),
    bind(NONE      , Char('q')   , &[PROPERTIES, HELP]                                                                 , Action::Cancel              ),

    /* Menu, breadcrumb, and toolbar bindings */
    bind(NONE      , Esc         , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::Cancel              ),
    bind(NONE      , Char('q')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::Cancel              ),
    bind(NONE      , Down        , &[MENU, CRUMB_MENU]                                                                 , Action::InterfaceDown       ),
    bind(NONE      , Char('j')   , &[MENU, CRUMB_MENU]                                                                 , Action::InterfaceDown       ),
    bind(CTRL      , Char('n')   , &[MENU, CRUMB_MENU]                                                                 , Action::InterfaceDown       ),
    bind(NONE      , Up          , &[MENU, CRUMB_MENU]                                                                 , Action::InterfaceUp         ),
    bind(NONE      , Char('k')   , &[MENU, CRUMB_MENU]                                                                 , Action::InterfaceUp         ),
    bind(CTRL      , Char('p')   , &[MENU, CRUMB_MENU]                                                                 , Action::InterfaceUp         ),
    bind(NONE      , Home        , &[MENU]                                                                             , Action::InterfaceFirst      ),
    bind(NONE      , Char('g')   , &[MENU]                                                                             , Action::InterfaceFirst      ),
    bind(NONE      , End         , &[MENU]                                                                             , Action::InterfaceLast       ),
    bind(NONE      , Char('G')   , &[MENU]                                                                             , Action::InterfaceLast       ),
    bind(NONE      , Left        , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceLeft       ),
    bind(NONE      , Char('h')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceLeft       ),
    bind(NONE      , Right       , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceRight      ),
    bind(NONE      , Char('l')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceRight      ),
    bind(NONE      , Enter       , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceAccept     ),
    bind(NONE      , Tab         , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceAccept     ),
    bind(CTRL      , Char('y')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceAccept     ),
    bind(CTRL      , Char('j')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceFocusDown  ),
    bind(CTRL      , Char('k')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceFocusUp    ),
    bind(CTRL      , Char('h')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceFocusLeft  ),
    bind(CTRL      , Char('l')   , &[MENU, CRUMB_MENU, BUTTONS]                                                        , Action::InterfaceFocusRight ),
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
pub const CHORDS: &[Chord] = &[
     /* leader  follower  modes                              action               */
    chord('g'  , 'g'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::Top         ),
    chord('g'  , 'h'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::GoHome      ),
    chord('g'  , 't'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::NextTab     ),
    chord('g'  , 'T'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::PrevTab     ),
    chord('g'  , 'u'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::GoUp        ),
    chord('z'  , 'a'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::ToggleExpand),
    chord('z'  , 'v'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::CycleView   ),
    chord('c'  , 'w'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::Rename      ),
    /* Space leader; selection moved to `v` */
    chord(' '  , 'h'     , &[NORMAL, VISUAL, VISUAL_LINE]  , Action::ToggleHidden),
];

/* sanity tests */
#[cfg(test)]
mod sanity {
    use super::*;
    use crate::vim::{menu_owner, normalize_mods, toolbar_buttons, MENU_BUTTONS};
    use crossterm::event::KeyCode;

    fn bindings_overlap(left: &Bind, right: &Bind) -> bool {
        left.code == right.code
            && normalize_mods(left.code, left.mods) == normalize_mods(right.code, right.mods)
            && left.modes.iter().any(|mode| right.modes.contains(mode))
    }

    #[test]
    fn every_binding_has_distinct_modes() {
        for binding in KEY_BINDINGS {
            assert!(!binding.modes.is_empty(), "binding has no modes");
            for (index, mode) in binding.modes.iter().enumerate() {
                assert!(
                    !binding.modes[index + 1..].contains(mode),
                    "{mode:?} occurs twice in one binding"
                );
            }
        }
    }

    #[test]
    fn no_key_has_overlapping_modes() {
        for (index, left) in KEY_BINDINGS.iter().enumerate() {
            for right in &KEY_BINDINGS[index + 1..] {
                assert!(
                    !bindings_overlap(left, right),
                    "{:?} with {:?} has overlapping modes",
                    left.code,
                    normalize_mods(left.code, left.mods)
                );
            }
        }
    }

    #[test]
    fn overlap_check_is_mode_aware_and_normalized() {
        let normal = bind(NONE, Char('x'), &[NORMAL], Action::Trash);
        let visual = bind(NONE, Char('x'), &[VISUAL], Action::Trash);
        let both = bind(NONE, Char('x'), &[NORMAL, VISUAL], Action::Trash);
        let shifted = bind(SHIFT, Char('x'), &[NORMAL], Action::Trash);

        assert!(!bindings_overlap(&normal, &visual));
        assert!(bindings_overlap(&normal, &both));
        assert!(bindings_overlap(&normal, &shifted));
    }

    /* SHIFT keybindings use uppercase characters. */
    #[test]
    fn shift_rows_spell_the_shifted_character() {
        for b in KEY_BINDINGS {
            if let KeyCode::Char(c) = b.code {
                assert!(
                    !(b.mods.contains(SHIFT) && c.is_lowercase()),
                    "{c:?} is bound with SHIFT but spelled lowercase"
                );
            }
        }
    }

    /* chord leaders must be unbound in the same modes. */
    #[test]
    fn chord_leaders_are_not_bindings() {
        for chord in CHORDS {
            let bound = KEY_BINDINGS.iter().any(|binding| {
                binding.code == KeyCode::Char(chord.leader)
                    && normalize_mods(binding.code, binding.mods) == NONE
                    && binding.modes.iter().any(|mode| chord.modes.contains(mode))
            });
            assert!(!bound, "chord leader {:?} is also a binding", chord.leader);
        }
    }

    #[test]
    fn chords_have_distinct_modes_and_do_not_overlap() {
        for (index, chord) in CHORDS.iter().enumerate() {
            assert!(!chord.modes.is_empty(), "chord has no modes");
            for (mode_index, mode) in chord.modes.iter().enumerate() {
                assert!(
                    !chord.modes[mode_index + 1..].contains(mode),
                    "{mode:?} occurs twice in one chord"
                );
            }
            for other in &CHORDS[index + 1..] {
                let overlaps = chord.leader == other.leader
                    && chord.follower == other.follower
                    && chord.modes.iter().any(|mode| other.modes.contains(mode));
                assert!(!overlaps, "chord {:?} overlaps", (chord.leader, chord.follower));
            }
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
