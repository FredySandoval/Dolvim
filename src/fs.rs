//! Directory listing, sorting, metadata formatting, and the listing worker.

use std::cmp::Ordering;
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
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
    /// Depth in an expanded Details tree; 0 for a plain listing.
    pub depth: u16,
    pub expanded: bool,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.kind == Kind::Dir
    }

    /// A directory the current user cannot look inside.
    pub fn is_locked(&self) -> bool {
        self.kind == Kind::Dir && !self.readable
    }

    pub fn is_executable(&self) -> bool {
        self.kind == Kind::File && self.mode & 0o111 != 0
    }

    pub fn ext(&self) -> Option<String> {
        self.path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
    }

    pub fn is_image(&self) -> bool {
        self.ext()
            .is_some_and(|e| config::IMAGE_EXTS.contains(&e.as_str()))
    }

    pub fn is_archive(&self) -> bool {
        self.ext()
            .is_some_and(|e| config::ARCHIVE_EXTS.contains(&e.as_str()))
    }

    /// The "Type" column, Dolphin-style plain-English descriptions.
    pub fn type_name(&self) -> String {
        match self.kind {
            Kind::Dir => "Folder".into(),
            Kind::Symlink => "Link".into(),
            Kind::File => match self.ext().as_deref() {
                None => {
                    if self.is_executable() {
                        "Executable".into()
                    } else {
                        "Unknown".into()
                    }
                }
                Some(e) if config::IMAGE_EXTS.contains(&e) => format!("{} image", e.to_uppercase()),
                Some(e) if config::ARCHIVE_EXTS.contains(&e) => "Archive".into(),
                Some("sh" | "bash" | "zsh" | "fish") => "Shell script".into(),
                Some("rs") => "Rust source".into(),
                Some("txt" | "md") => "Text document".into(),
                Some("pdf") => "PDF document".into(),
                Some("mp3" | "flac" | "ogg" | "wav" | "m4a") => "Audio".into(),
                Some("mp4" | "mkv" | "webm" | "avi" | "mov") => "Video".into(),
                Some(e) => format!("{} file", e.to_uppercase()),
            },
        }
    }

    /// `home()` is only touched once a name matches, so the common case is a
    /// string compare against a five-row table.
    fn home_folder_glyph(&self) -> Option<&'static str> {
        let gl = config::HOME_FOLDER_ICONS
            .iter()
            .find(|(n, _)| *n == self.name)?
            .1;
        (self.path.parent()? == crate::places::home()).then_some(gl)
    }

    pub fn glyph(&self) -> &'static str {
        use config::glyph as g;
        match self.kind {
            Kind::Symlink => g::SYMLINK,
            Kind::Dir => {
                if self.is_locked() {
                    g::FOLDER_LOCKED
                } else if self.expanded {
                    g::FOLDER_OPEN
                } else if let Some(gl) = self.home_folder_glyph() {
                    gl
                } else if self.size == 0 {
                    g::FOLDER_EMPTY
                } else {
                    g::FOLDER
                }
            }
            Kind::File => match self.ext().as_deref() {
                Some(e) if config::IMAGE_EXTS.contains(&e) => g::PICTURE,
                Some(e) if config::ARCHIVE_EXTS.contains(&e) => g::ARCHIVE,
                Some("mp3" | "flac" | "ogg" | "wav" | "m4a") => g::MUSIC,
                Some("mp4" | "mkv" | "webm" | "avi" | "mov") => g::VIDEO,
                Some("txt" | "md" | "pdf" | "doc" | "docx" | "odt") => g::DOCUMENT,
                _ => g::FILE,
            },
        }
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
    let (mut x, mut y) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (x.peek().copied(), y.peek().copied()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let na = take_number(&mut x);
                    let nb = take_number(&mut y);
                    match na.cmp(&nb) {
                        Ordering::Equal => {}
                        o => return o,
                    }
                } else {
                    let (la, lb) = (lower(ca), lower(cb));
                    match la.cmp(&lb) {
                        Ordering::Equal => {
                            x.next();
                            y.next();
                        }
                        o => return o,
                    }
                }
            }
        }
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn take_number(it: &mut std::iter::Peekable<std::str::Chars>) -> u128 {
    let mut n: u128 = 0;
    while let Some(c) = it.peek().copied() {
        if !c.is_ascii_digit() {
            break;
        }
        // Saturate rather than wrap: a 40-digit filename is not a number.
        n = n.saturating_mul(10).saturating_add(c as u128 - '0' as u128);
        it.next();
    }
    n
}

pub fn sort_entries(v: &mut [Entry], s: Sort) {
    v.sort_by(|a, b| {
        if s.dirs_first {
            match (a.is_dir(), b.is_dir()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }
        let o = match s.key {
            SortKey::Name => natural_cmp(&a.name, &b.name),
            SortKey::Size => a
                .size
                .cmp(&b.size)
                .then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Date => a
                .mtime
                .cmp(&b.mtime)
                .then_with(|| natural_cmp(&a.name, &b.name)),
            SortKey::Type => a
                .type_name()
                .cmp(&b.type_name())
                .then_with(|| natural_cmp(&a.name, &b.name)),
        };
        if s.reverse {
            o.reverse()
        } else {
            o
        }
    });
}

/// Read one directory. Unreadable entries are skipped, not fatal — Dolphin
/// shows what it can and complains in the status bar.
pub fn read_dir(path: &Path, depth: u16) -> std::io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for de in fs::read_dir(path)? {
        let Ok(de) = de else { continue };
        let name = de.file_name().to_string_lossy().into_owned();
        let p = de.path();
        let Ok(lmeta) = fs::symlink_metadata(&p) else {
            continue;
        };
        let is_link = lmeta.file_type().is_symlink();
        // Dolphin follows links for the type shown, but keeps the link marker.
        let meta = if is_link {
            fs::metadata(&p).unwrap_or(lmeta)
        } else {
            lmeta
        };
        let kind = if meta.is_dir() {
            Kind::Dir
        } else if is_link {
            Kind::Symlink
        } else {
            Kind::File
        };
        // One `read_dir` answers both "how many children" and "can we get in",
        // so the lock state costs no extra syscall.
        let children = (kind == Kind::Dir).then(|| dir_child_count(&p));
        out.push(Entry {
            hidden: name.starts_with('.'),
            name,
            path: p,
            kind,
            size: match children {
                Some(c) => c.unwrap_or(0),
                None => meta.len(),
            },
            mtime: meta.mtime(),
            mode: meta.permissions().mode(),
            readable: children.map(|c| c.is_some()).unwrap_or(true),
            depth,
            expanded: false,
        });
    }
    Ok(out)
}

/// `None` when the directory cannot be opened at all.
fn dir_child_count(p: &Path) -> Option<u64> {
    fs::read_dir(p).map(|d| d.count() as u64).ok()
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i + 1 < UNITS.len() {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.1} {}", v, UNITS[i])
}

/// Dolphin renders folder sizes as a child count, not bytes.
pub fn format_entry_size(e: &Entry) -> String {
    if e.is_dir() {
        match e.size {
            0 => "0 items".into(),
            1 => "1 item".into(),
            n => format!("{n} items"),
        }
    } else {
        format_size(e.size)
    }
}

/// `YYYY-MM-DD HH:MM`, computed from the epoch by hand. Pulling `chrono` to
/// print sixteen characters would be the exact bloat the manifesto forbids.
pub fn format_time(epoch: i64) -> String {
    let (mut days, secs) = (epoch.div_euclid(86400), epoch.rem_euclid(86400));
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
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}")
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
    /// Filesystem label; empty when the partition has none.
    pub label: String,
    pub size: u64,
    pub mount: Option<PathBuf>,
    pub removable: bool,
}

pub fn devices() -> Vec<Device> {
    let Ok(out) = std::process::Command::new("lsblk")
        .args([
            "-rnb",
            "-o",
            "SIZE,FSTYPE,LABEL,MOUNTPOINT,HOTPLUG,PARTTYPENAME",
        ])
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
    let f: Vec<&str> = line.split(' ').collect();
    if f.len() < 6 {
        return None;
    }
    // No filesystem, swap, or the EFI system partition: Dolphin shows none of
    // these, and none of them is a place a user navigates to.
    if f[1].is_empty() || f[1] == "swap" || unescape_lsblk(f[5]) == "EFI System" {
        return None;
    }
    Some(Device {
        label: unescape_lsblk(f[2]),
        size: f[0].parse().unwrap_or(0),
        mount: (!f[3].is_empty()).then(|| PathBuf::from(unescape_lsblk(f[3]))),
        removable: f[4] == "1",
    })
}

/// `lsblk -r` escapes the bytes that would otherwise break the column split.
fn unescape_lsblk(s: &str) -> String {
    s.replace("\\x20", " ")
}

/// Free/total bytes for the filesystem holding `path`.
///
/// `df` rather than `statfs(2)`: the syscall needs `unsafe` and hardcoded
/// offsets into a libc struct we do not otherwise depend on, to answer a
/// question a coreutils binary already answers exactly. See docs/DECISIONS.md.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let out = std::process::Command::new("df")
        .args(["-B1", "--output=avail,size"])
        .arg(path)
        .output()
        .ok()?;
    let txt = String::from_utf8_lossy(&out.stdout);
    let line = txt.lines().nth(1)?;
    let mut it = line.split_whitespace();
    let avail = it.next()?.parse().ok()?;
    let total = it.next()?.parse().ok()?;
    Some((avail, total))
}

// ---------------------------------------------------------------------------
// Listing worker
// ---------------------------------------------------------------------------

/// A listing request carries a generation counter so stale results from a
/// directory the user has already navigated away from are dropped, not shown.
pub struct Listing {
    pub path: PathBuf,
    pub seq: u64,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
}

pub enum Msg {
    Listed(Listing),
    /// Partial batch for huge directories, so 100k entries do not block.
    Batch(PathBuf, u64, Vec<Entry>),
    Done(PathBuf, u64),
}

/// Spawns the listing thread and returns a handle you push requests into.
pub struct Lister {
    jobs: Sender<(PathBuf, u64)>,
}

impl Lister {
    pub fn new(tx: Sender<Msg>) -> Lister {
        let (jobs, rx) = channel::<(PathBuf, u64)>();
        thread::spawn(move || {
            // Held across iterations: a request that arrives mid-listing has to
            // outlive the job it interrupts. The single-slot mutex this
            // replaced could drop such a request outright, leaving the pane
            // that asked for it `loading` with nothing ever to arrive.
            let mut pending: Option<(PathBuf, u64)> = None;
            loop {
                // Blocks between requests instead of waking a hundred times a
                // second to look at an empty slot.
                let mut job = match pending.take() {
                    Some(j) => j,
                    None => match rx.recv() {
                        Ok(j) => j,
                        // The App is gone; so is any reason to keep listing.
                        Err(_) => return,
                    },
                };
                // Whatever piled up behind this one is already newer than it.
                while let Ok(j) = rx.try_recv() {
                    job = j;
                }
                let (path, seq) = job;
                match read_dir(&path, 0) {
                    Err(e) => {
                        let _ = tx.send(Msg::Listed(Listing {
                            path,
                            seq,
                            entries: Vec::new(),
                            error: Some(e.to_string()),
                        }));
                    }
                    Ok(all) => {
                        // Stream in batches so the first screenful appears at once.
                        let mut sent = false;
                        for chunk in all.chunks(2000) {
                            // A newer request supersedes this one; abandon it,
                            // keeping the request for the next turn.
                            while let Ok(j) = rx.try_recv() {
                                pending = Some(j);
                            }
                            if pending.is_some() {
                                break;
                            }
                            let _ = tx.send(Msg::Batch(path.clone(), seq, chunk.to_vec()));
                            sent = true;
                        }
                        if !sent {
                            let _ = tx.send(Msg::Batch(path.clone(), seq, Vec::new()));
                        }
                        let _ = tx.send(Msg::Done(path, seq));
                    }
                }
            }
        });
        Lister { jobs }
    }

    pub fn request(&self, path: PathBuf, seq: u64) {
        let _ = self.jobs.send((path, seq));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_sort_orders_digit_runs_numerically() {
        let mut v = vec!["file10", "file9", "File2", "a"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, vec!["a", "File2", "file9", "file10"]);
    }

    #[test]
    fn giant_digit_runs_do_not_panic() {
        let long = "9".repeat(60);
        assert_eq!(natural_cmp(&long, &long), Ordering::Equal);
    }

    #[test]
    fn sizes_match_dolphin_formatting() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(701), "701 B");
        assert_eq!(format_size(1024), "1.0 KiB");
        assert_eq!(format_size(499_289_948_160), "465.0 GiB");
    }

    #[test]
    fn epoch_converts_to_civil_time() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_753_776_000), "2025-07-29 08:00");
    }

    #[test]
    fn dirs_sort_before_files_when_asked() {
        let mk = |n: &str, k: Kind| Entry {
            name: n.into(),
            path: PathBuf::from(n),
            kind: k,
            size: 0,
            mtime: 0,
            mode: 0,
            readable: true,
            hidden: false,
            depth: 0,
            expanded: false,
        };
        let mut v = vec![mk("z", Kind::Dir), mk("a", Kind::File)];
        sort_entries(&mut v, Sort::default());
        assert_eq!(v[0].name, "z");
    }

    /// Real `lsblk -rnb` output. The empty columns are the trap: a partition
    /// with no label and no mount point still emits its separators.
    #[test]
    fn lsblk_rows_parse_with_columns_missing() {
        let esp = "1073741824 vfat   0 EFI\\x20System";
        assert!(parse_device(esp).is_none());
        assert!(parse_device("17179869184 swap  [SWAP] 0 Linux\\x20swap").is_none());

        let unmounted = parse_device("498754322432 ext4   0 Linux\\x20root\\x20(x86-64)").unwrap();
        assert_eq!(unmounted.label, "");
        assert_eq!(unmounted.size, 498754322432);
        assert!(unmounted.mount.is_none());
        assert!(!unmounted.removable);

        let usb =
            parse_device("220675866624 ext4 my\\x20disk /run/media/fredy/d 1 Linux\\x20filesystem")
                .unwrap();
        assert_eq!(usb.label, "my disk");
        assert_eq!(usb.mount.unwrap(), PathBuf::from("/run/media/fredy/d"));
        assert!(usb.removable);
    }
}
