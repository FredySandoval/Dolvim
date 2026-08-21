//! Directory listing, sorting, metadata formatting, and the listing worker.

use std::borrow::Cow;
use std::cmp::{Ordering, Reverse};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Dir,
    File,
    Symlink,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    /// Accessible bytes when a row's logical path is not its filesystem path.
    /// Currently used by Trash browsing; live entries leave this unset.
    pub backing_path: Option<PathBuf>,
    /// Target exactly as stored in the directory entry. Kept relative because
    /// resolving it would hide both useful link text and broken links.
    pub link_target: Option<PathBuf>,
    pub kind: Kind,
    /// Byte size for files; for directories Dolphin shows a child count, which
    /// we store here too and disambiguate with `kind`.
    pub size: u64,
    pub mtime: i64,
    pub mode: u32,
    /// False for a directory we cannot open — root-owned `0700` and the like.
    /// Free to compute: the child count already has to open it.
    pub readable: bool,
    pub hidden: bool,
    /// Controlled Trash generation identity. Keeping the backend ID and its
    /// allocation-free selection key in one value prevents divergence.
    pub(crate) trash_identity: Option<TrashIdentity>,
    /// Depth in an expanded Details tree; 0 for a plain listing.
    pub depth: u16,
    pub expanded: bool,
}

pub fn trash_selection_key(id: &std::ffi::OsStr) -> PathBuf {
    let mut key = std::ffi::OsString::from("trash-generation:");
    key.push(id);
    PathBuf::from(key)
}

#[derive(Clone, Debug)]
pub struct TrashIdentity {
    id: std::ffi::OsString,
    selection_key: PathBuf,
}

impl TrashIdentity {
    pub fn new(id: std::ffi::OsString) -> Self {
        let selection_key = trash_selection_key(&id);
        Self { id, selection_key }
    }
}

impl Entry {
    /// Filesystem location containing this row's bytes. Trash rows retain
    /// their original path for display and restore semantics.
    pub fn filesystem_path(&self) -> &Path {
        self.backing_path.as_deref().unwrap_or(&self.path)
    }

    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// Stable selection identity. Live rows borrow their path; Trash generations
    /// borrow the key computed when the listing was built.
    pub fn selection_key(&self) -> &Path {
        self.trash_identity
            .as_ref()
            .map(|identity| identity.selection_key.as_path())
            .unwrap_or(&self.path)
    }

    pub fn trash_id(&self) -> Option<&std::ffi::OsStr> {
        self.trash_identity
            .as_ref()
            .map(|identity| identity.id.as_os_str())
    }

    #[cfg(test)]
    pub fn set_trash_id(&mut self, id: impl Into<std::ffi::OsString>) {
        self.trash_identity = Some(TrashIdentity::new(id.into()));
    }

    /// A directory the current user cannot look inside.
    pub fn is_locked(&self) -> bool {
        self.kind == Kind::Dir && !self.readable
    }

    pub fn is_executable(&self) -> bool {
        self.kind == Kind::File && self.mode & 0o111 != 0
    }

    /// Borrow UTF-8 extensions and use a lossy value only for unusual Unix
    /// names. This keeps the rendering fast path allocation-free without
    /// classifying every non-UTF-8 extension as absent.
    pub fn ext(&self) -> Option<Cow<'_, str>> {
        Some(self.path.extension()?.to_string_lossy())
    }

    pub fn is_image(&self) -> bool {
        self.ext()
            .is_some_and(|extension| extension_in(&extension, config::IMAGE_EXTS))
    }

    /// The "Type" column, Dolphin-style plain-English descriptions. Most
    /// descriptions are borrowed; only names containing an extension allocate.
    pub fn type_name(&self) -> Cow<'static, str> {
        match self.kind {
            Kind::Dir => "Folder".into(),
            Kind::Symlink => "Link".into(),
            Kind::File => match self.ext() {
                None => {
                    if self.is_executable() {
                        "Executable".into()
                    } else {
                        "Unknown".into()
                    }
                }
                Some(extension) if extension_in(&extension, config::IMAGE_EXTS) => {
                    format!("{} image", extension.to_uppercase()).into()
                }
                Some(extension) if extension_in(&extension, config::ARCHIVE_EXTS) => {
                    "Archive".into()
                }
                Some(extension) if extension_in(&extension, &["sh", "bash", "zsh", "fish"]) => {
                    "Shell script".into()
                }
                Some(extension) if extension_eq(&extension, "rs") => "Rust source".into(),
                Some(extension) if extension_in(&extension, &["txt", "md"]) => {
                    "Text document".into()
                }
                Some(extension) if extension_eq(&extension, "pdf") => "PDF document".into(),
                Some(extension)
                    if extension_in(&extension, &["mp3", "flac", "ogg", "wav", "m4a"]) =>
                {
                    "Audio".into()
                }
                Some(extension)
                    if extension_in(&extension, &["mp4", "mkv", "webm", "avi", "mov"]) =>
                {
                    "Video".into()
                }
                Some(extension) => format!("{} file", extension.to_uppercase()).into(),
            },
        }
    }

    /// `home()` is only touched once a name matches, so the common case is a
    /// string compare against a six-row table.
    fn home_folder_glyph(&self) -> Option<&'static str> {
        let home_glyph = config::XDG_DIRS
            .iter()
            .find(|xdg_dir| xdg_dir.name == self.name)?
            .glyph;
        (self.path.parent()? == crate::places::home()).then_some(home_glyph)
    }

    pub fn glyph(&self) -> &'static str {
        use config::glyph;
        match self.kind {
            Kind::Symlink => glyph::SYMLINK,
            Kind::Dir => {
                if self.is_locked() {
                    glyph::FOLDER_LOCKED
                } else if self.expanded {
                    glyph::FOLDER_OPEN
                } else if let Some(home_glyph) = self.home_folder_glyph() {
                    home_glyph
                } else if self.size == 0 {
                    glyph::FOLDER_EMPTY
                } else {
                    glyph::FOLDER
                }
            }
            Kind::File => {
                let extension = self.ext();
                if let Some(icon) = extension.as_deref().and_then(|extension| {
                    config::FILE_ICONS.iter().find_map(|&(candidate, icon)| {
                        extension_eq(candidate, extension).then_some(icon)
                    })
                }) {
                    icon
                } else {
                    match extension.as_deref() {
                        Some(extension) if extension_in(extension, config::IMAGE_EXTS) => {
                            glyph::PICTURE
                        }
                        Some(extension) if extension_in(extension, config::ARCHIVE_EXTS) => {
                            glyph::ARCHIVE
                        }
                        Some(extension)
                            if extension_in(extension, &["mp3", "flac", "ogg", "wav", "m4a"]) =>
                        {
                            glyph::MUSIC
                        }
                        Some(extension)
                            if extension_in(extension, &["mp4", "mkv", "webm", "avi", "mov"]) =>
                        {
                            glyph::VIDEO
                        }
                        Some(extension)
                            if extension_in(
                                extension,
                                &["txt", "md", "pdf", "doc", "docx", "odt"],
                            ) =>
                        {
                            glyph::DOCUMENT
                        }
                        _ => glyph::FILE,
                    }
                }
            }
        }
    }
}

fn extension_in(extension: &str, candidates: &[&str]) -> bool {
    candidates
        .iter()
        .any(|candidate| extension_eq(extension, candidate))
}

fn extension_eq(left: &str, right: &str) -> bool {
    if left.is_ascii() && right.is_ascii() {
        left.eq_ignore_ascii_case(right)
    } else {
        left.chars()
            .flat_map(char::to_lowercase)
            .eq(right.chars().flat_map(char::to_lowercase))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortKey {
    Name,
    Size,
    Date,
    Type,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Name => "Name",
            SortKey::Size => "Size",
            SortKey::Date => "Modified",
            SortKey::Type => "Type",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Sort {
    pub key: SortKey,
    pub reverse: bool,
    pub dirs_first: bool,
}

impl Default for Sort {
    fn default() -> Self {
        Sort {
            key: SortKey::Name,
            reverse: false,
            dirs_first: true,
        }
    }
}

/// Dolphin's natural sort: digit runs compare numerically, so `file10` follows
/// `file9`. Case-insensitive, like every other file manager worth using.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    if a.is_ascii() && b.is_ascii() {
        return natural_cmp_folded(a, b).then_with(|| a.cmp(b));
    }
    let left_lower: String = a.chars().flat_map(char::to_lowercase).collect();
    let right_lower: String = b.chars().flat_map(char::to_lowercase).collect();
    natural_cmp_folded(&left_lower, &right_lower).then_with(|| a.cmp(b))
}

fn natural_cmp_folded(a: &str, b: &str) -> Ordering {
    let (mut left_chars, mut right_chars) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (left_chars.peek().copied(), right_chars.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(left_char), Some(right_char)) => {
                if left_char.is_ascii_digit() && right_char.is_ascii_digit() {
                    let left_number = take_number(&mut left_chars);
                    let right_number = take_number(&mut right_chars);
                    match left_number.cmp(&right_number) {
                        Ordering::Equal => {}
                        order => return order,
                    }
                } else {
                    match left_char
                        .to_ascii_lowercase()
                        .cmp(&right_char.to_ascii_lowercase())
                    {
                        Ordering::Equal => {
                            left_chars.next();
                            right_chars.next();
                        }
                        order => return order,
                    }
                }
            }
        }
    }
}

fn take_number(digit_chars: &mut std::iter::Peekable<std::str::Chars>) -> u128 {
    let mut number: u128 = 0;
    while let Some(digit) = digit_chars.peek().copied() {
        if !digit.is_ascii_digit() {
            break;
        }
        // Saturate rather than wrap: a 40-digit filename is not a number.
        number = number
            .saturating_mul(10)
            .saturating_add(digit as u128 - '0' as u128);
        digit_chars.next();
    }
    number
}

pub fn sort_entries(entries: &mut [Entry], sort: Sort) {
    if sort.key == SortKey::Type {
        // Establish the secondary order first. The stable cached-key sort then
        // formats each type once, rather than allocating in every comparison.
        entries.sort_by(|a, b| reverse_if(natural_cmp(&a.name, &b.name), sort.reverse));
        if sort.reverse {
            entries.sort_by_cached_key(|entry| {
                (
                    sort.dirs_first && !entry.is_dir(),
                    Reverse(entry.type_name()),
                )
            });
        } else {
            entries.sort_by_cached_key(|entry| {
                (sort.dirs_first && !entry.is_dir(), entry.type_name())
            });
        }
        return;
    }

    entries.sort_by(|a, b| {
        if sort.dirs_first {
            match (a.is_dir(), b.is_dir()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }
        let order = match sort.key {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Size => a
                .size
                .cmp(&b.size)
                .then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Date => a
                .mtime
                .cmp(&b.mtime)
                .then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Type => unreachable!("type sorting is handled above"),
        };
        reverse_if(order, sort.reverse)
    });
}

fn reverse_if(order: Ordering, reverse: bool) -> Ordering {
    if reverse {
        order.reverse()
    } else {
        order
    }
}

/// Entries successfully read from a directory, plus the first per-entry
/// failure. The outer `Result` is reserved for failure to open the directory.
pub struct DirectoryListing {
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

pub fn read_dir(path: &Path, depth: u16) -> std::io::Result<DirectoryListing> {
    read_dir_as(path, path, depth, false)
}

/// Read physical directory contents while assigning paths below a logical root.
/// This keeps ordinary path semantics out of virtual filesystem traversal.
pub fn read_dir_as(
    physical: &Path,
    logical: &Path,
    depth: u16,
    backed: bool,
) -> std::io::Result<DirectoryListing> {
    let read_dir = fs::read_dir(physical)?;
    Ok(collect_entries(read_dir, logical, depth, backed))
}

fn collect_entries(
    read_dir: impl Iterator<Item = std::io::Result<fs::DirEntry>>,
    logical: &Path,
    depth: u16,
    backed: bool,
) -> DirectoryListing {
    let mut entries = Vec::new();
    let mut error = None;
    for result in read_dir {
        let result = result.and_then(|entry| entry_from_dir_entry(entry, logical, depth, backed));
        match result {
            Ok(built) => {
                entries.push(built.entry);
                if let Some(warning) = built.warning {
                    error.get_or_insert(warning);
                }
            }
            Err(entry_error) => {
                error.get_or_insert_with(|| entry_error.to_string());
            }
        }
    }
    DirectoryListing { entries, error }
}

struct BuiltEntry {
    entry: Entry,
    warning: Option<String>,
}

fn entry_from_dir_entry(
    dir_entry: fs::DirEntry,
    logical: &Path,
    depth: u16,
    backed: bool,
) -> std::io::Result<BuiltEntry> {
    let name = dir_entry.file_name().to_string_lossy().into_owned();
    let physical_path = dir_entry.path();
    let entry_path = logical.join(dir_entry.file_name());
    let link_metadata = fs::symlink_metadata(&physical_path)?;
    let is_link = link_metadata.file_type().is_symlink();
    let link_target = if is_link {
        Some(fs::read_link(&physical_path)?)
    } else {
        None
    };
    // Dolphin follows links for the type shown, but keeps broken-link rows.
    let (metadata, mut warning) = if is_link {
        match fs::metadata(&physical_path) {
            Ok(metadata) => (metadata, None),
            Err(error) => (
                link_metadata,
                Some(format!(
                    "Cannot follow {}: {error}",
                    physical_path.display()
                )),
            ),
        }
    } else {
        (link_metadata, None)
    };
    let kind = if metadata.is_dir() {
        Kind::Dir
    } else if is_link {
        Kind::Symlink
    } else {
        Kind::File
    };
    // One `read_dir` answers both "how many children" and "can we get in",
    // so the lock state costs no extra syscall.
    let child_count = (kind == Kind::Dir).then(|| dir_child_count(&physical_path));
    if let Some(Err(error)) = &child_count {
        warning.get_or_insert_with(|| format!("Cannot count {}: {error}", physical_path.display()));
    }
    Ok(BuiltEntry {
        entry: Entry {
            hidden: name.starts_with('.'),
            name,
            path: entry_path,
            backing_path: backed.then_some(physical_path),
            link_target,
            kind,
            size: match &child_count {
                Some(Ok(count)) => *count,
                Some(Err(_)) => 0,
                None => metadata.len(),
            },
            mtime: metadata.mtime(),
            mode: metadata.permissions().mode(),
            readable: child_count.map(|count| count.is_ok()).unwrap_or(true),
            trash_identity: None,
            depth,
            expanded: false,
        },
        warning,
    })
}

fn dir_child_count(dir: &Path) -> std::io::Result<u64> {
    let mut count = 0;
    for entry in fs::read_dir(dir)? {
        entry?;
        count += 1;
    }
    Ok(count)
}

/// Base 1000, KDE's "Metric" file size setting. The binary units it also offers
/// disagree with `eza`, which is what the terminal beside this one is showing.
/// The unit is two wide either way, so a bare `B` carries a leading space. The
/// Details column right-aligns the whole string; without the pad, a `B` row
/// pushes its digits one place right of the `KB` rows it sits under.
pub fn format_size(bytes: u64) -> String {
    if bytes < 1000 {
        return format!("{bytes}  B");
    }
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    let mut scaled = bytes as f64 / 1000.0;
    let mut unit_index = 0;
    while scaled >= 1000.0 && unit_index + 1 < UNITS.len() {
        scaled /= 1000.0;
        unit_index += 1;
    }
    format!("{:.1} {}", scaled, UNITS[unit_index])
}

/// Dolphin renders folder sizes as a child count, not bytes. `item` carries a
/// trailing pad for the same reason a bare `B` carries a leading one: the
/// column right-aligns the whole string, and an unpadded singular would push
/// its digit one place right of the `items` rows above it.
pub fn format_entry_size(entry: &Entry) -> String {
    if entry.is_dir() {
        match entry.size {
            0 => "0 items".into(),
            1 => "1  item".into(),
            count => format!("{count} items"),
        }
    } else {
        format_size(entry.size)
    }
}

/// How the Details `Modified` column and the information panel spell a time.
/// Month names are English on purpose: reading them out of `LC_TIME` means
/// calling the C library, and this crate forbids `unsafe`.
// Whichever variant `config::TIME_STYLE` does not name is unconstructed by
// definition. That is what a compiled-in setting looks like, not dead code.
#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq)]
pub enum TimeStyle {
    /// `2026-08-02 20:22`. Sorts as it reads and never needs a year rule.
    Iso,
    /// `Aug 2, 8:22pm`, and `Aug 2 2025, 8:22pm` outside the current year.
    /// Dolphin's default.
    Short,
}

/// Seconds east of UTC. Asked of `date` once, because `std` cannot read the
/// zone and parsing `/etc/localtime` is more code than this column is worth.
/// A file manager that prints UTC mtimes is simply wrong about what it shows.
fn utc_offset() -> i64 {
    static UTC_OFFSET_SECS: OnceLock<i64> = OnceLock::new();
    *UTC_OFFSET_SECS.get_or_init(|| {
        let date_output = Command::new("date").arg("+%z").output().ok();
        let offset_text = date_output.map(|process_output| {
            String::from_utf8_lossy(&process_output.stdout)
                .trim()
                .to_string()
        });
        // `+HHMM`, or nothing usable — in which case UTC is the honest fallback.
        let Some(offset_text) = offset_text.filter(|text| text.len() == 5) else {
            return 0;
        };
        let (offset_hours, offset_minutes) = (
            offset_text[1..3].parse::<i64>(),
            offset_text[3..5].parse::<i64>(),
        );
        let (Ok(offset_hours), Ok(offset_minutes)) = (offset_hours, offset_minutes) else {
            return 0;
        };
        let secs = offset_hours * 3600 + offset_minutes * 60;
        if offset_text.starts_with('-') {
            -secs
        } else {
            secs
        }
    })
}

/// Broken-down local civil time.
pub struct CivilTime {
    pub year: i64,
    /// 1-12.
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
}

fn civil(epoch: i64) -> CivilTime {
    let local = epoch + utc_offset();
    let (mut days, secs) = (local.div_euclid(86400), local.rem_euclid(86400));
    let (h, m) = (secs / 3600, (secs % 3600) / 60);
    // Civil-from-days, Howard Hinnant's algorithm.
    days += 719468;
    let era = days.div_euclid(146097);
    let doe = days.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    CivilTime {
        year: y,
        month: mo,
        day: d,
        hour: h,
        minute: m,
    }
}

/// The local calendar year `epoch` falls in.
pub fn year_of(epoch: i64) -> i64 {
    civil(epoch).year
}

/// A 24-hour hour as a 12-hour one and its marker. Midnight and noon are the
/// cases worth naming: both read 12, and `h % 12` gives 0 for each.
fn hour12(h: i64) -> (i64, &'static str) {
    let suffix = if h < 12 { "am" } else { "pm" };
    match h % 12 {
        0 => (12, suffix),
        n => (n, suffix),
    }
}

/// A timestamp in whichever style `config::TIME_STYLE` selects.
pub fn format_time(epoch: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let CivilTime {
        year,
        month,
        day,
        hour,
        minute,
    } = civil(epoch);
    match config::TIME_STYLE {
        TimeStyle::Iso => format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}"),
        TimeStyle::Short => {
            let (clock_hour, suffix) = hour12(hour);
            let year_part = match civil(now_epoch()).year {
                this_year if this_year == year => String::new(),
                _ => format!(" {year}"),
            };
            // The hour is padded to two so everything after the comma is one
            // fixed width. Right-aligning the whole string then lands the comma
            // in the same cell whether the day is one digit or two.
            format!(
                "{} {day}{year_part}, {clock_hour:>2}:{minute:02}{suffix}",
                MONTHS[month as usize - 1]
            )
        }
    }
}

pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A partition worth showing under Places, as `lsblk` reports it.
///
/// `/proc/mounts` cannot answer this: Dolphin lists disks that are *not*
/// mounted, and only `lsblk` knows a partition's type, label and whether its
/// bus is hotpluggable. It ships with util-linux, same as `mount` itself.
pub struct Device {
    /// Block-device node passed to udisksctl for mounting.
    pub path: PathBuf,
    /// Filesystem label; empty when the partition has none.
    pub label: String,
    pub size: u64,
    pub mount: Option<PathBuf>,
    pub removable: bool,
}

/// The `lsblk -o` columns below, by position. Keep the two in step: adding a
/// column to the `-o` string shifts every index after it.
const COL_SIZE: usize = 0;
const COL_FSTYPE: usize = 1;
const COL_LABEL: usize = 2;
const COL_MOUNTPOINT: usize = 3;
const COL_HOTPLUG: usize = 4;
const COL_PARTTYPENAME: usize = 5;
const COL_PATH: usize = 6;
const LSBLK_COLUMNS: &str = "SIZE,FSTYPE,LABEL,MOUNTPOINT,HOTPLUG,PARTTYPENAME,PATH";

pub fn devices() -> Vec<Device> {
    let Ok(out) = std::process::Command::new("lsblk")
        .args(["-rnb", "-o", LSBLK_COLUMNS])
        .output()
    else {
        return Vec::new();
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(parse_device)
        .collect()
}

fn parse_device(line: &str) -> Option<Device> {
    // Raw mode separates with single spaces and escapes real ones as \x20, so
    // empty columns must survive the split. `split_whitespace` eats them.
    let fields: Vec<&str> = line.split(' ').collect();
    if fields.len() < 7 {
        return None;
    }
    // No filesystem, swap, or the EFI system partition: Dolphin shows none of
    // these, and none of them is a place a user navigates to.
    if fields[COL_FSTYPE].is_empty()
        || fields[COL_FSTYPE] == "swap"
        || unescape_lsblk(fields[COL_PARTTYPENAME]) == "EFI System"
    {
        return None;
    }
    Some(Device {
        path: PathBuf::from(unescape_lsblk(fields[COL_PATH])),
        label: unescape_lsblk(fields[COL_LABEL]),
        size: fields[COL_SIZE].parse().unwrap_or(0),
        mount: (!fields[COL_MOUNTPOINT].is_empty())
            .then(|| PathBuf::from(unescape_lsblk(fields[COL_MOUNTPOINT]))),
        removable: fields[COL_HOTPLUG] == "1",
    })
}

/// `lsblk -r` escapes the bytes that would otherwise break the column split.
fn unescape_lsblk(s: &str) -> String {
    s.replace("\\x20", " ")
}

/// Ask the desktop storage service to mount a filesystem and return its path.
pub fn mount_device(path: &Path) -> Result<PathBuf, String> {
    let output = Command::new("udisksctl")
        .args(["mount", "--block-device"])
        .arg(path)
        .output()
        .map_err(|error| format!("Could not run udisksctl: {error}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    devices()
        .into_iter()
        .find(|device| device.path == path)
        .and_then(|device| device.mount)
        .ok_or_else(|| "Device mounted without a mount point".into())
}

/// Ask the desktop storage service to unmount a filesystem.
pub fn unmount_device(path: &Path) -> Result<(), String> {
    let output = Command::new("udisksctl")
        .args(["unmount", "--block-device"])
        .arg(path)
        .output()
        .map_err(|error| format!("Could not run udisksctl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// Free/total bytes for the filesystem holding `path`.
///
/// `df` rather than `statfs(2)`: the syscall needs `unsafe` and hardcoded
/// offsets into a libc struct we do not otherwise depend on, to answer a
/// question a coreutils binary already answers exactly. See docs/DECISIONS.md.
#[derive(Clone, Copy)]
pub struct DiskSpace {
    pub available_bytes: u64,
    pub total_bytes: u64,
}

pub fn disk_space(path: &Path) -> Option<DiskSpace> {
    let df_output = std::process::Command::new("df")
        .args(["-B1", "--output=avail,size"])
        .arg(path)
        .output()
        .ok()?;
    let df_output_text = String::from_utf8_lossy(&df_output.stdout);
    let line = df_output_text.lines().nth(1)?;
    let mut fields = line.split_whitespace();
    let available_bytes = fields.next()?.parse().ok()?;
    let total_bytes = fields.next()?.parse().ok()?;
    Some(DiskSpace {
        available_bytes,
        total_bytes,
    })
}

// ---------------------------------------------------------------------------
// Listing worker
// ---------------------------------------------------------------------------

/// A listing request carries a `seq` sequence number so stale results from a
/// directory the user has already navigated away from are dropped, not shown.
pub struct Listing {
    pub path: PathBuf,
    pub seq: u64,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

pub enum ListingMsg {
    Listed(Listing),
    /// Partial batch for huge directories, so 100k entries do not block.
    Batch {
        path: PathBuf,
        seq: u64,
        entries: Vec<Entry>,
    },
    Done {
        path: PathBuf,
        seq: u64,
        error: Option<String>,
    },
}

/// One directory listing asked of the worker thread.
pub struct ListingRequest {
    pub path: PathBuf,
    pub seq: u64,
}

/// Spawns the listing thread and returns a handle you push requests into.
pub struct Lister {
    jobs: Sender<ListingRequest>,
}

impl Lister {
    pub fn new(tx: Sender<ListingMsg>) -> Lister {
        let (jobs, rx) = channel::<ListingRequest>();
        thread::spawn(move || {
            // Held across iterations: a request that arrives mid-listing has to
            // outlive the job it interrupts. The single-slot mutex this
            // replaced could drop such a request outright, leaving the pane
            // that asked for it `loading` with nothing ever to arrive.
            let mut pending: Option<ListingRequest> = None;
            loop {
                // Blocks between requests instead of waking a hundred times a
                // second to look at an empty slot.
                let mut request = match pending.take() {
                    Some(pending_request) => pending_request,
                    None => match rx.recv() {
                        Ok(received_request) => received_request,
                        // The App is gone; so is any reason to keep listing.
                        Err(_) => return,
                    },
                };
                // Whatever piled up behind this one is already newer than it.
                while let Ok(newer_request) = rx.try_recv() {
                    request = newer_request;
                }
                let ListingRequest { path, seq } = request;
                let read_dir = match fs::read_dir(&path) {
                    Ok(read_dir) => read_dir,
                    Err(read_error) => {
                        let _ = tx.send(ListingMsg::Listed(Listing {
                            path,
                            seq,
                            entries: Vec::new(),
                            error: Some(read_error.to_string()),
                        }));
                        continue;
                    }
                };
                let mut batch = Vec::with_capacity(2000);
                let mut first_error = None;
                for dir_entry in read_dir {
                    while let Ok(newer_request) = rx.try_recv() {
                        pending = Some(newer_request);
                    }
                    if pending.is_some() {
                        break;
                    }
                    match dir_entry.and_then(|entry| entry_from_dir_entry(entry, &path, 0, false)) {
                        Ok(built) => {
                            batch.push(built.entry);
                            if let Some(warning) = built.warning {
                                first_error.get_or_insert(warning);
                            }
                        }
                        Err(error) => {
                            first_error
                                .get_or_insert_with(|| format!("Listing incomplete: {error}"));
                            continue;
                        }
                    }
                    if batch.len() == 2000 {
                        let _ = tx.send(ListingMsg::Batch {
                            path: path.clone(),
                            seq,
                            entries: std::mem::take(&mut batch),
                        });
                        batch.reserve(2000);
                    }
                }
                if pending.is_some() {
                    continue;
                }
                let _ = tx.send(ListingMsg::Batch {
                    path: path.clone(),
                    seq,
                    entries: batch,
                });
                let _ = tx.send(ListingMsg::Done {
                    path,
                    seq,
                    error: first_error,
                });
            }
        });
        Lister { jobs }
    }

    pub fn request(&self, path: PathBuf, seq: u64) {
        let _ = self.jobs.send(ListingRequest { path, seq });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_orders_digit_runs_numerically() {
        let mut names = vec!["file10", "file9", "File2", "a"];
        names.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(names, vec!["a", "File2", "file9", "file10"]);
    }

    #[test]
    fn giant_digit_runs_do_not_panic() {
        let long = "9".repeat(60);
        assert_eq!(natural_cmp(&long, &long), Ordering::Equal);
    }

    #[test]
    fn unicode_folding_compares_complete_lowercase_expansions() {
        assert_eq!(natural_cmp("İ2", "i\u{307}10"), Ordering::Less);
        assert!(extension_eq("K", "k"));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_extensions_are_classified_as_lossy_extensions() {
        use std::os::unix::ffi::OsStringExt;

        let entry = Entry {
            name: "invalid extension".into(),
            path: PathBuf::from(std::ffi::OsString::from_vec(b"file.\xffx".to_vec())),
            backing_path: None,
            link_target: None,
            kind: Kind::File,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            trash_identity: None,
            depth: 0,
            expanded: false,
        };
        assert_eq!(entry.ext().as_deref(), Some("�x"));
        assert_eq!(entry.type_name(), "�X file");
    }

    #[test]
    fn sizes_match_dolphin_formatting() {
        assert_eq!(format_size(0), "0  B");
        assert_eq!(format_size(701), "701  B");
        assert_eq!(format_size(1000), "1.0 KB");
        assert_eq!(format_size(499_289_948_160), "499.3 GB");
    }

    /// The zone is the machine's, so assert the parts the epoch fixes rather
    /// than a rendered string that moves with the tester's `date +%z`.
    #[test]
    fn epoch_converts_to_civil_time() {
        let utc = |epoch| {
            let c = civil(epoch - utc_offset());
            (c.year, c.month, c.day, c.hour, c.minute)
        };
        assert_eq!(utc(0), (1970, 1, 1, 0, 0));
        assert_eq!(utc(1_753_776_000), (2025, 7, 29, 8, 0));
        // A leap day is where a hand-rolled calendar goes wrong.
        assert_eq!(utc(1_709_164_800), (2024, 2, 29, 0, 0));
    }

    #[test]
    fn the_twelve_hour_clock_has_no_zero_oclock() {
        let want = [
            (0, (12, "am")),
            (1, (1, "am")),
            (11, (11, "am")),
            (12, (12, "pm")),
            (13, (1, "pm")),
            (23, (11, "pm")),
        ];
        for (h, expect) in want {
            assert_eq!(hour12(h), expect, "hour {h}");
        }
    }

    #[test]
    fn directory_entries_retain_relative_symlink_targets() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dolvim-symlink-target-{unique}"));
        fs::create_dir(&dir).unwrap();
        let target = PathBuf::from("../../.agents/skills/herdr");
        std::os::unix::fs::symlink(&target, dir.join("herdr")).unwrap();

        let listing = read_dir(&dir, 0).unwrap();

        assert!(listing
            .error
            .as_deref()
            .is_some_and(|error| error.contains("Cannot follow")));
        assert_eq!(listing.entries[0].link_target.as_ref(), Some(&target));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn per_entry_errors_leave_a_partial_listing_and_report_the_first_failure() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dolvim-partial-listing-{unique}"));
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("visible.txt"), b"visible").unwrap();
        let entry = fs::read_dir(&dir).unwrap().next().unwrap().unwrap();
        let injected = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "entry denied");

        let listing = collect_entries(vec![Ok(entry), Err(injected)].into_iter(), &dir, 0, false);

        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].name, "visible.txt");
        assert_eq!(listing.error.as_deref(), Some("entry denied"));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn metadata_failure_is_reported_instead_of_silently_dropping_the_entry() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("dolvim-metadata-failure-{unique}"));
        fs::create_dir(&dir).unwrap();
        let vanished = dir.join("vanished.txt");
        fs::write(&vanished, b"temporary").unwrap();
        let entry = fs::read_dir(&dir).unwrap().next().unwrap().unwrap();
        fs::remove_file(vanished).unwrap();

        let listing = collect_entries(vec![Ok(entry)].into_iter(), &dir, 0, false);

        assert!(listing.entries.is_empty());
        assert!(listing.error.is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn configured_file_icons_override_generic_file_types() {
        let file = |name: &str| Entry {
            name: name.into(),
            path: PathBuf::from(name),
            backing_path: None,
            link_target: None,
            kind: Kind::File,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            trash_identity: None,
            depth: 0,
            expanded: false,
        };

        assert_eq!(file("main.rs").glyph(), "");
        assert_eq!(file("MAIN.RS").glyph(), "");
        assert_eq!(file("source.f#").glyph(), "");
        assert_eq!(file("photo.jpg").glyph(), "");
        assert_eq!(file("unknown.custom").glyph(), config::glyph::FILE);
    }

    #[test]
    fn dirs_sort_before_files_when_asked() {
        let make_entry = |name: &str, kind: Kind| Entry {
            name: name.into(),
            path: PathBuf::from(name),
            backing_path: None,
            link_target: None,
            kind,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            trash_identity: None,
            depth: 0,
            expanded: false,
        };
        let mut entries = vec![make_entry("z", Kind::Dir), make_entry("a", Kind::File)];
        sort_entries(&mut entries, Sort::default());
        assert_eq!(entries[0].name, "z");
    }

    /// Real `lsblk -rnb` output. The empty columns are the trap: a partition
    /// with no label and no mount point still emits its separators.
    #[test]
    fn lsblk_rows_parse_with_columns_missing() {
        let esp = "1073741824 vfat   0 EFI\\x20System /dev/nvme0n1p1";
        assert!(parse_device(esp).is_none());
        assert!(parse_device("17179869184 swap  [SWAP] 0 Linux\\x20swap /dev/sda3").is_none());

        let unmounted =
            parse_device("498754322432 ext4   0 Linux\\x20root\\x20(x86-64) /dev/nvme0n1p2")
                .unwrap();
        assert_eq!(unmounted.label, "");
        assert_eq!(unmounted.size, 498754322432);
        assert!(unmounted.mount.is_none());
        assert!(!unmounted.removable);

        let usb = parse_device(
            "220675866624 ext4 my\\x20disk /run/media/fredy/d 1 Linux\\x20filesystem /dev/sdc1",
        )
        .unwrap();
        assert_eq!(usb.label, "my disk");
        assert_eq!(usb.mount.unwrap(), PathBuf::from("/run/media/fredy/d"));
        assert!(usb.removable);
    }
}
