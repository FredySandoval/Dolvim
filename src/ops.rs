//! File operations: copy, move, trash, delete, rename, clipboard, undo.
//!
//! Long operations run on a worker thread behind a `Progress` handle so the
//! UI stays live and cancellable, exactly like Dolphin's progress popup.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::config;
use crate::fs::{Entry, Kind};

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

impl TrashRef {
    pub fn selection_key(&self) -> PathBuf {
        let mut key = std::ffi::OsString::from("trash-generation:");
        key.push(&self.id);
        PathBuf::from(key)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum UnnamedRegister {
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

impl UnnamedRegister {
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

    /// Whether this is the register generation a deferred change expects.
    /// Trash ids are the durable identity; restore-location metadata may differ
    /// between a paste undo and the older delete undo that shares those bytes.
    pub fn matches_expected(&self, expected: &Self) -> bool {
        match (self, expected) {
            (
                Self::Deleted { items },
                Self::Deleted {
                    items: expected_items,
                },
            ) => {
                items.len() == expected_items.len()
                    && items
                        .iter()
                        .zip(expected_items)
                        .all(|(item, expected_item)| item.id == expected_item.id)
            }
            _ => self == expected,
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
    pub expected: UnnamedRegister,
    pub replacement: UnnamedRegister,
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
            let mut reversed: Vec<&(PathBuf, PathBuf)> = Vec::new();
            for pair @ (from, to) in moved_pairs.iter().rev() {
                if let Some(parent) = from.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                if let Err(error) = fs::rename(to, from) {
                    // Restore the pre-undo state so the unchanged journal entry
                    // remains truthful when the caller puts it back.
                    for (rolled_from, rolled_to) in reversed.into_iter().rev() {
                        let _ = fs::rename(rolled_from, rolled_to);
                    }
                    return Err(error.to_string());
                }
                reversed.push(pair);
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
                expected: UnnamedRegister::Deleted {
                    items: items.clone(),
                },
                replacement: UnnamedRegister::Live {
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
                    expected: UnnamedRegister::Live {
                        paths: restored_paths.clone(),
                        cut: false,
                    },
                    replacement: UnnamedRegister::Deleted {
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
                    // The new generation identifies where the bytes are now in
                    // Trash, but this older undo entry must retain the location
                    // it originally promised to restore.
                    item.id = replacement.id.clone();
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

fn trash_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

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
    let _guard = trash_lock();
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
                trash_id: Some(trash_item.id),
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

fn exact_trash_items(items: &[TrashRef]) -> Result<Vec<trash::TrashItem>, String> {
    let listed = trash::os_limited::list().map_err(trash_error)?;
    let mut by_id: std::collections::HashMap<_, _> = listed
        .into_iter()
        .map(|item| (item.id.clone(), item))
        .collect();
    items
        .iter()
        .map(|item| {
            by_id.remove(&item.id).ok_or_else(|| {
                format!(
                    "{} is no longer in Trash",
                    file_name_of(&item.original_path)
                )
            })
        })
        .collect()
}

fn restore_trash_refs(
    items: &[TrashRef],
    destination: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    let _guard = trash_lock();
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
            let original_name = item_ref
                .original_path
                .file_name()
                .ok_or_else(|| "Invalid original Trash path".to_string())?;
            item.original_parent = item_ref
                .original_path
                .parent()
                .ok_or_else(|| "Invalid original Trash path".to_string())?
                .to_path_buf();
            item.name = original_name.to_os_string();
            item_ref.original_path.clone()
        };
        restored_paths.push(path);
        wanted.push(item);
    }
    trash::os_limited::restore_all(wanted).map_err(trash_error)?;
    Ok(restored_paths)
}

#[cfg(test)]
fn restore_deleted_to(items: &[TrashRef], destination: &Path) -> Result<Vec<PathBuf>, String> {
    restore_trash_refs(items, Some(destination))
}

pub fn restore_from_trash(items: &[TrashRef]) -> Result<usize, String> {
    let _guard = trash_lock();
    let wanted = exact_trash_items(items)?;
    let n = wanted.len();
    trash::os_limited::restore_all(wanted).map_err(trash_error)?;
    Ok(n)
}

pub fn purge_from_trash(items: &[TrashRef]) -> Result<usize, String> {
    let _guard = trash_lock();
    let wanted = exact_trash_items(items)?;
    let n = wanted.len();
    trash::os_limited::purge_all(wanted).map_err(trash_error)?;
    Ok(n)
}

pub fn empty_trash() -> Result<usize, String> {
    let _guard = trash_lock();
    let items = trash::os_limited::list().map_err(trash_error)?;
    let n = items.len();
    trash::os_limited::purge_all(items).map_err(trash_error)?;
    Ok(n)
}

// ---------------------------------------------------------------------------
// Create / rename
// ---------------------------------------------------------------------------

fn validated_name(name: &str) -> Result<&std::ffi::OsStr, String> {
    if name.trim().is_empty() {
        return Err("Name cannot be empty".into());
    }
    let path = Path::new(name);
    let mut components = path.components();
    let Some(std::path::Component::Normal(component)) = components.next() else {
        return Err("Name must be a single file name".into());
    };
    if components.next().is_some() || path.as_os_str() != component {
        return Err("Name must be a single file name".into());
    }
    Ok(component)
}

pub fn new_folder(dir: &Path, name: &str) -> Result<UndoOp, String> {
    let p = dir.join(validated_name(name)?);
    fs::create_dir(&p).map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            format!("{name} already exists")
        } else {
            error.to_string()
        }
    })?;
    Ok(UndoOp::Create { path: p })
}

pub fn new_file(dir: &Path, name: &str) -> Result<UndoOp, String> {
    let p = dir.join(validated_name(name)?);
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!("{name} already exists")
            } else {
                error.to_string()
            }
        })?;
    Ok(UndoOp::Create { path: p })
}

pub fn rename(from: &Path, new_name: &str) -> Result<UndoOp, String> {
    validated_name(new_name)?;
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
    let hash_count = pattern
        .chars()
        .filter(|character| *character == '#')
        .count();
    let mut planned = Vec::with_capacity(paths.len());
    let mut destinations = std::collections::HashSet::new();
    for (index, source) in paths.iter().enumerate() {
        let counter = format!("{:0width$}", index + 1, width = hash_count);
        let mut name = String::new();
        let mut wrote_counter = false;
        for character in pattern.chars() {
            if character == '#' {
                if !wrote_counter {
                    name.push_str(&counter);
                    wrote_counter = true;
                }
            } else {
                name.push(character);
            }
        }
        if !name.contains('.') {
            if let Some(extension) = source.extension() {
                name = format!("{name}.{}", extension.to_string_lossy());
            }
        }
        validated_name(&name)?;
        let destination = source.with_file_name(&name);
        if !destinations.insert(destination.clone()) {
            return Err(format!("Batch pattern produces duplicate name {name}"));
        }
        if destination.exists() && destination != *source {
            return Err(format!("{name} already exists"));
        }
        planned.push((source.clone(), destination));
    }

    let mut committed: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (source, destination) in &planned {
        if source == destination {
            continue;
        }
        if let Err(error) = fs::rename(source, destination) {
            for (from, to) in committed.iter().rev() {
                let _ = fs::rename(to, from);
            }
            return Err(format!("{}: {error}", file_name_of(source)));
        }
        committed.push((source.clone(), destination.clone()));
    }
    Ok(UndoOp::Move {
        moved_pairs: committed,
    })
}

// ---------------------------------------------------------------------------
// Copy / move with progress
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct TransferEffect {
    pub source: PathBuf,
    pub target: PathBuf,
    pub trash_ref: Option<TrashRef>,
}

#[derive(Clone, Debug, Default)]
pub struct TransferOutcome {
    pub committed: Vec<TransferEffect>,
    pub failed: Vec<ItemFailure>,
    pub cancelled: bool,
}

pub struct Progress {
    pub kind: TransferKind,
    pub label: String,
    pub total_bytes: Arc<AtomicU64>,
    pub copied_bytes: Arc<AtomicU64>,
    pub current_file: Arc<Mutex<String>>,
    pub cancel_requested: Arc<AtomicBool>,
    pub finished: Arc<AtomicBool>,
    pub outcome: Arc<Mutex<Option<TransferOutcome>>>,
    /// Register state this paste was derived from. Drag transfers leave it None.
    pub expected_register: Option<UnnamedRegister>,
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
    Restore,
}

/// Start a copy or move in the background. Returns immediately.
pub fn start_transfer(sources: Vec<PathBuf>, dest: PathBuf, kind: TransferKind) -> Progress {
    debug_assert!(kind != TransferKind::Restore);
    let transfer_progress = new_progress(
        kind,
        format!(
            "{} {} item(s) to {}",
            if kind == TransferKind::Move {
                "Moving"
            } else {
                "Copying"
            },
            sources.len(),
            file_name_of(&dest)
        ),
    );
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
            sources.iter().map(|source| tree_size(source)).sum(),
            Ordering::Relaxed,
        );
        let mut result = TransferOutcome::default();
        for source in &sources {
            if cancel_requested.load(Ordering::Relaxed) {
                result.cancelled = true;
                break;
            }
            let target = match unique_target(&dest.join(file_name_of(source))) {
                Ok(target) => target,
                Err(error) => {
                    result.failed.push(ItemFailure {
                        path: source.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            if let Err(error) = copy_tree(
                source,
                &target,
                &copied_bytes,
                &current_file,
                &cancel_requested,
            ) {
                let _ = remove_tree(&target);
                result.failed.push(ItemFailure {
                    path: source.clone(),
                    message: error.to_string(),
                });
                continue;
            }
            if cancel_requested.load(Ordering::Relaxed) {
                let _ = remove_tree(&target);
                result.cancelled = true;
                break;
            }
            if kind == TransferKind::Move {
                if let Err(error) = remove_tree(source) {
                    let _ = remove_tree(&target);
                    result.failed.push(ItemFailure {
                        path: source.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            }
            result.committed.push(TransferEffect {
                source: source.clone(),
                target,
                trash_ref: None,
            });
        }
        if let Ok(mut guard) = outcome.lock() {
            *guard = Some(result);
        }
        finished.store(true, Ordering::Relaxed);
    });
    transfer_progress
}

pub fn start_restore(items: Vec<TrashRef>, destination: PathBuf) -> Progress {
    let mut progress = new_progress(
        TransferKind::Restore,
        format!(
            "Restoring {} item(s) to {}",
            items.len(),
            file_name_of(&destination)
        ),
    );
    progress.expected_register = Some(UnnamedRegister::Deleted {
        items: items.clone(),
    });
    let (total, copied, current, cancel, finished, outcome) = (
        Arc::clone(&progress.total_bytes),
        Arc::clone(&progress.copied_bytes),
        Arc::clone(&progress.current_file),
        Arc::clone(&progress.cancel_requested),
        Arc::clone(&progress.finished),
        Arc::clone(&progress.outcome),
    );
    thread::spawn(move || {
        total.store(items.len() as u64, Ordering::Relaxed);
        let mut result = TransferOutcome::default();
        for item in items {
            if cancel.load(Ordering::Relaxed) {
                result.cancelled = true;
                break;
            }
            if let Ok(mut name) = current.lock() {
                *name = item.name.to_string_lossy().into_owned();
            }
            let target = match unique_target(&destination.join(&item.name)) {
                Ok(target) => target,
                Err(error) => {
                    result.failed.push(ItemFailure {
                        path: item.original_path.clone(),
                        message: error.to_string(),
                    });
                    continue;
                }
            };
            match restore_one_to(&item, &target) {
                Ok(()) => {
                    copied.fetch_add(1, Ordering::Relaxed);
                    result.committed.push(TransferEffect {
                        source: item.original_path.clone(),
                        target,
                        trash_ref: Some(item),
                    });
                }
                Err(message) => result.failed.push(ItemFailure {
                    path: item.original_path.clone(),
                    message,
                }),
            }
        }
        if let Ok(mut guard) = outcome.lock() {
            *guard = Some(result);
        }
        finished.store(true, Ordering::Relaxed);
    });
    progress
}

fn new_progress(kind: TransferKind, label: String) -> Progress {
    Progress {
        kind,
        label,
        total_bytes: Arc::new(AtomicU64::new(0)),
        copied_bytes: Arc::new(AtomicU64::new(0)),
        current_file: Arc::new(Mutex::new(String::new())),
        cancel_requested: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
        outcome: Arc::new(Mutex::new(None)),
        expected_register: None,
    }
}

fn restore_one_to(item: &TrashRef, target: &Path) -> Result<(), String> {
    let _guard = trash_lock();
    let mut exact = exact_trash_items(std::slice::from_ref(item))?
        .pop()
        .ok_or_else(|| "No matching item in Trash".to_string())?;
    exact.original_parent = target.parent().unwrap_or(Path::new("/")).to_path_buf();
    exact.name = target
        .file_name()
        .ok_or_else(|| "Invalid restore destination".to_string())?
        .to_os_string();
    trash::os_limited::restore_all(vec![exact]).map_err(trash_error)
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
fn unique_target(path: &Path) -> io::Result<PathBuf> {
    if !path.exists() {
        return Ok(path.to_path_buf());
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
            return Ok(candidate);
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "No collision-free destination name is available",
    ))
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
        fs::create_dir(dest)?;
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
    let mut dest_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
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

#[derive(Clone, Debug, Default)]
pub struct DeleteOutcome {
    pub committed: Vec<PathBuf>,
    pub failed: Vec<ItemFailure>,
}

pub fn delete_permanently(paths: &[PathBuf]) -> DeleteOutcome {
    let mut outcome = DeleteOutcome::default();
    for path in paths {
        match remove_tree(path) {
            Ok(()) => outcome.committed.push(path.clone()),
            Err(error) => outcome.failed.push(ItemFailure {
                path: path.clone(),
                message: format!("{}: {error}", file_name_of(path)),
            }),
        }
    }
    outcome
}

// ---------------------------------------------------------------------------
// Archive extraction — shelled out and presence-checked
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
    fn creation_accepts_only_one_safe_name_and_never_clobbers() {
        let temp_dir = tmpdir("create-name");
        for invalid in ["", "   ", ".", "..", "a/b", "/absolute"] {
            assert!(
                new_file(&temp_dir, invalid).is_err(),
                "accepted {invalid:?}"
            );
            assert!(
                new_folder(&temp_dir, invalid).is_err(),
                "accepted {invalid:?}"
            );
        }
        fs::write(temp_dir.join("existing"), b"keep").unwrap();
        assert!(new_file(&temp_dir, "existing").is_err());
        assert_eq!(fs::read(temp_dir.join("existing")).unwrap(), b"keep");
        fs::remove_dir_all(&temp_dir).unwrap();
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
        assert_eq!(
            unique_target(&file_path).unwrap(),
            temp_dir.join("f (1).txt")
        );
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
    fn deleted_restore_uses_transfer_collision_policy() {
        let source_dir = tmpdir("restore-transfer-source");
        let destination_dir = tmpdir("restore-transfer-destination");
        let original = source_dir.join("note.txt");
        fs::write(&original, b"restored").unwrap();
        fs::write(destination_dir.join("note.txt"), b"existing").unwrap();
        let deleted = trash(std::slice::from_ref(&original));
        assert!(deleted.failed.is_empty());

        let progress = start_restore(deleted.committed, destination_dir.clone());
        while !progress.finished.load(Ordering::Relaxed) {
            std::thread::yield_now();
        }
        let outcome = progress.outcome.lock().unwrap().take().unwrap();
        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.committed.len(), 1);
        assert_eq!(
            fs::read(destination_dir.join("note.txt")).unwrap(),
            b"existing"
        );
        assert_eq!(
            fs::read(destination_dir.join("note (1).txt")).unwrap(),
            b"restored"
        );
        fs::remove_dir_all(source_dir).unwrap();
        fs::remove_dir_all(destination_dir).unwrap();
    }

    #[test]
    fn permanent_delete_reports_partial_effects() {
        let temp_dir = tmpdir("partial-delete");
        let existing = temp_dir.join("existing");
        let missing = temp_dir.join("missing");
        fs::write(&existing, b"data").unwrap();
        let outcome = delete_permanently(&[existing.clone(), missing.clone()]);
        assert_eq!(outcome.committed, vec![existing]);
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].path, missing);
        fs::remove_dir_all(temp_dir).unwrap();
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
        let mut register = UnnamedRegister::default();
        register.set_deleted(vec![item.clone()]);
        assert_eq!(register, UnnamedRegister::Deleted { items: vec![item] });

        register.set(vec![path.clone()], false);
        assert_eq!(
            register,
            UnnamedRegister::Live {
                paths: vec![path.clone()],
                cut: false
            }
        );
        register.set(vec![path.clone()], true);
        assert_eq!(
            register,
            UnnamedRegister::Live {
                paths: vec![path.clone()],
                cut: true
            }
        );
        register.set_deleted(vec![TrashRef {
            id: "new-generation".into(),
            original_path: path,
            name: "item".into(),
        }]);
        assert!(matches!(register, UnnamedRegister::Deleted { .. }));
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
