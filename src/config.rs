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
    pub const HOVER     :Color = Color::Rgb(224, 239, 249); /* item under the active cursor    */
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

/* MIME types without an OS association that should use the editor role */
pub const EDITOR_MIME_TYPES   :&[&str] = &["application/javascript", "application/json", "application/sql", "application/toml", "application/xml", "application/x-perl", "application/x-shellscript", "application/x-yaml", "application/yaml", "inode/x-empty"];
pub const EDITOR_MIME_SUFFIXES:&[&str] = &["+json", "+xml"];

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

use crate::app::Direction;
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
const VISUAL_BLOCK: BindMode = BindMode::VisualBlock;
const VIEW_MODES  :&[BindMode] = &[NORMAL, VISUAL, VISUAL_LINE, VISUAL_BLOCK];
const VISUAL_MODES:&[BindMode] = &[VISUAL, VISUAL_LINE, VISUAL_BLOCK];
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
/*  modifier       key            modes          action   */
    bind(NONE    , Char('h')    , &[PLACES]    , Action::NoOp                    ),
    bind(NONE    , Char('j')    , &[PLACES]    , Action::PlacesDown              ),
    bind(NONE    , Char('k')    , &[PLACES]    , Action::PlacesUp                ),
    bind(NONE    , Char('l')    , &[PLACES]    , Action::PlacesOpen              ),
    bind(NONE    , Left         , &[PLACES]    , Action::NoOp                    ),
    bind(NONE    , Down         , &[PLACES]    , Action::PlacesDown              ),
    bind(NONE    , Up           , &[PLACES]    , Action::PlacesUp                ),
    bind(NONE    , Right        , &[PLACES]    , Action::PlacesOpen              ),
    bind(NONE    , Enter        , &[PLACES]    , Action::PlacesAccept            ),
    bind(NONE    , Tab          , &[PLACES]    , Action::Focus(Direction::Right) ),
    bind(CTRL    , Char('h')    , &[PLACES]    , Action::Focus(Direction::Left)  ),
    bind(CTRL    , Char('j')    , &[PLACES]    , Action::Focus(Direction::Down)  ),
    bind(CTRL    , Char('k')    , &[PLACES]    , Action::Focus(Direction::Up)    ),
    bind(CTRL    , Char('l')    , &[PLACES]    , Action::Focus(Direction::Right) ),
        // 
    /* Tabs pane bindings */
    bind(NONE    , Char('h')    , &[TABS]    , Action::PrevTab                 ),
    bind(NONE    , Char('l')    , &[TABS]    , Action::NextTab                 ),
    bind(NONE    , Char('j')    , &[TABS]    , Action::NoOp                    ),
    bind(NONE    , Char('k')    , &[TABS]    , Action::NoOp                    ),
    bind(NONE    , Left         , &[TABS]    , Action::PrevTab                 ),
    bind(NONE    , Down         , &[TABS]    , Action::NoOp                    ),
    bind(NONE    , Up           , &[TABS]    , Action::NoOp                    ),
    bind(NONE    , Right        , &[TABS]    , Action::NextTab                 ),
    bind(CTRL    , Char('h')    , &[TABS]    , Action::Focus(Direction::Left)  ),
    bind(CTRL    , Char('j')    , &[TABS]    , Action::Focus(Direction::Down)  ),
    bind(CTRL    , Char('k')    , &[TABS]    , Action::Focus(Direction::Up)    ),
    bind(CTRL    , Char('l')    , &[TABS]    , Action::Focus(Direction::Right) ),

    /* Dolphin-like bindings */
    /* modifier   key              modes                             action                   */
    bind(ALT       , Left        , VIEW_MODES   , Action::Back            ),
    bind(ALT       , Right       , VIEW_MODES   , Action::Forward         ),
    bind(ALT       , Up          , VIEW_MODES   , Action::GoUp            ),
    bind(NONE      , Backspace   , VIEW_MODES   , Action::GoUp            ),
    bind(ALT       , Home        , VIEW_MODES   , Action::GoHome          ),
    bind(NONE      , F(5)        , VIEW_MODES   , Action::Refresh         ),
    bind(NONE      , Enter       , VIEW_MODES   , Action::Open            ),
    bind(NONE      , Down        , VIEW_MODES   , Action::MoveDown        ),
    bind(NONE      , Up          , VIEW_MODES   , Action::MoveUp          ),
    bind(NONE      , Left        , VIEW_MODES   , Action::MoveLeft        ),
    bind(NONE      , Right       , VIEW_MODES   , Action::MoveRight       ),
    bind(NONE      , Home        , VIEW_MODES   , Action::Top             ),
    bind(NONE      , End         , VIEW_MODES   , Action::Bottom          ),
    bind(NONE      , PageDown    , VIEW_MODES   , Action::PageDown        ),
    bind(NONE      , PageUp      , VIEW_MODES   , Action::PageUp          ),
    bind(CTRL      , Char('a')   , VIEW_MODES   , Action::SelectAll       ),
    bind(CTRL_SHIFT, Char('A')   , VIEW_MODES   , Action::InvertSelect    ),
    bind(CTRL      , Char('c')   , VIEW_MODES   , Action::Copy            ),
    bind(CTRL      , Char('x')   , VIEW_MODES   , Action::Cut             ),
    bind(CTRL      , Char('v')   , VIEW_MODES   , Action::EnterVisualBlock),
    bind(NONE      , Delete      , VIEW_MODES   , Action::Trash           ),
    bind(SHIFT     , Delete      , VIEW_MODES   , Action::DeletePerm      ),
    bind(NONE      , F(2)        , VIEW_MODES   , Action::Rename          ),
    bind(NONE      , F(10)       , VIEW_MODES   , Action::NewFolder       ),
    bind(CTRL_SHIFT, Char('N')   , VIEW_MODES   , Action::NewFolder       ),
    bind(ALT       , Enter       , VIEW_MODES   , Action::Properties      ),
    bind(CTRL      , Char('1')   , VIEW_MODES   , Action::ViewIcons       ),
    bind(CTRL      , Char('2')   , VIEW_MODES   , Action::ViewCompact     ),
    bind(CTRL      , Char('3')   , VIEW_MODES   , Action::ViewDetails     ),
    bind(NONE      , F(3)        , VIEW_MODES   , Action::ToggleSplit     ),
    bind(NONE      , Tab         , VIEW_MODES   , Action::SwapPane        ),
    bind(NONE      , F(9)        , VIEW_MODES   , Action::TogglePlaces    ),
    bind(NONE      , F(11)       , VIEW_MODES   , Action::ToggleInfo      ),
    bind(CTRL      , Char('i')   , VIEW_MODES   , Action::ToggleFilterBar ),
    bind(CTRL      , Char('t')   , VIEW_MODES   , Action::NewTab          ),
    bind(CTRL      , Char('w')   , VIEW_MODES   , Action::CloseTab        ),
    bind(CTRL      , Tab         , VIEW_MODES   , Action::NextTab         ),
    bind(CTRL      , BackTab     , VIEW_MODES   , Action::PrevTab         ),
    bind(NONE      , BackTab     , VIEW_MODES   , Action::PrevTab         ),
    bind(NONE      , F(6)        , VIEW_MODES   , Action::EnterPathEdit   ),
    bind(NONE      , F(4)        , VIEW_MODES   , Action::TerminalPanel   ),
    bind(SHIFT     , F(4)        , VIEW_MODES   , Action::TerminalHere    ),
    bind(NONE      , F(1)        , VIEW_MODES   , Action::Help            ),
    bind(CTRL      , Char('q')   , VIEW_MODES   , Action::QuitAll         ),

    /* Vim-like bindings */
    /* modifier   key           modes                                   action                   */
    bind(NONE      , Char('h')   , VIEW_MODES         , Action::MoveLeft       ),
    bind(NONE      , Char('j')   , VIEW_MODES         , Action::MoveDown       ),
    bind(NONE      , Char('k')   , VIEW_MODES         , Action::MoveUp         ),
    bind(NONE      , Char('l')   , VIEW_MODES         , Action::MoveRight      ),
    bind(SHIFT     , Char('G')   , VIEW_MODES         , Action::Bottom         ),
    bind(SHIFT     , Char('H')   , VIEW_MODES         , Action::NavigateUp     ),
    bind(SHIFT     , Char('L')   , VIEW_MODES         , Action::NavigateInto   ),
    bind(CTRL      , Char('d')   , VIEW_MODES         , Action::HalfPageDown   ),
    bind(CTRL      , Char('u')   , VIEW_MODES         , Action::HalfPageUp     ),
    bind(CTRL      , Char('f')   , VIEW_MODES         , Action::PageDown       ),
    bind(CTRL      , Char('b')   , VIEW_MODES         , Action::PageUp         ),
    bind(NONE      , Char('0')   , VIEW_MODES         , Action::RowStart       ),
    bind(NONE      , Char('$')   , VIEW_MODES         , Action::RowEnd         ),
    bind(NONE      , Char('v')   , VIEW_MODES         , Action::EnterVisual    ),
    bind(SHIFT     , Char('V')   , VIEW_MODES         , Action::EnterVisualLine),
    bind(NONE      , Char('y')   , VISUAL_MODES                 , Action::Yank           ),
    bind(NONE      , Char('d')   , &[NORMAL]                              , Action::DeleteOp       ),
    bind(NONE      , Char('d')   , VISUAL_MODES                 , Action::DeleteSelection),
    bind(NONE      , Char('p')   , VIEW_MODES         , Action::Paste          ),
    bind(NONE      , Char('x')   , VIEW_MODES         , Action::Trash          ),
    bind(NONE      , Char('o')   , VIEW_MODES         , Action::NewFile        ),
    bind(SHIFT     , Char('O')   , VIEW_MODES         , Action::NewFolder      ),
    bind(NONE      , Char('u')   , VIEW_MODES         , Action::Undo           ),
    bind(NONE      , Char('/')   , VIEW_MODES         , Action::EnterSearch    ),
    bind(NONE      , Char('n')   , VIEW_MODES         , Action::SearchNext     ),
    bind(SHIFT     , Char('N')   , VIEW_MODES         , Action::SearchPrev     ),
    bind(NONE      , Char(':')   , VIEW_MODES         , Action::EnterCommand   ),
    bind(CTRL      , Enter       , VIEW_MODES         , Action::OpenInNewTab   ),
    bind(NONE      , Char('m')   , VIEW_MODES         , Action::SetMark        ),
    bind(NONE      , Char('\'')  , VIEW_MODES         , Action::JumpMark       ),
    bind(CTRL      , Char('h')   , VIEW_MODES         , Action::Focus(Direction::Left) ),
    bind(CTRL      , Char('j')   , VIEW_MODES         , Action::Focus(Direction::Down) ),
    bind(CTRL      , Char('l')   , VIEW_MODES         , Action::Focus(Direction::Right)),
    bind(CTRL      , Char('k')   , VIEW_MODES         , Action::Focus(Direction::Up)   ),

    /* Text-entry bindings */
    bind(NONE      , Esc         , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::Cancel         ),
    bind(NONE      , Enter       , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::CommitInput    ),
    bind(NONE      , Backspace   , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputBackspace ),
    bind(NONE      , Delete      , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputDelete    ),
    bind(NONE      , Left        , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputLeft      ),
    bind(NONE      , Right       , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputRight     ),
    bind(NONE      , Home        , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputHome      ),
    bind(NONE      , End         , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputEnd       ),
    bind(CTRL      , Char('u')   , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputClear     ),
    bind(CTRL      , Char('w')   , &[COMMAND, SEARCH, FILTER, PATH_EDIT, RENAME, BATCH_RENAME, NEW_FOLDER, NEW_FILE]   , Action::InputDeleteWord),
    bind(NONE      , Tab         , &[PATH_EDIT]                                                                        , Action::CompletePath   ),

    /* Confirmation and overlay bindings */
    bind(NONE      , Char('y')   , &[CONFIRM]                      , Action::ConfirmAccept),
    bind(NONE      , Char('Y')   , &[CONFIRM]                      , Action::ConfirmAccept),
    bind(NONE      , Enter       , &[CONFIRM]                      , Action::ConfirmAccept),
    bind(NONE      , Esc         , &[CONFIRM, PROPERTIES, HELP]    , Action::Cancel       ),
    bind(NONE      , Char('q')   , &[PROPERTIES, HELP]             , Action::Cancel       ),

    /* Menu, breadcrumb, and toolbar bindings */
    bind(NONE      , Esc         , &[MENU, CRUMB_MENU, BUTTONS]    , Action::Cancel                 ),
    bind(NONE      , Char('q')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::Cancel                 ),
    bind(NONE      , Down        , &[MENU, CRUMB_MENU]             , Action::InterfaceDown          ),
    bind(NONE      , Char('j')   , &[MENU, CRUMB_MENU]             , Action::InterfaceDown          ),
    bind(CTRL      , Char('n')   , &[MENU, CRUMB_MENU]             , Action::InterfaceDown          ),
    bind(NONE      , Up          , &[MENU, CRUMB_MENU]             , Action::InterfaceUp            ),
    bind(NONE      , Char('k')   , &[MENU, CRUMB_MENU]             , Action::InterfaceUp            ),
    bind(CTRL      , Char('p')   , &[MENU, CRUMB_MENU]             , Action::InterfaceUp            ),
    bind(NONE      , Home        , &[MENU]                         , Action::InterfaceFirst         ),
    bind(NONE      , Char('g')   , &[MENU]                         , Action::InterfaceFirst         ),
    bind(NONE      , End         , &[MENU]                         , Action::InterfaceLast          ),
    bind(NONE      , Char('G')   , &[MENU]                         , Action::InterfaceLast          ),
    bind(NONE      , Left        , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceLeft          ),
    bind(NONE      , Char('h')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceLeft          ),
    bind(NONE      , Right       , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceRight         ),
    bind(NONE      , Char('l')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceRight         ),
    bind(NONE      , Enter       , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceAccept        ),
    bind(NONE      , Tab         , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceAccept        ),
    bind(CTRL      , Char('y')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::InterfaceAccept        ),
    bind(CTRL      , Char('j')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::Focus(Direction::Down) ),
    bind(CTRL      , Char('k')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::Focus(Direction::Up)   ),
    bind(CTRL      , Char('h')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::Focus(Direction::Left) ),
    bind(CTRL      , Char('l')   , &[MENU, CRUMB_MENU, BUTTONS]    , Action::Focus(Direction::Right)),
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
    chord('g'  , 'g'     , VIEW_MODES  , Action::Top         ),
    chord('g'  , 'h'     , VIEW_MODES  , Action::GoHome      ),
    chord('g'  , 't'     , VIEW_MODES  , Action::NextTab     ),
    chord('g'  , 'T'     , VIEW_MODES  , Action::PrevTab     ),
    chord('g'  , 'u'     , VIEW_MODES  , Action::GoUp        ),
    chord('z'  , 'c'     , VIEW_MODES  , Action::CloseFold          ),
    chord('z'  , 'o'     , VIEW_MODES  , Action::OpenFold           ),
    chord('z'  , 'a'     , VIEW_MODES  , Action::ToggleExpand       ),
    chord('z'  , 'C'     , VIEW_MODES  , Action::CloseFoldRecursive ),
    chord('z'  , 'O'     , VIEW_MODES  , Action::OpenFoldRecursive  ),
    chord('z'  , 'M'     , VIEW_MODES  , Action::CloseAllFolds      ),
    chord('z'  , 'R'     , VIEW_MODES  , Action::OpenAllFolds       ),
    chord('z'  , 'v'     , VIEW_MODES  , Action::CycleView          ),
    chord('c'  , 'w'     , VIEW_MODES  , Action::Rename      ),
    /* Space leader; selection moved to `v` */
    chord(' '  , 'h'     , VIEW_MODES  , Action::ToggleHidden),
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

    #[test]
    fn fold_chords_match_vim_semantics() {
        let expected = [
            ('c', Action::CloseFold),
            ('o', Action::OpenFold),
            ('a', Action::ToggleExpand),
            ('C', Action::CloseFoldRecursive),
            ('O', Action::OpenFoldRecursive),
            ('M', Action::CloseAllFolds),
            ('R', Action::OpenAllFolds),
        ];
        for (follower, action) in expected {
            assert!(CHORDS.iter().any(|chord| {
                chord.leader == 'z' && chord.follower == follower && chord.action == action
            }));
        }
    }

}
