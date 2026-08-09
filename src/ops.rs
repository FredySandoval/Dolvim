//! File operations: copy, move, trash, delete, rename, clipboard, undo.
//!
//! Long operations run on a worker thread behind a `Progress` handle so the
//! UI stays live and cancellable, exactly like Dolphin's progress popup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::config;
use crate::fs::{Entry, Kind};

// ---------------------------------------------------------------------------
// Opening files
// ---------------------------------------------------------------------------

/// Does this file's handler want a terminal of its own?
///
/// The desktop entry answers outright, in `Terminal=`. Every shortcut gets it
/// wrong in one direction or the other: the mime type cannot tell `nvim` from
/// a graphical editor for the same `text/x-c`, and `$DISPLAY` is set here even
/// though the handler is a tty program. Unknown means "not a terminal app",
/// which is the safe way to be wrong — a detached child cannot corrupt our
/// screen, whereas a terminal one we failed to yield to certainly can.
pub fn opens_in_terminal(path: &Path) -> bool {
    let Some(entry) = desktop_entry(path) else {
        return false;
    };
    entry
        .lines()
        // `Terminal=` belongs to [Desktop Entry]; later groups are per-action.
        .take_while(|l| !l.starts_with("[Desktop Action"))
        .any(|l| l.trim().eq_ignore_ascii_case("terminal=true"))
}

fn desktop_entry(path: &Path) -> Option<String> {
    let mime = xdg_mime(&["query", "filetype", &path.to_string_lossy()])?;
    let id = xdg_mime(&["query", "default", &mime])?;
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| crate::places::home().join(".local/share"));
    let dirs =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    std::iter::once(home)
        .chain(dirs.split(':').map(PathBuf::from))
        .find_map(|d| fs::read_to_string(d.join("applications").join(&id)).ok())
}

fn xdg_mime(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("xdg-mime")
        .args(args)
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!s.is_empty()).then_some(s)
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrashRef {
    /// Backend identity of one specific generation in the Trash.
    pub id: std::ffi::OsString,
    pub original_path: PathBuf,
    pub name: std::ffi::OsString,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Clipboard {
    #[default]
    Empty,
    Live {
        paths: Vec<PathBuf>,
        cut: bool,
    },
    Deleted {
        items: Vec<TrashRef>,
    },
}

impl Clipboard {
    pub fn set(&mut self, paths: Vec<PathBuf>, cut: bool) {
        export_uris(&paths);
        *self = if paths.is_empty() {
            Self::Empty
        } else {
            Self::Live { paths, cut }
        };
    }

    pub fn set_deleted(&mut self, items: Vec<TrashRef>) {
        *self = if items.is_empty() {
            Self::Empty
        } else {
            Self::Deleted { items }
        };
    }

    pub fn cut_paths(&self) -> &[PathBuf] {
        match self {
            Self::Live { paths, cut: true } => paths,
            _ => &[],
        }
    }
}

/// The tools that put a `text/uri-list` on the system clipboard, in the order
/// they are tried.
const CLIPBOARD_WRITE_TOOLS: [(&str, &[&str]); 3] = [
    ("wl-copy", &["--type", "text/uri-list"]),
    ("xclip", &["-selection", "clipboard", "-t", "text/uri-list"]),
    ("xsel", &["--clipboard", "--input"]),
];

/// The tools that read a `text/uri-list` back off it.
const CLIPBOARD_READ_TOOLS: [(&str, &[&str]); 3] = [
    ("wl-paste", &["--type", "text/uri-list", "--no-newline"]),
    (
        "xclip",
        &["-selection", "clipboard", "-o", "-t", "text/uri-list"],
    ),
    ("xsel", &["--clipboard", "--output"]),
];

/// Publish the selection as `text/uri-list` for other applications.
///
/// Whichever of wl-copy/xclip/xsel exists wins; if none does, OSC 52 puts at
/// least the newline-joined paths on the terminal's clipboard. No crate, no
/// daemon — see docs/DECISIONS.md.
fn export_uris(paths: &[PathBuf]) {
    let uris: String = paths
        .iter()
        .map(|p| format!("file://{}\n", percent_encode(&p.to_string_lossy())))
        .collect();
    for (bin, args) in CLIPBOARD_WRITE_TOOLS {
        if which(bin).is_none() {
            continue;
        }
        let child = std::process::Command::new(bin)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut clipboard_child) = child {
            use std::io::Write;
            if let Some(mut child_stdin) = clipboard_child.stdin.take() {
                let _ = child_stdin.write_all(uris.as_bytes());
            }
            // Selection owners must outlive us; do not wait on them.
            return;
        }
    }
    osc52(&uris);
}

fn osc52(text: &str) {
    use std::io::Write;
    let b64 = base64(text.as_bytes());
    let mut out = io::stdout();
    let _ = write!(out, "\x1b]52;c;{b64}\x07");
    let _ = out.flush();
}

/// Twenty lines of base64 beats a dependency for one escape sequence.
fn base64(data: &[u8]) -> String {
    const BASE64_ALPHABET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for byte_triple in data.chunks(3) {
        let padded = [
            byte_triple[0],
            *byte_triple.get(1).unwrap_or(&0),
            *byte_triple.get(2).unwrap_or(&0),
        ];
        let packed = ((padded[0] as u32) << 16) | ((padded[1] as u32) << 8) | padded[2] as u32;
        out.push(BASE64_ALPHABET[(packed >> 18) as usize & 63] as char);
        out.push(BASE64_ALPHABET[(packed >> 12) as usize & 63] as char);
        out.push(if byte_triple.len() > 1 {
            BASE64_ALPHABET[(packed >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if byte_triple.len() > 2 {
            BASE64_ALPHABET[packed as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

/// Read `text/uri-list` back out of the system clipboard, for Paste of files
/// copied in another application.
pub fn import_uris() -> Vec<PathBuf> {
    for (bin, args) in CLIPBOARD_READ_TOOLS {
        if which(bin).is_none() {
            continue;
        }
        if let Ok(paste_output) = std::process::Command::new(bin).args(args).output() {
            let uri_list_text = String::from_utf8_lossy(&paste_output.stdout);
            let imported_paths: Vec<PathBuf> = uri_list_text
                .lines()
                .filter_map(|uri_line| uri_line.trim().strip_prefix("file://"))
                .map(|uri_line| PathBuf::from(percent_decode(uri_line)))
                .collect();
            if !imported_paths.is_empty() {
                return imported_paths;
            }
        }
    }
    Vec::new()
}

fn percent_decode(s: &str) -> String {
    let input_bytes = s.as_bytes();
    let mut decoded = Vec::with_capacity(input_bytes.len());
    let mut index = 0;
    while index < input_bytes.len() {
        if input_bytes[index] == b'%' && index + 2 < input_bytes.len() {
            if let Ok(decoded_byte) = u8::from_str_radix(&s[index + 1..index + 3], 16) {
                decoded.push(decoded_byte);
                index += 3;
                continue;
            }
        }
        decoded.push(input_bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

// ---------------------------------------------------------------------------
// Undo journal
// ---------------------------------------------------------------------------

/// Exactly the set Dolphin can undo: renames, moves, trashing, and creation.
/// A recursive copy is *not* undoable in Dolphin either — it would mean
/// deleting files the user may have since edited.
#[derive(Clone, Debug)]
pub enum UndoOp {
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Move {
        moved_pairs: Vec<(PathBuf, PathBuf)>,
    },
    Trash {
        items: Vec<TrashRef>,
    },
    /// A paste which consumed deleted-register items by restoring them.
    Restore {
        restored_paths: Vec<PathBuf>,
        previous_items: Vec<TrashRef>,
    },
    Create {
        path: PathBuf,
    },
}

#[derive(Clone, Debug)]
pub struct RegisterChange {
    pub expected: Clipboard,
    pub replacement: Clipboard,
}

#[derive(Clone, Debug)]
pub struct UndoOutcome {
    pub message: String,
    pub register_change: Option<RegisterChange>,
    /// Rebind older delete history from consumed Trash generations to the new
    /// generations created while undoing a restore-paste.
    pub trash_replacements: Vec<(std::ffi::OsString, TrashRef)>,
}

impl UndoOutcome {
    fn message(message: String) -> Self {
        Self {
            message,
            register_change: None,
            trash_replacements: Vec::new(),
        }
    }
}

pub fn undo(op: &UndoOp) -> Result<UndoOutcome, String> {
    match op {
        UndoOp::Rename { from, to } => {
            fs::rename(to, from).map_err(|e| e.to_string())?;
            Ok(UndoOutcome::message(format!(
                "Renamed back to {}",
                file_name_of(from)
            )))
        }
        UndoOp::Move { moved_pairs } => {
            for (from, to) in moved_pairs {
                if let Some(p) = from.parent() {
                    let _ = fs::create_dir_all(p);
                }
                fs::rename(to, from).map_err(|e| e.to_string())?;
            }
            Ok(UndoOutcome::message(format!(
                "Moved {} item(s) back",
                moved_pairs.len()
            )))
        }
        UndoOp::Trash { items } => {
            let restored = restore_trash_refs(items, None)?;
            let mut outcome =
                UndoOutcome::message(format!("Restored {} item(s) from Trash", items.len()));
            outcome.register_change = Some(RegisterChange {
                expected: Clipboard::Deleted {
                    items: items.clone(),
                },
                replacement: Clipboard::Live {
                    paths: restored,
                    cut: false,
                },
            });
            Ok(outcome)
        }
        UndoOp::Restore {
            restored_paths,
            previous_items,
        } => {
            let deleted = trash(restored_paths);
            if !deleted.is_complete() || deleted.committed.len() != restored_paths.len() {
                return Err(format!(
                    "Could not move the restored paste back to Trash: {}",
                    deleted
                        .failed
                        .first()
                        .map(|failure| failure.message.as_str())
                        .unwrap_or("incomplete operation")
                ));
            }
            let replacements = previous_items
                .iter()
                .zip(&deleted.committed)
                .map(|(old, new)| (old.id.clone(), new.clone()))
                .collect();
            Ok(UndoOutcome {
                message: format!("Undid paste of {} item(s)", restored_paths.len()),
                register_change: Some(RegisterChange {
                    expected: Clipboard::Live {
                        paths: restored_paths.clone(),
                        cut: false,
                    },
                    replacement: Clipboard::Deleted {
                        items: deleted.committed,
                    },
                }),
                trash_replacements: replacements,
            })
        }
        UndoOp::Create { path } => {
            if path.is_dir() {
                fs::remove_dir(path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            Ok(UndoOutcome::message(format!(
                "Removed {}",
                file_name_of(path)
            )))
        }
    }
}

pub fn rebase_trash_history(
    history: &mut [UndoOp],
    replacements: &[(std::ffi::OsString, TrashRef)],
) {
    for op in history {
        if let UndoOp::Trash { items } = op {
            for item in items {
                if let Some((_, replacement)) =
                    replacements.iter().find(|(old_id, _)| *old_id == item.id)
                {
                    *item = replacement.clone();
                }
            }
        }
    }
}

pub fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Trash
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemFailure {
    pub path: PathBuf,
    pub message: String,
}

/// The actual effects of a multi-item operation. `committed` is authoritative:
/// callers must journal and update the register from it, never from the request.
#[derive(Clone, Debug, Default)]
pub struct TrashOutcome {
    pub committed: Vec<TrashRef>,
    pub failed: Vec<ItemFailure>,
}

impl TrashOutcome {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// Trash each operand separately and retain the backend identity of every item
/// that actually moved. This intentionally does not flatten partial completion
/// into `Err(String)`.
pub fn trash(paths: &[PathBuf]) -> TrashOutcome {
    let mut outcome = TrashOutcome::default();
    for path in paths {
        let before = trash::os_limited::list()
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| item.id)
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        if let Err(error) = trash::delete(path) {
            outcome.failed.push(ItemFailure {
                path: path.clone(),
                message: trash_error(error),
            });
            continue;
        }
        let found = trash::os_limited::list().ok().and_then(|items| {
            items.into_iter().find(|item| {
                !before.contains(&item.id) && item.original_parent.join(&item.name) == *path
            })
        });
        match found {
            Some(item) => outcome.committed.push(TrashRef {
                id: item.id,
                original_path: path.clone(),
                name: item.name,
            }),
            None => outcome.failed.push(ItemFailure {
                path: path.clone(),
                message: "Moved to Trash, but its backend identity could not be read".into(),
            }),
        }
    }
    outcome
}

/// Trash contents as view entries, so the Trash place browses like a folder.
pub fn list_trash() -> Vec<Entry> {
    let Ok(items) = trash::os_limited::list() else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|trash_item| {
            let original = trash_item.original_parent.join(&trash_item.name);
            let meta = fs::symlink_metadata(&original).ok();
            Entry {
                name: trash_item.name.to_string_lossy().into_owned(),
                path: original,
                kind: match meta.as_ref().map(|m| m.is_dir()) {
                    Some(true) => Kind::Dir,
                    _ => Kind::File,
                },
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                mtime: trash_item.time_deleted,
                mode: 0o644,
                readable: true,
                hidden: false,
                depth: 0,
                expanded: false,
            }
        })
        .collect()
}

/// The crate's `Display` prints its Debug-ish struct variant — a path and a
/// field name where the status bar has room for a sentence. Say what happened.
fn trash_error(e: trash::Error) -> String {
    match e {
        trash::Error::CanonicalizePath { original } => {
            format!("{}: no longer there", file_name_of(&original))
        }
        // `path` here is a directory inside the trash can, not the file the
        // user asked about, so naming it would only mislead. The io error is
        // the part that says why.
        trash::Error::FileSystem { source, .. } => {
            format!("Cannot write to the Trash: {source}")
        }
        trash::Error::CouldNotAccess { target } => format!("{target}: cannot access"),
        trash::Error::TargetedRoot => "Refusing to trash a filesystem root".into(),
        e => format!("Trash failed: {e}"),
    }
}

/// A Trash entry's `path` is where it came from, not where it now sits, so
/// both operations on trashed items look them up by that original path.
fn trash_items(originals: &[PathBuf]) -> Result<Vec<trash::TrashItem>, String> {
    let items = trash::os_limited::list().map_err(trash_error)?;
    let wanted: Vec<_> = items
        .into_iter()
        .filter(|trash_item| {
            originals.iter().any(|original_path| {
                trash_item.original_parent.join(&trash_item.name) == *original_path
            })
        })
        .collect();
    if wanted.is_empty() {
        return Err("No matching items in Trash".into());
    }
    Ok(wanted)
}

fn restore_trash_refs(
    items: &[TrashRef],
    destination: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let listed = trash::os_limited::list().map_err(trash_error)?;
    let by_id: std::collections::HashMap<_, _> = listed
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();
    let mut wanted = Vec::with_capacity(items.len());
    let mut restored_paths = Vec::with_capacity(items.len());
    for item_ref in items {
        let mut item = by_id.get(&item_ref.id).cloned().ok_or_else(|| {
            format!(
                "{} is no longer in Trash",
                file_name_of(&item_ref.original_path)
            )
        })?;
        let path = if let Some(dir) = destination {
            item.original_parent = dir.to_path_buf();
            dir.join(&item.name)
        } else {
            item_ref.original_path.clone()
        };
        restored_paths.push(path);
        wanted.push(item);
    }
    trash::os_limited::restore_all(wanted).map_err(trash_error)?;
    Ok(restored_paths)
}

pub fn restore_deleted_to(items: &[TrashRef], destination: &Path) -> Result<Vec<PathBuf>, String> {
    restore_trash_refs(items, Some(destination))
}

pub fn restore_from_trash(originals: &[PathBuf]) -> Result<usize, String> {
    let wanted = trash_items(originals)?;
    let n = wanted.len();
    trash::os_limited::restore_all(wanted).map_err(trash_error)?;
    Ok(n)
}

pub fn purge_from_trash(originals: &[PathBuf]) -> Result<usize, String> {
    let wanted = trash_items(originals)?;
    let n = wanted.len();
    trash::os_limited::purge_all(wanted).map_err(trash_error)?;
    Ok(n)
}

pub fn empty_trash() -> Result<usize, String> {
    let items = trash::os_limited::list().map_err(trash_error)?;
    let n = items.len();
    trash::os_limited::purge_all(items).map_err(trash_error)?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// Create / rename
// ---------------------------------------------------------------------------

pub fn new_folder(dir: &Path, name: &str) -> Result<UndoOp, String> {
    let p = dir.join(name);
    if p.exists() {
        return Err(format!("{name} already exists"));
    }
    fs::create_dir(&p).map_err(|e| e.to_string())?;
    Ok(UndoOp::Create { path: p })
}

pub fn new_file(dir: &Path, name: &str) -> Result<UndoOp, String> {
    let p = dir.join(name);
    if p.exists() {
        return Err(format!("{name} already exists"));
    }
    fs::File::create(&p).map_err(|e| e.to_string())?;
    Ok(UndoOp::Create { path: p })
}

pub fn rename(from: &Path, new_name: &str) -> Result<UndoOp, String> {
    if new_name.is_empty() || new_name.contains('/') {
        return Err("Invalid name".into());
    }
    let to = from.with_file_name(new_name);
    if to == from {
        return Err("Unchanged".into());
    }
    if to.exists() {
        return Err(format!("{new_name} already exists"));
    }
    fs::rename(from, &to).map_err(|e| e.to_string())?;
    Ok(UndoOp::Rename {
        from: from.to_path_buf(),
        to,
    })
}

/// Dolphin's batch rename: `#` in the pattern expands to a zero-padded index,
/// widened to the number of `#`s. `Holiday #.jpg` → `Holiday 1.jpg`.
pub fn batch_rename(paths: &[PathBuf], pattern: &str) -> Result<UndoOp, String> {
    if !pattern.contains('#') {
        return Err("Pattern must contain # for the counter".into());
    }
    let hash_count = pattern.chars().filter(|c| *c == '#').count();
    let mut renamed_pairs = Vec::new();
    for (index, source_path) in paths.iter().enumerate() {
        let counter_text = format!("{:0width$}", index + 1, width = hash_count);
        let mut new_name = String::new();
        let mut counter_written = false;
        for pattern_char in pattern.chars() {
            if pattern_char == '#' {
                if !counter_written {
                    new_name.push_str(&counter_text);
                    counter_written = true;
                }
            } else {
                new_name.push(pattern_char);
            }
        }
        // Keep the original extension when the pattern does not supply one.
        let new_name = if !new_name.contains('.') {
            match source_path.extension() {
                Some(extension) => format!("{new_name}.{}", extension.to_string_lossy()),
                None => new_name,
            }
        } else {
            new_name
        };
        let to = source_path.with_file_name(&new_name);
        if to.exists() && to != *source_path {
            return Err(format!("{new_name} already exists"));
        }
        fs::rename(source_path, &to).map_err(|e| format!("{}: {e}", file_name_of(source_path)))?;
        renamed_pairs.push((source_path.clone(), to));
    }
    Ok(UndoOp::Move {
        moved_pairs: renamed_pairs,
    })
}

// ---------------------------------------------------------------------------
// Copy / move with progress
// ---------------------------------------------------------------------------

pub struct Progress {
    pub kind: TransferKind,
    pub label: String,
    pub total_bytes: Arc<AtomicU64>,
    pub copied_bytes: Arc<AtomicU64>,
    pub current_file: Arc<Mutex<String>>,
    pub cancel_requested: Arc<AtomicBool>,
    pub finished: Arc<AtomicBool>,
    pub outcome: Arc<Mutex<Option<Result<UndoOp, String>>>>,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        let total_bytes = self.total_bytes.load(Ordering::Relaxed);
        if total_bytes == 0 {
            return 0.0;
        }
        (self.copied_bytes.load(Ordering::Relaxed) as f64 / total_bytes as f64).clamp(0.0, 1.0)
    }
}

/// Whether a transfer leaves the sources in place or removes them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TransferKind {
    Copy,
    Move,
}

/// Start a copy or move in the background. Returns immediately.
pub fn start_transfer(sources: Vec<PathBuf>, dest: PathBuf, kind: TransferKind) -> Progress {
    let transfer_progress = Progress {
        kind,
        label: format!(
            "{} {} item(s) to {}",
            match kind {
                TransferKind::Move => "Moving",
                TransferKind::Copy => "Copying",
            },
            sources.len(),
            file_name_of(&dest)
        ),
        total_bytes: Arc::new(AtomicU64::new(0)),
        copied_bytes: Arc::new(AtomicU64::new(0)),
        current_file: Arc::new(Mutex::new(String::new())),
        cancel_requested: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
        outcome: Arc::new(Mutex::new(None)),
    };
    let (total_bytes, copied_bytes, current_file, cancel_requested, finished, outcome) = (
        Arc::clone(&transfer_progress.total_bytes),
        Arc::clone(&transfer_progress.copied_bytes),
        Arc::clone(&transfer_progress.current_file),
        Arc::clone(&transfer_progress.cancel_requested),
        Arc::clone(&transfer_progress.finished),
        Arc::clone(&transfer_progress.outcome),
    );

    thread::spawn(move || {
        total_bytes.store(
            sources.iter().map(|s| tree_size(s)).sum(),
            Ordering::Relaxed,
        );
        let mut moved_pairs = Vec::new();
        let mut err = None;
        for src in &sources {
            if cancel_requested.load(Ordering::Relaxed) {
                break;
            }
            let target = unique_target(&dest.join(file_name_of(src)));
            // A rename within one filesystem is instant; try it first.
            if kind == TransferKind::Move && fs::rename(src, &target).is_ok() {
                copied_bytes.fetch_add(tree_size(src), Ordering::Relaxed);
                moved_pairs.push((src.clone(), target));
                continue;
            }
            if let Err(e) = copy_tree(
                src,
                &target,
                &copied_bytes,
                &current_file,
                &cancel_requested,
            ) {
                err = Some(format!("{}: {e}", file_name_of(src)));
                break;
            }
            if kind == TransferKind::Move && !cancel_requested.load(Ordering::Relaxed) {
                if let Err(remove_error) = remove_tree(src) {
                    err = Some(format!("{}: {remove_error}", file_name_of(src)));
                    break;
                }
                moved_pairs.push((src.clone(), target));
            }
        }
        let result = match err {
            Some(e) => Err(e),
            None if cancel_requested.load(Ordering::Relaxed) => Err("Cancelled".into()),
            None if kind == TransferKind::Move => Ok(UndoOp::Move { moved_pairs }),
            // A copy leaves nothing to undo, so report it as a no-op journal
            // entry the caller drops.
            None => Ok(UndoOp::Move {
                moved_pairs: Vec::new(),
            }),
        };
        if let Ok(mut outcome_guard) = outcome.lock() {
            *outcome_guard = Some(result);
        }
        finished.store(true, Ordering::Relaxed);
    });
    transfer_progress
}

fn tree_size(path: &Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return 0;
    };
    if metadata.is_dir() {
        fs::read_dir(path)
            .map(|read_dir| {
                read_dir
                    .flatten()
                    .map(|dir_entry| tree_size(&dir_entry.path()))
                    .sum()
            })
            .unwrap_or(0)
    } else {
        metadata.len()
    }
}

/// Never clobber silently: `file.txt` becomes `file (1).txt`, as Dolphin does
/// when you paste into the directory a file already lives in.
fn unique_target(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path
        .file_stem()
        .map(|stem_os| stem_os.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = path
        .extension()
        .map(|ext_os| format!(".{}", ext_os.to_string_lossy()))
        .unwrap_or_default();
    for suffix_number in 1..10_000 {
        let candidate = path.with_file_name(format!("{stem} ({suffix_number}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path.to_path_buf()
}

fn copy_tree(
    src: &Path,
    dest: &Path,
    copied_bytes: &Arc<AtomicU64>,
    current_file: &Arc<Mutex<String>>,
    cancel_requested: &Arc<AtomicBool>,
) -> io::Result<()> {
    if cancel_requested.load(Ordering::Relaxed) {
        return Ok(());
    }
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dest)?;
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir_all(dest)?;
        for dir_entry in fs::read_dir(src)? {
            let dir_entry = dir_entry?;
            copy_tree(
                &dir_entry.path(),
                &dest.join(dir_entry.file_name()),
                copied_bytes,
                current_file,
                cancel_requested,
            )?;
            if cancel_requested.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
        return Ok(());
    }
    if let Ok(mut current_file_guard) = current_file.lock() {
        *current_file_guard = file_name_of(src);
    }
    copy_file_streaming(src, dest, copied_bytes, cancel_requested)
}

/// Copy in chunks so the progress bar moves during a 4 GiB file and cancel is
/// honoured mid-file rather than only between files.
fn copy_file_streaming(
    src: &Path,
    dest: &Path,
    copied_bytes: &Arc<AtomicU64>,
    cancel_requested: &Arc<AtomicBool>,
) -> io::Result<()> {
    use io::{Read, Write};
    let mut source_file = fs::File::open(src)?;
    let mut dest_file = fs::File::create(dest)?;
    let mut buf = vec![0u8; config::COPY_CHUNK_BYTES];
    loop {
        if cancel_requested.load(Ordering::Relaxed) {
            // A half-written file is worse than none; take it back out.
            drop(dest_file);
            let _ = fs::remove_file(dest);
            return Ok(());
        }
        let n = source_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dest_file.write_all(&buf[..n])?;
        copied_bytes.fetch_add(n as u64, Ordering::Relaxed);
    }
    if let Ok(m) = fs::metadata(src) {
        let _ = fs::set_permissions(dest, m.permissions());
    }
    Ok(())
}

pub fn remove_tree(p: &Path) -> io::Result<()> {
    let m = fs::symlink_metadata(p)?;
    if m.is_dir() && !m.file_type().is_symlink() {
        fs::remove_dir_all(p)
    } else {
        fs::remove_file(p)
    }
}

pub fn delete_permanently(paths: &[PathBuf]) -> Result<usize, String> {
    let mut n = 0;
    for p in paths {
        remove_tree(p).map_err(|e| format!("{}: {e}", file_name_of(p)))?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Archives — shelled out, presence-checked, as PLAN.md specifies
// ---------------------------------------------------------------------------

pub fn extract(archive: &Path, dest_dir: &Path) -> Result<String, String> {
    let ext = archive
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let (bin, args): (&str, Vec<String>) = match ext.as_str() {
        "zip" => (
            "unzip",
            vec![
                "-o".into(),
                archive.to_string_lossy().into(),
                "-d".into(),
                dest_dir.to_string_lossy().into(),
            ],
        ),
        "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" => (
            "tar",
            vec![
                "-xaf".into(),
                archive.to_string_lossy().into(),
                "-C".into(),
                dest_dir.to_string_lossy().into(),
            ],
        ),
        _ => return Err(format!("Cannot extract .{ext}")),
    };
    if which(bin).is_none() {
        return Err(format!("{bin} is not installed"));
    }
    let out = std::process::Command::new(bin)
        .args(&args)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(format!("Extracted {}", file_name_of(archive)))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

pub fn compress(paths: &[PathBuf], dest: &Path) -> Result<String, String> {
    if which("tar").is_none() {
        return Err("tar is not installed".into());
    }
    let parent = paths
        .first()
        .and_then(|p| p.parent())
        .ok_or("Nothing selected")?;
    let mut cmd = std::process::Command::new("tar");
    cmd.arg("-czf").arg(dest).arg("-C").arg(parent);
    for p in paths {
        cmd.arg(file_name_of(p));
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(format!("Created {}", file_name_of(dest)))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let temp_dir =
            std::env::temp_dir().join(format!("dolvim-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();
        temp_dir
    }

    #[test]
    fn rename_round_trips_through_undo() {
        let temp_dir = tmpdir("rename");
        let file_path = temp_dir.join("a.txt");
        fs::write(&file_path, b"x").unwrap();
        let undo_op = rename(&file_path, "b.txt").unwrap();
        assert!(temp_dir.join("b.txt").exists() && !file_path.exists());
        undo(&undo_op).unwrap();
        assert!(file_path.exists() && !temp_dir.join("b.txt").exists());
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn rename_refuses_to_clobber() {
        let temp_dir = tmpdir("clobber");
        fs::write(temp_dir.join("a"), b"1").unwrap();
        fs::write(temp_dir.join("b"), b"2").unwrap();
        assert!(rename(&temp_dir.join("a"), "b").is_err());
        assert_eq!(fs::read(temp_dir.join("b")).unwrap(), b"2");
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn batch_rename_pads_to_the_hash_count() {
        let temp_dir = tmpdir("batch");
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| {
                let file_path = temp_dir.join(format!("src{i}.jpg"));
                fs::write(&file_path, b"x").unwrap();
                file_path
            })
            .collect();
        batch_rename(&paths, "Holiday ##").unwrap();
        assert!(temp_dir.join("Holiday 01.jpg").exists());
        assert!(temp_dir.join("Holiday 03.jpg").exists());
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn batch_rename_is_undoable_as_a_move() {
        let temp_dir = tmpdir("batchundo");
        let file_path = temp_dir.join("one.txt");
        fs::write(&file_path, b"x").unwrap();
        let undo_op = batch_rename(std::slice::from_ref(&file_path), "n#").unwrap();
        assert!(temp_dir.join("n1.txt").exists());
        undo(&undo_op).unwrap();
        assert!(file_path.exists());
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn unique_target_never_overwrites() {
        let temp_dir = tmpdir("unique");
        let file_path = temp_dir.join("f.txt");
        fs::write(&file_path, b"x").unwrap();
        assert_eq!(unique_target(&file_path), temp_dir.join("f (1).txt"));
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn copy_tree_reproduces_the_whole_subtree() {
        let temp_dir = tmpdir("copytree");
        fs::create_dir_all(temp_dir.join("src/sub")).unwrap();
        fs::write(temp_dir.join("src/sub/f"), b"hello").unwrap();
        let done = Arc::new(AtomicU64::new(0));
        let current_file = Arc::new(Mutex::new(String::new()));
        let cancel_requested = Arc::new(AtomicBool::new(false));
        copy_tree(
            &temp_dir.join("src"),
            &temp_dir.join("dst"),
            &done,
            &current_file,
            &cancel_requested,
        )
        .unwrap();
        assert_eq!(fs::read(temp_dir.join("dst/sub/f")).unwrap(), b"hello");
        assert_eq!(done.load(Ordering::Relaxed), 5);
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn cancelling_mid_copy_leaves_no_partial_file() {
        let temp_dir = tmpdir("cancel");
        fs::write(temp_dir.join("big"), vec![0u8; 1024]).unwrap();
        let done = Arc::new(AtomicU64::new(0));
        let cancel_requested = Arc::new(AtomicBool::new(true));
        copy_file_streaming(
            &temp_dir.join("big"),
            &temp_dir.join("out"),
            &done,
            &cancel_requested,
        )
        .unwrap();
        assert!(!temp_dir.join("out").exists());
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn deleted_register_restores_into_paste_destination() {
        let source_dir = tmpdir("deleted-register-source");
        let destination_dir = tmpdir("deleted-register-destination");
        let original = source_dir.join("note.txt");
        fs::write(&original, b"preserved").unwrap();

        let deleted = trash(std::slice::from_ref(&original));
        assert!(deleted.failed.is_empty());
        assert!(!original.exists());
        let restored = restore_deleted_to(&deleted.committed, &destination_dir).unwrap();

        let pasted = destination_dir.join("note.txt");
        assert_eq!(restored, vec![pasted.clone()]);
        assert_eq!(fs::read(pasted).unwrap(), b"preserved");
        fs::remove_dir_all(source_dir).unwrap();
        fs::remove_dir_all(destination_dir).unwrap();
    }

    #[test]
    fn clipboard_distinguishes_deleted_items_from_ordinary_copies() {
        let path = PathBuf::from("item");
        let item = TrashRef {
            id: "trash-id".into(),
            original_path: path.clone(),
            name: "item".into(),
        };
        let mut clipboard = Clipboard::default();
        clipboard.set_deleted(vec![item.clone()]);
        assert_eq!(clipboard, Clipboard::Deleted { items: vec![item] });

        clipboard.set(vec![path.clone()], false);
        assert_eq!(
            clipboard,
            Clipboard::Live {
                paths: vec![path],
                cut: false
            }
        );
    }

    #[test]
    fn base64_matches_rfc4648_padding() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn uri_encoding_round_trips_spaces() {
        assert_eq!(percent_encode("/file_path b/c"), "/file_path%20b/c");
        assert_eq!(percent_decode("/file_path%20b/c"), "/file_path b/c");
    }
}
