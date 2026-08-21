//! File operations: copy, move, trash, delete, rename, clipboard, undo.
//!
//! Long operations run on a worker thread behind a `Progress` handle so the
//! UI stays live and cancellable, exactly like Dolphin's progress popup.

use std::cell::Cell;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use crate::config;
use crate::fs::{Entry, Kind, TrashIdentity};

/// Remove duplicate operands and descendants already covered by an earlier
/// ancestor. Recursive filesystem operations must act on each tree only once.
pub fn normalize_operands(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut normalized = Vec::with_capacity(paths.len());
    for path in paths {
        if normalized
            .iter()
            .any(|ancestor: &PathBuf| path.starts_with(ancestor))
        {
            continue;
        }
        normalized.retain(|descendant| !descendant.starts_with(&path));
        normalized.push(path);
    }
    normalized
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

impl TrashRef {
    pub fn selection_key(&self) -> PathBuf {
        crate::fs::trash_selection_key(&self.id)
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
    /// Durable cleanup created by a failed undo attempt. Identity is retained
    /// across retries so a concurrent replacement is never removed.
    RetryCleanup {
        parents: Vec<ParentCleanupEffect>,
        operation: Option<Box<UndoOp>>,
    },
    /// A no-replace destination was committed but source unlink failed. The
    /// effect is durable and intentionally not auto-deleted without an atomic
    /// identity-bound unlink primitive.
    UnresolvedRename {
        effects: Vec<RenamePartial>,
        operation: Option<Box<UndoOp>>,
    },
}

#[derive(Clone, Debug)]
pub struct RenamePartial {
    pub source: PathBuf,
    pub target: PathBuf,
    pub message: String,
}

pub fn unresolved_rename_paths(operation: &UndoOp) -> Vec<PathBuf> {
    match operation {
        UndoOp::UnresolvedRename { effects, operation } => effects
            .iter()
            .flat_map(|effect| [effect.source.clone(), effect.target.clone()])
            .chain(
                operation
                    .as_deref()
                    .into_iter()
                    .flat_map(unresolved_rename_paths),
            )
            .collect(),
        UndoOp::RetryCleanup { operation, .. } => operation
            .as_deref()
            .map(unresolved_rename_paths)
            .unwrap_or_default(),
        _ => Vec::new(),
    }
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

#[derive(Debug)]
pub struct OperationFailure {
    pub message: String,
    /// The exact effects that still need undoing after any rollback attempt.
    pub remaining: Option<UndoOp>,
}

impl OperationFailure {
    fn unchanged(message: String, op: &UndoOp) -> Self {
        Self {
            message,
            remaining: Some(op.clone()),
        }
    }

    fn without_effects(message: String) -> Self {
        Self {
            message,
            remaining: None,
        }
    }
}

impl std::fmt::Display for OperationFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl From<String> for OperationFailure {
    fn from(message: String) -> Self {
        Self::without_effects(message)
    }
}

impl From<&str> for OperationFailure {
    fn from(message: &str) -> Self {
        Self::without_effects(message.into())
    }
}

#[derive(Clone, Debug)]
pub struct ParentCleanupEffect {
    pub path: PathBuf,
    device: u64,
    inode: u64,
}

#[cfg(test)]
pub(crate) fn test_create_missing_parents(parent: &Path) -> Vec<ParentCleanupEffect> {
    create_missing_parents(parent).expect("test parent creation failed")
}

fn create_missing_parents(
    parent: &Path,
) -> Result<Vec<ParentCleanupEffect>, (io::Error, Vec<ParentCleanupEffect>)> {
    if parent.as_os_str().is_empty() {
        return Ok(Vec::new());
    }
    let mut missing = Vec::new();
    let mut current = Some(parent);
    while let Some(path) = current {
        if path.exists() {
            break;
        }
        missing.push(path.to_path_buf());
        current = path.parent();
    }
    missing.reverse();

    let mut created = Vec::new();
    for path in missing {
        match fs::create_dir(&path) {
            Ok(()) => match fs::symlink_metadata(&path) {
                Ok(metadata) => created.push(ParentCleanupEffect {
                    path,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                }),
                Err(error) => return Err((error, created)),
            },
            // A concurrent creator owns this directory, so exclude it from
            // cleanup even though it was absent during the initial inspection.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && path.is_dir() => {}
            Err(error) => return Err((error, created)),
        }
    }
    Ok(created)
}

struct ParentCleanupResult {
    errors: Vec<String>,
    remaining: Vec<ParentCleanupEffect>,
}

fn cleanup_created_parents(created: &[ParentCleanupEffect]) -> ParentCleanupResult {
    let mut errors = Vec::new();
    let mut remaining = Vec::new();
    for parent in created.iter().rev() {
        let path = &parent.path;
        let identity_matches = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata.dev() == parent.device && metadata.ino() == parent.inode,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => false,
        };
        let reason = if identity_matches {
            "atomic identity-bound directory cleanup is unavailable"
        } else {
            "directory identity changed"
        };
        errors.push(format!("{}: {reason}; preserved", path.display()));
        remaining.push(parent.clone());
    }
    ParentCleanupResult { errors, remaining }
}

fn rollback_undone_moves(
    moved_pairs: &[(PathBuf, PathBuf)],
    undone: &[(usize, Vec<ParentCleanupEffect>)],
    primary_error: String,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), RenameFailure>,
) -> OperationFailure {
    let mut remains_committed = vec![true; moved_pairs.len()];
    let mut rollback_errors = Vec::new();
    let mut outstanding_cleanup = Vec::new();
    let mut unresolved_renames = Vec::new();
    for (index, _) in undone {
        remains_committed[*index] = false;
    }
    for (index, created_parents) in undone.iter().rev() {
        let (from, to) = &moved_pairs[*index];
        match rename(from, to) {
            Ok(()) => {
                remains_committed[*index] = true;
                let cleanup = cleanup_created_parents(created_parents);
                rollback_errors.extend(
                    cleanup
                        .errors
                        .into_iter()
                        .map(|error| format!("parent cleanup: {error}")),
                );
                outstanding_cleanup.extend(cleanup.remaining);
            }
            Err(RenameFailure::Untouched(error)) => {
                rollback_errors.push(format!("{}: {error}", file_name_of(from)));
            }
            Err(RenameFailure::Partial(partial)) => {
                rollback_errors.push(partial.message.clone());
                unresolved_renames.push(partial);
            }
        }
    }
    let remaining: Vec<_> = moved_pairs
        .iter()
        .zip(remains_committed)
        .filter(|(_, committed)| *committed)
        .map(|(pair, _)| pair.clone())
        .collect();
    let message = if rollback_errors.is_empty() {
        primary_error
    } else {
        format!(
            "{primary_error}; rollback also failed: {}",
            rollback_errors.join("; ")
        )
    };
    let operation = (!remaining.is_empty()).then_some(UndoOp::Move {
        moved_pairs: remaining,
    });
    let mut operation = operation;
    if !outstanding_cleanup.is_empty() {
        operation = Some(UndoOp::RetryCleanup {
            parents: outstanding_cleanup,
            operation: operation.map(Box::new),
        });
    }
    if !unresolved_renames.is_empty() {
        operation = Some(UndoOp::UnresolvedRename {
            effects: unresolved_renames,
            operation: operation.map(Box::new),
        });
    }
    OperationFailure {
        message,
        remaining: operation,
    }
}

fn failure_from_rename(error: RenameFailure, unchanged: Option<&UndoOp>) -> OperationFailure {
    match error {
        RenameFailure::Untouched(error) => unchanged.map_or_else(
            || OperationFailure::without_effects(error.to_string()),
            |operation| OperationFailure::unchanged(error.to_string(), operation),
        ),
        RenameFailure::Partial(partial) => OperationFailure {
            message: partial.message.clone(),
            remaining: Some(UndoOp::UnresolvedRename {
                effects: vec![partial],
                operation: None,
            }),
        },
    }
}

fn attach_rename_partial(
    mut failure: OperationFailure,
    partial: RenamePartial,
) -> OperationFailure {
    failure.message = format!("{}; {}", failure.message, partial.message);
    failure.remaining = Some(UndoOp::UnresolvedRename {
        effects: vec![partial],
        operation: failure.remaining.take().map(Box::new),
    });
    failure
}

fn attach_parent_cleanup(
    mut failure: OperationFailure,
    cleanup: ParentCleanupResult,
) -> OperationFailure {
    if !cleanup.errors.is_empty() {
        failure.message = format!(
            "{}; parent cleanup also failed: {}",
            failure.message,
            cleanup.errors.join("; ")
        );
    }
    if !cleanup.remaining.is_empty() {
        failure.remaining = Some(UndoOp::RetryCleanup {
            parents: cleanup.remaining,
            operation: failure.remaining.take().map(Box::new),
        });
    }
    failure
}

pub fn undo(op: &UndoOp) -> Result<UndoOutcome, OperationFailure> {
    match op {
        UndoOp::Rename { from, to } => {
            rename_noreplace(to, from).map_err(|error| failure_from_rename(error, Some(op)))?;
            Ok(UndoOutcome::message(format!(
                "Renamed back to {}",
                file_name_of(from)
            )))
        }
        UndoOp::Move { moved_pairs } => {
            let mut undone = Vec::new();
            for index in (0..moved_pairs.len()).rev() {
                let (from, to) = &moved_pairs[index];
                let created_parents = if let Some(parent) = from.parent() {
                    match create_missing_parents(parent) {
                        Ok(created) => created,
                        Err((error, created)) => {
                            let cleanup = cleanup_created_parents(&created);
                            let failure = rollback_undone_moves(
                                moved_pairs,
                                &undone,
                                error.to_string(),
                                rename_noreplace,
                            );
                            return Err(attach_parent_cleanup(failure, cleanup));
                        }
                    }
                } else {
                    Vec::new()
                };
                if let Err(error) = rename_noreplace(to, from) {
                    let cleanup = cleanup_created_parents(&created_parents);
                    let (message, partial) = match error {
                        RenameFailure::Untouched(error) => (error.to_string(), None),
                        RenameFailure::Partial(partial) => (partial.message.clone(), Some(partial)),
                    };
                    let failure =
                        rollback_undone_moves(moved_pairs, &undone, message, rename_noreplace);
                    let failure = attach_parent_cleanup(failure, cleanup);
                    return Err(match partial {
                        Some(partial) => attach_rename_partial(failure, partial),
                        None => failure,
                    });
                }
                undone.push((index, created_parents));
            }
            Ok(UndoOutcome::message(format!(
                "Moved {} item(s) back",
                moved_pairs.len()
            )))
        }
        UndoOp::RetryCleanup { parents, operation } => {
            let cleanup = cleanup_created_parents(parents);
            if !cleanup.remaining.is_empty() {
                return Err(OperationFailure {
                    message: format!(
                        "Parent cleanup remains incomplete: {}",
                        cleanup.errors.join("; ")
                    ),
                    remaining: Some(UndoOp::RetryCleanup {
                        parents: cleanup.remaining,
                        operation: operation.clone(),
                    }),
                });
            }
            if let Some(operation) = operation {
                undo(operation)
            } else {
                Ok(UndoOutcome::message(
                    "Cleaned created parent directories".into(),
                ))
            }
        }
        UndoOp::UnresolvedRename { effects, operation } => {
            let operation_message = if let Some(operation) = operation {
                match undo(operation) {
                    Ok(outcome) => Some(outcome.message),
                    Err(mut failure) => {
                        failure.remaining = Some(UndoOp::UnresolvedRename {
                            effects: effects.clone(),
                            operation: failure.remaining.take().map(Box::new),
                        });
                        return Err(failure);
                    }
                }
            } else {
                None
            };
            let paths = effects
                .iter()
                .map(|effect| format!("{} -> {}", effect.source.display(), effect.target.display()))
                .collect::<Vec<_>>()
                .join(", ");
            Err(OperationFailure {
                message: format!(
                    "Unresolved partial rename retained for manual resolution: {paths}{}",
                    operation_message
                        .map(|message| format!("; completed nested undo: {message}"))
                        .unwrap_or_default()
                ),
                remaining: Some(UndoOp::UnresolvedRename {
                    effects: effects.clone(),
                    operation: None,
                }),
            })
        }
        UndoOp::Trash { items } => {
            let restored = restore_trash_refs(items, None)
                .map_err(|message| OperationFailure::unchanged(message, op))?;
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
                return Err(OperationFailure::unchanged(
                    format!(
                        "Could not move the restored paste back to Trash: {}",
                        deleted
                            .failed
                            .first()
                            .map(|failure| failure.message.as_str())
                            .or_else(|| {
                                deleted
                                    .committed_untracked
                                    .first()
                                    .map(|commit| commit.message.as_str())
                            })
                            .unwrap_or("incomplete operation")
                    ),
                    op,
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
                fs::remove_dir(path)
                    .map_err(|error| OperationFailure::unchanged(error.to_string(), op))?;
            } else {
                fs::remove_file(path)
                    .map_err(|error| OperationFailure::unchanged(error.to_string(), op))?;
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

/// The actual effects of a multi-item operation. The two committed collections
/// are authoritative; callers must update state from them, never the request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UntrackedTrashCommit {
    pub path: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct TrashOutcome {
    /// Commits whose backend identity is known and can therefore be undone.
    pub committed: Vec<TrashRef>,
    /// Successful mutations that cannot safely be added to undo history.
    pub committed_untracked: Vec<UntrackedTrashCommit>,
    /// Operands for which no mutation took place.
    pub failed: Vec<ItemFailure>,
}

impl TrashOutcome {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }

    pub fn committed_len(&self) -> usize {
        self.committed.len() + self.committed_untracked.len()
    }
}

fn trash_inventory() -> Result<Vec<TrashRef>, String> {
    trash::os_limited::list().map_err(trash_error).map(|items| {
        items
            .into_iter()
            .map(|item| TrashRef {
                original_path: item.original_parent.join(&item.name),
                id: item.id,
                name: item.name,
            })
            .collect()
    })
}

fn trash_with(
    paths: &[PathBuf],
    mut inventory: impl FnMut() -> Result<Vec<TrashRef>, String>,
    mut delete: impl FnMut(&Path) -> Result<(), String>,
) -> TrashOutcome {
    let mut outcome = TrashOutcome::default();
    for path in normalize_operands(paths.to_vec()) {
        let before = match inventory() {
            Ok(items) => items
                .into_iter()
                .map(|item| item.id)
                .collect::<std::collections::HashSet<_>>(),
            Err(message) => {
                outcome.failed.push(ItemFailure {
                    path,
                    message: format!("Could not inspect Trash before deleting: {message}"),
                });
                continue;
            }
        };
        if let Err(message) = delete(&path) {
            outcome.failed.push(ItemFailure {
                path: path.clone(),
                message,
            });
            continue;
        }
        match inventory().ok().and_then(|items| {
            items
                .into_iter()
                .find(|item| !before.contains(&item.id) && item.original_path == path)
        }) {
            Some(item) => outcome.committed.push(item),
            None => outcome.committed_untracked.push(UntrackedTrashCommit {
                path,
                message: "Moved to Trash, but its backend identity could not be read".into(),
            }),
        }
    }
    outcome
}

/// Trash each operand separately and retain the backend identity of every item
/// that actually moved. This intentionally does not flatten partial completion
/// into `Err(String)`.
pub fn trash(paths: &[PathBuf]) -> TrashOutcome {
    let _guard = trash_lock();
    trash_with(paths, trash_inventory, |path| {
        trash::delete(path).map_err(trash_error)
    })
}

/// Resolve Freedesktop's data entry from the backend's exact `.trashinfo` ID.
/// The ID, rather than the original name, preserves collision suffixes and the
/// trash can location chosen for another mount.
#[cfg(target_os = "linux")]
fn trash_backing_path(id: &std::ffi::OsStr) -> Option<PathBuf> {
    let info = Path::new(id);
    let info_dir = info.parent()?;
    if info_dir.file_name()? != "info" || info.extension()? != "trashinfo" {
        return None;
    }
    Some(info_dir.parent()?.join("files").join(info.file_stem()?))
}

#[cfg(not(target_os = "linux"))]
fn trash_backing_path(_: &std::ffi::OsStr) -> Option<PathBuf> {
    None
}

/// Trash contents as view entries, so the Trash place browses like a folder.
pub fn list_trash() -> Result<Vec<Entry>, String> {
    let items = trash::os_limited::list().map_err(trash_error)?;
    Ok(items
        .into_iter()
        .map(|trash_item| {
            let original = trash_item.original_parent.join(&trash_item.name);
            let metadata = trash::os_limited::metadata(&trash_item).ok();
            let backing_path = trash_backing_path(&trash_item.id)
                .filter(|path| fs::symlink_metadata(path).is_ok());
            let backing_metadata = backing_path
                .as_ref()
                .and_then(|path| fs::symlink_metadata(path).ok());
            let (kind, size) = match metadata.map(|metadata| metadata.size) {
                Some(trash::TrashItemSize::Entries(count)) => (Kind::Dir, count as u64),
                Some(trash::TrashItemSize::Bytes(bytes)) => (Kind::File, bytes),
                None => (Kind::File, 0),
            };
            Entry {
                name: trash_item.name.to_string_lossy().into_owned(),
                path: original,
                backing_path,
                link_target: None,
                kind,
                size,
                mtime: trash_item.time_deleted,
                mode: backing_metadata
                    .as_ref()
                    .map(std::os::unix::fs::MetadataExt::mode)
                    .unwrap_or(0),
                readable: kind != Kind::Dir || backing_metadata.is_some(),
                hidden: false,
                trash_identity: Some(TrashIdentity::new(trash_item.id)),
                depth: 0,
                expanded: false,
            }
        })
        .collect())
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
            format!("Cannot access the Trash filesystem: {source}")
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

pub fn rename(from: &Path, new_name: &str) -> Result<UndoOp, OperationFailure> {
    rename_with(from, new_name, rename_noreplace)
}

fn rename_with(
    from: &Path,
    new_name: &str,
    rename: impl FnOnce(&Path, &Path) -> Result<(), RenameFailure>,
) -> Result<UndoOp, OperationFailure> {
    validated_name(new_name)?;
    let to = from.with_file_name(new_name);
    if to == from {
        return Err("Unchanged".into());
    }
    if to.exists() {
        return Err(format!("{new_name} already exists").into());
    }
    rename(from, &to).map_err(|error| failure_from_rename(error, None))?;
    Ok(UndoOp::Rename {
        from: from.to_path_buf(),
        to,
    })
}

fn rollback_committed_moves(
    committed: &[(PathBuf, PathBuf)],
    primary_error: String,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), RenameFailure>,
) -> OperationFailure {
    let mut remaining = Vec::new();
    let mut rollback_errors = Vec::new();
    let mut unresolved = Vec::new();
    for (from, to) in committed.iter().rev() {
        if let Err(error) = rename(to, from) {
            match error {
                RenameFailure::Untouched(error) => {
                    remaining.push((from.clone(), to.clone()));
                    rollback_errors.push(format!("{}: {error}", file_name_of(to)));
                }
                RenameFailure::Partial(partial) => {
                    rollback_errors.push(partial.message.clone());
                    unresolved.push(partial);
                }
            }
        }
    }
    remaining.reverse();
    let message = if rollback_errors.is_empty() {
        primary_error
    } else {
        format!(
            "{primary_error}; rollback also failed: {}",
            rollback_errors.join("; ")
        )
    };
    let operation = (!remaining.is_empty()).then_some(UndoOp::Move {
        moved_pairs: remaining,
    });
    let remaining = if unresolved.is_empty() {
        operation
    } else {
        Some(UndoOp::UnresolvedRename {
            effects: unresolved,
            operation: operation.map(Box::new),
        })
    };
    OperationFailure { message, remaining }
}

/// Dolphin's batch rename: `#` in the pattern expands to a zero-padded index,
/// widened to the number of `#`s. `Holiday #.jpg` → `Holiday 1.jpg`.
pub fn batch_rename(paths: &[PathBuf], pattern: &str) -> Result<UndoOp, OperationFailure> {
    batch_rename_with(paths, pattern, rename_noreplace)
}

fn batch_rename_with(
    paths: &[PathBuf],
    pattern: &str,
    mut rename: impl FnMut(&Path, &Path) -> Result<(), RenameFailure>,
) -> Result<UndoOp, OperationFailure> {
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
            return Err(format!("Batch pattern produces duplicate name {name}").into());
        }
        if destination.exists() && destination != *source {
            return Err(format!("{name} already exists").into());
        }
        planned.push((source.clone(), destination));
    }

    let mut committed: Vec<(PathBuf, PathBuf)> = Vec::new();
    for (source, destination) in &planned {
        if source == destination {
            continue;
        }
        if let Err(error) = rename(source, destination) {
            let (message, partial) = match error {
                RenameFailure::Untouched(error) => {
                    (format!("{}: {error}", file_name_of(source)), None)
                }
                RenameFailure::Partial(partial) => (partial.message.clone(), Some(partial)),
            };
            let failure = rollback_committed_moves(&committed, message, &mut rename);
            return Err(match partial {
                Some(partial) => attach_rename_partial(failure, partial),
                None => failure,
            });
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
    /// True only when the operation actually consumed the source identity.
    pub source_removed: bool,
}

#[derive(Clone, Debug)]
pub struct RetainedOutput {
    pub source: PathBuf,
    pub target: PathBuf,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct TransferOutcome {
    pub committed: Vec<TransferEffect>,
    pub failed: Vec<ItemFailure>,
    /// Destination output that could not safely be cleaned up or whose source
    /// could not be removed after the destination committed.
    pub retained_output: Vec<RetainedOutput>,
    pub cleanup_failed: Vec<ItemFailure>,
    pub cancelled: bool,
}

#[derive(Debug)]
pub enum TransferCompletion {
    Completed(TransferOutcome),
    Panicked {
        outcome: TransferOutcome,
        message: String,
    },
}

#[cfg(test)]
impl TransferCompletion {
    pub(crate) fn expect_completed(self) -> TransferOutcome {
        match self {
            Self::Completed(outcome) => outcome,
            Self::Panicked { message, .. } => panic!("transfer panicked: {message}"),
        }
    }
}

thread_local! {
    static TRANSFER_WORKER: Cell<bool> = const { Cell::new(false) };
}

/// Panic hooks run before `catch_unwind`. The process hook uses this marker to
/// leave terminal restoration and panic reporting to the main-thread reducer.
pub(crate) fn transfer_worker_handles_panic() -> bool {
    TRANSFER_WORKER.get()
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic payload".into())
}

fn spawn_transfer_worker(
    work: impl FnOnce(&mut TransferOutcome) + Send + 'static,
) -> thread::JoinHandle<TransferCompletion> {
    thread::spawn(move || {
        let mut outcome = TransferOutcome::default();
        TRANSFER_WORKER.set(true);
        let result = panic::catch_unwind(AssertUnwindSafe(|| work(&mut outcome)));
        TRANSFER_WORKER.set(false);
        match result {
            Ok(()) => TransferCompletion::Completed(outcome),
            Err(payload) => TransferCompletion::Panicked {
                outcome,
                message: panic_message(payload),
            },
        }
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransferProgress {
    /// Recursive copy discovers entries as it goes, so no honest percentage
    /// exists. Only fully copied entries are counted.
    Discovering {
        completed_entries: u64,
        current_entry: String,
    },
    /// Item progress is determinate because the complete list is already known.
    Items {
        completed_items: u64,
        total_items: u64,
        current_entry: String,
    },
    Cleanup {
        retained_outputs: usize,
        cleanup_failures: usize,
        current_entry: String,
    },
}

struct TransferState {
    /// One lock makes the counter and current entry a coherent UI snapshot.
    progress: Mutex<TransferProgress>,
    cancel_requested: AtomicBool,
}

impl TransferState {
    fn new() -> Self {
        Self {
            progress: Mutex::new(TransferProgress::Discovering {
                completed_entries: 0,
                current_entry: String::new(),
            }),
            cancel_requested: AtomicBool::new(false),
        }
    }

    fn snapshot(&self) -> TransferProgress {
        self.progress.lock().map_or_else(
            |_| TransferProgress::Discovering {
                completed_entries: 0,
                current_entry: String::new(),
            },
            |progress| progress.clone(),
        )
    }

    fn set_current_entry(&self, path: &Path) {
        if let Ok(mut progress) = self.progress.lock() {
            let current_entry = match &mut *progress {
                TransferProgress::Discovering { current_entry, .. }
                | TransferProgress::Items { current_entry, .. }
                | TransferProgress::Cleanup { current_entry, .. } => current_entry,
            };
            *current_entry = file_name_of(path);
        }
    }

    fn complete_entry(&self) {
        if let Ok(mut progress) = self.progress.lock() {
            if let TransferProgress::Discovering {
                completed_entries, ..
            } = &mut *progress
            {
                *completed_entries += 1;
            }
        }
    }

    fn report_cleanup(&self, outcome: &TransferOutcome, path: &Path) {
        if let Ok(mut progress) = self.progress.lock() {
            *progress = TransferProgress::Cleanup {
                retained_outputs: outcome.retained_output.len(),
                cleanup_failures: outcome.cleanup_failed.len(),
                current_entry: file_name_of(path),
            };
        }
    }

    fn begin_items(&self, total_items: u64) {
        if let Ok(mut progress) = self.progress.lock() {
            *progress = TransferProgress::Items {
                completed_items: 0,
                total_items,
                current_entry: String::new(),
            };
        }
    }

    fn complete_item(&self) {
        if let Ok(mut progress) = self.progress.lock() {
            if let TransferProgress::Items {
                completed_items, ..
            } = &mut *progress
            {
                *completed_items += 1;
            }
        }
    }
}

pub struct Progress {
    pub kind: TransferKind,
    pub label: String,
    state: Arc<TransferState>,
    worker: Option<thread::JoinHandle<TransferCompletion>>,
    /// Register state this paste was derived from. Drag transfers leave it None.
    pub expected_register: Option<UnnamedRegister>,
    affected_paths: Vec<PathBuf>,
}

impl Progress {
    pub fn snapshot(&self) -> TransferProgress {
        self.state.snapshot()
    }

    pub fn cancel(&self) {
        self.state.cancel_requested.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelling(&self) -> bool {
        self.state.cancel_requested.load(Ordering::Relaxed)
    }

    pub fn affected_paths(&self) -> &[PathBuf] {
        &self.affected_paths
    }

    pub fn is_finished(&self) -> bool {
        self.worker
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    pub(crate) fn join(&mut self) -> Result<TransferCompletion, String> {
        self.worker
            .take()
            .ok_or_else(|| "Transfer worker was already joined".to_string())?
            .join()
            .map_err(|_| "Transfer worker escaped panic containment".to_string())
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        self.cancel();
        // A filesystem call may remain blocked after cooperative cancellation.
        // Reap workers that have already stopped, but detach unfinished ones so
        // application teardown can never wait on storage indefinitely.
        if worker.is_finished() {
            let _ = worker.join();
        }
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
fn try_rename_move(
    cross_device: bool,
    source: &Path,
    target: &Path,
    rename: impl FnOnce(&Path, &Path) -> Result<(), RenameFailure>,
) -> Result<bool, RenameFailure> {
    if cross_device {
        return Ok(false);
    }
    match rename(source, target) {
        Ok(()) => Ok(true),
        Err(RenameFailure::Untouched(error))
            if matches!(
                error.kind(),
                io::ErrorKind::CrossesDevices | io::ErrorKind::Unsupported
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

fn reduce_move_rename(
    outcome: &mut TransferOutcome,
    source: &Path,
    target: &Path,
    attempt: Result<bool, RenameFailure>,
) -> bool {
    match attempt {
        Ok(true) => {
            outcome.committed.push(TransferEffect {
                source: source.to_path_buf(),
                target: target.to_path_buf(),
                trash_ref: None,
                source_removed: true,
            });
            true
        }
        Ok(false) => false,
        Err(RenameFailure::Untouched(error)) => {
            outcome.failed.push(ItemFailure {
                path: source.to_path_buf(),
                message: error.to_string(),
            });
            true
        }
        Err(RenameFailure::Partial(partial)) => {
            outcome.committed.push(TransferEffect {
                source: partial.source.clone(),
                target: partial.target.clone(),
                trash_ref: None,
                source_removed: false,
            });
            outcome.retained_output.push(RetainedOutput {
                source: partial.source,
                target: partial.target,
                message: partial.message,
            });
            true
        }
    }
}

pub fn start_transfer(sources: Vec<PathBuf>, dest: PathBuf, kind: TransferKind) -> Progress {
    debug_assert!(kind != TransferKind::Restore);
    let sources = normalize_operands(sources);
    let mut transfer_progress = new_progress(
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
    transfer_progress.affected_paths = sources
        .iter()
        .cloned()
        .chain(std::iter::once(dest.clone()))
        .collect();
    let state = Arc::clone(&transfer_progress.state);
    transfer_progress.worker = Some(spawn_transfer_worker(move |result| {
        for source in &sources {
            state.set_current_entry(source);
            if state.cancel_requested.load(Ordering::Relaxed) {
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
            if kind == TransferKind::Move {
                // Only attempt the no-replace primitive on one filesystem.
                // Cross-device moves use the manifest-backed copy fallback.
                let cross_device = fs::symlink_metadata(source)
                    .and_then(|source_meta| {
                        fs::metadata(&dest).map(|dest_meta| source_meta.dev() != dest_meta.dev())
                    })
                    .unwrap_or(false);
                let committed_before = result.committed.len();
                let retained_before = result.retained_output.len();
                if reduce_move_rename(
                    result,
                    source,
                    &target,
                    try_rename_move(cross_device, source, &target, rename_noreplace),
                ) {
                    if result.committed.len() > committed_before
                        && result.retained_output.len() == retained_before
                    {
                        state.complete_entry();
                    } else if result.retained_output.len() > retained_before {
                        state.report_cleanup(result, &target);
                    }
                    continue;
                }
            }
            let mut manifest = Vec::new();
            if let Err(error) = copy_tree(source, &target, &state, &mut manifest) {
                let (cleanup_failed, retained) = cleanup_created_nodes(&manifest);
                result.cleanup_failed.extend(cleanup_failed);
                if !retained.is_empty() {
                    result.retained_output.push(RetainedOutput {
                        source: source.clone(),
                        target: target.clone(),
                        message: format!(
                            "copy failed ({error}); {} created node(s) retained",
                            retained.len()
                        ),
                    });
                }
                result.failed.push(ItemFailure {
                    path: source.clone(),
                    message: error.to_string(),
                });
                state.report_cleanup(result, &target);
                continue;
            }
            if state.cancel_requested.load(Ordering::Relaxed) {
                let (cleanup_failed, retained) = cleanup_created_nodes(&manifest);
                result.cleanup_failed.extend(cleanup_failed);
                if !retained.is_empty() {
                    result.retained_output.push(RetainedOutput {
                        source: source.clone(),
                        target: target.clone(),
                        message: format!("cancelled; {} created node(s) retained", retained.len()),
                    });
                }
                result.cancelled = true;
                state.report_cleanup(result, &target);
                break;
            }
            let mut source_removed = false;
            if kind == TransferKind::Move {
                source_removed = record_source_cleanup(result, source, &target);
                if !source_removed {
                    state.report_cleanup(result, &target);
                }
            }
            result.committed.push(TransferEffect {
                source: source.clone(),
                target,
                trash_ref: None,
                source_removed,
            });
        }
    }));
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
    progress.affected_paths = items
        .iter()
        .map(|item| item.original_path.clone())
        .chain(std::iter::once(destination.clone()))
        .collect();
    progress.state.begin_items(items.len() as u64);
    let state = Arc::clone(&progress.state);
    progress.worker = Some(spawn_transfer_worker(move |result| {
        for item in items {
            if state.cancel_requested.load(Ordering::Relaxed) {
                result.cancelled = true;
                break;
            }
            state.set_current_entry(Path::new(&item.name));
            let target = match unique_target(&destination.join(&item.name)) {
                Ok(target) => target,
                Err(error) => {
                    result.failed.push(ItemFailure {
                        path: item.original_path.clone(),
                        message: error.to_string(),
                    });
                    // Failed items are still processed items; cancellation does
                    // not advance entries that were never attempted.
                    state.complete_item();
                    continue;
                }
            };
            match restore_one_to(&item, &target) {
                Ok(()) => result.committed.push(TransferEffect {
                    source: item.original_path.clone(),
                    target,
                    trash_ref: Some(item),
                    source_removed: true,
                }),
                Err(message) => result.failed.push(ItemFailure {
                    path: item.original_path.clone(),
                    message,
                }),
            }
            state.complete_item();
        }
    }));
    progress
}

fn new_progress(kind: TransferKind, label: String) -> Progress {
    Progress {
        kind,
        label,
        state: Arc::new(TransferState::new()),
        worker: None,
        expected_register: None,
        affected_paths: Vec::new(),
    }
}

#[cfg(test)]
pub(crate) fn completed_progress(kind: TransferKind, outcome: TransferOutcome) -> Progress {
    let mut progress = new_progress(kind, "Test transfer".into());
    progress.worker = Some(spawn_transfer_worker(move |completed| *completed = outcome));
    progress
}

#[cfg(test)]
pub(crate) fn panicking_progress_after_effect(
    kind: TransferKind,
    effect: TransferEffect,
) -> Progress {
    let mut progress = new_progress(kind, "Test transfer".into());
    progress.worker = Some(spawn_transfer_worker(move |outcome| {
        outcome.committed.push(effect);
        panic!("panic after effect");
    }));
    progress
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

#[derive(Debug)]
enum RenameFailure {
    /// No destination was created and the source is untouched.
    Untouched(io::Error),
    /// Destination committed atomically, but source unlink failed.
    Partial(RenamePartial),
}

#[cfg(test)]
impl RenameFailure {
    fn kind(&self) -> io::ErrorKind {
        match self {
            Self::Untouched(error) => error.kind(),
            Self::Partial(_) => io::ErrorKind::Other,
        }
    }
}

impl std::fmt::Display for RenameFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Untouched(error) => error.fmt(formatter),
            Self::Partial(partial) => formatter.write_str(&partial.message),
        }
    }
}

fn rename_noreplace_with(
    from: &Path,
    to: &Path,
    link: impl FnOnce(&Path, &Path) -> io::Result<()>,
    unlink: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<(), RenameFailure> {
    let metadata = fs::symlink_metadata(from).map_err(RenameFailure::Untouched)?;
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        return Err(RenameFailure::Untouched(io::Error::new(
            io::ErrorKind::Unsupported,
            "atomic no-replace directory rename is unsupported on this platform",
        )));
    }
    link(from, to).map_err(RenameFailure::Untouched)?;
    unlink(from).map_err(|error| {
        RenameFailure::Partial(RenamePartial {
            source: from.to_path_buf(),
            target: to.to_path_buf(),
            message: format!(
                "destination committed at {} but source unlink failed: {error}",
                to.display()
            ),
        })
    })
}

fn rename_noreplace(from: &Path, to: &Path) -> Result<(), RenameFailure> {
    rename_noreplace_with(
        from,
        to,
        |source, target| fs::hard_link(source, target),
        |source| fs::remove_file(source),
    )
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreatedKind {
    Directory,
    Other,
}

#[derive(Clone, Debug)]
struct CreatedNode {
    path: PathBuf,
    kind: CreatedKind,
}

/// Record immediately after an exclusive creation call. A post-create stat is
/// intentionally forbidden: a published path may be substituted before stat.
fn record_created(path: &Path, kind: CreatedKind, manifest: &mut Vec<CreatedNode>) {
    manifest.push(CreatedNode {
        path: path.to_path_buf(),
        kind,
    });
}

/// Safe `std` has no atomic unlink-if-inode. Failure/cancellation rollback is
/// therefore deliberately non-destructive: report every created path in reverse
/// creation order and preserve it. This removes both create-to-stat and
/// stat-to-remove races and can never unlink foreign data.
fn cleanup_created_nodes(manifest: &[CreatedNode]) -> (Vec<ItemFailure>, Vec<PathBuf>) {
    let mut failures = Vec::with_capacity(manifest.len());
    let mut retained = Vec::with_capacity(manifest.len());
    for node in manifest.iter().rev() {
        let kind = match node.kind {
            CreatedKind::Directory => "directory",
            CreatedKind::Other => "file or symlink",
        };
        failures.push(ItemFailure {
            path: node.path.clone(),
            message: format!(
                "created {kind} preserved: atomic identity-bound cleanup is unavailable"
            ),
        });
        retained.push(node.path.clone());
    }
    (failures, retained)
}

fn copy_tree(
    src: &Path,
    dest: &Path,
    state: &TransferState,
    manifest: &mut Vec<CreatedNode>,
) -> io::Result<()> {
    if state.cancel_requested.load(Ordering::Relaxed) {
        return Ok(());
    }
    // Every entry type is observable, including directories and symlinks.
    state.set_current_entry(src);
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dest)?;
        record_created(dest, CreatedKind::Other, manifest);
        state.complete_entry();
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir(dest)?;
        record_created(dest, CreatedKind::Directory, manifest);
        for dir_entry in fs::read_dir(src)? {
            let dir_entry = dir_entry?;
            copy_tree(
                &dir_entry.path(),
                &dest.join(dir_entry.file_name()),
                state,
                manifest,
            )?;
            if state.cancel_requested.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
        state.complete_entry();
        return Ok(());
    }
    if copy_file_streaming(src, dest, state, manifest)? {
        state.complete_entry();
    }
    Ok(())
}

/// Copy in chunks so cancel is honoured mid-file rather than only between
/// files. The return value says whether the complete output was committed.
fn copy_file_streaming(
    src: &Path,
    dest: &Path,
    state: &TransferState,
    manifest: &mut Vec<CreatedNode>,
) -> io::Result<bool> {
    use io::{Read, Write};
    let mut source_file = fs::File::open(src)?;
    let mut dest_file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    record_created(dest, CreatedKind::Other, manifest);
    let mut buf = vec![0u8; config::COPY_CHUNK_BYTES];
    loop {
        if state.cancel_requested.load(Ordering::Relaxed) {
            // The manifest reducer owns cleanup and validates identity first.
            drop(dest_file);
            return Ok(false);
        }
        let n = source_file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        dest_file.write_all(&buf[..n])?;
    }
    if let Ok(m) = fs::metadata(src) {
        let _ = fs::set_permissions(dest, m.permissions());
    }
    Ok(true)
}

fn record_source_cleanup(outcome: &mut TransferOutcome, source: &Path, target: &Path) -> bool {
    outcome.retained_output.push(RetainedOutput {
        source: source.to_path_buf(),
        target: target.to_path_buf(),
        message: "destination committed; source preserved because atomic identity-bound recursive cleanup is unavailable".into(),
    });
    false
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
    for path in normalize_operands(paths.to_vec()) {
        match remove_tree(&path) {
            Ok(()) => outcome.committed.push(path.clone()),
            Err(error) => outcome.failed.push(ItemFailure {
                path: path.clone(),
                message: format!("{}: {error}", file_name_of(&path)),
            }),
        }
    }
    outcome
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

    #[cfg(target_os = "linux")]
    #[test]
    fn trash_backing_path_uses_the_exact_collision_suffixed_generation() {
        let info = Path::new("/mnt/.Trash-1000/info/herdr.2.trashinfo");
        assert_eq!(
            trash_backing_path(info.as_os_str()),
            Some(PathBuf::from("/mnt/.Trash-1000/files/herdr.2"))
        );
    }

    #[test]
    fn recursive_operands_keep_only_the_outermost_selected_paths() {
        let root = PathBuf::from("/tmp/tree");
        let sibling = PathBuf::from("/tmp/sibling");
        assert_eq!(
            normalize_operands(vec![
                root.join("child/grandchild"),
                sibling.clone(),
                root.join("child"),
                root.clone(),
                sibling.clone(),
            ]),
            vec![sibling, root]
        );
    }

    #[test]
    fn unavailable_pre_delete_inventory_prevents_mutation() {
        let path = PathBuf::from("/tmp/must-survive");
        let delete_calls = std::cell::Cell::new(0);
        let outcome = trash_with(
            std::slice::from_ref(&path),
            || Err("inventory unavailable".into()),
            |_| {
                delete_calls.set(delete_calls.get() + 1);
                Ok(())
            },
        );

        assert_eq!(delete_calls.get(), 0);
        assert!(outcome.committed.is_empty());
        assert!(outcome.committed_untracked.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert_eq!(outcome.failed[0].path, path);
    }

    #[test]
    fn unavailable_post_delete_inventory_is_a_committed_untracked_mutation() {
        let path = PathBuf::from("/tmp/was-removed");
        let inventory_calls = std::cell::Cell::new(0);
        let outcome = trash_with(
            std::slice::from_ref(&path),
            || {
                inventory_calls.set(inventory_calls.get() + 1);
                if inventory_calls.get() == 1 {
                    Ok(Vec::new())
                } else {
                    Err("inventory unavailable".into())
                }
            },
            |_| Ok(()),
        );

        assert!(outcome.committed.is_empty());
        assert_eq!(outcome.committed_untracked.len(), 1);
        assert_eq!(outcome.committed_untracked[0].path, path);
        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.committed_len(), 1);
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

    fn injected_partial(source: &Path, target: &Path) -> RenameFailure {
        RenameFailure::Partial(RenamePartial {
            source: source.to_path_buf(),
            target: target.to_path_buf(),
            message: "injected destination commit with retained source".into(),
        })
    }

    #[test]
    fn no_replace_rename_reports_destination_commit_when_source_unlink_fails() {
        let root = tmpdir("rename-unlink-failure");
        let source = root.join("source");
        let target = root.join("target");
        fs::write(&source, b"payload").unwrap();
        let error = rename_noreplace_with(
            &source,
            &target,
            |from, to| fs::hard_link(from, to),
            |_| {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected unlink",
                ))
            },
        )
        .unwrap_err();
        assert!(matches!(error, RenameFailure::Partial(_)));
        assert_eq!(fs::read(&source).unwrap(), b"payload");
        assert_eq!(fs::read(&target).unwrap(), b"payload");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ordinary_batch_transfer_undo_and_rollback_reducers_journal_partial_renames() {
        let source = PathBuf::from("source");
        let target = PathBuf::from("renamed-1");

        let ordinary = rename_with(&source, "renamed-1", |from, to| {
            Err(injected_partial(from, to))
        })
        .unwrap_err();
        assert!(matches!(
            ordinary.remaining,
            Some(UndoOp::UnresolvedRename { .. })
        ));

        let batch = batch_rename_with(std::slice::from_ref(&source), "renamed-#", |from, to| {
            Err(injected_partial(from, to))
        })
        .unwrap_err();
        assert!(matches!(
            batch.remaining,
            Some(UndoOp::UnresolvedRename { .. })
        ));

        let mut transfer = TransferOutcome::default();
        assert!(reduce_move_rename(
            &mut transfer,
            &source,
            &target,
            Err(injected_partial(&source, &target)),
        ));
        assert_eq!(transfer.committed.len(), 1);
        assert_eq!(transfer.retained_output.len(), 1);
        assert!(!transfer.committed[0].source_removed);

        let rollback = rollback_committed_moves(
            &[(source.clone(), target.clone())],
            "primary".into(),
            |from, to| Err(injected_partial(from, to)),
        );
        assert!(matches!(
            rollback.remaining,
            Some(UndoOp::UnresolvedRename { .. })
        ));

        let undone = vec![(0, Vec::new())];
        let undo_failure = rollback_undone_moves(
            &[(source, target)],
            &undone,
            "undo primary".into(),
            |from, to| Err(injected_partial(from, to)),
        );
        assert!(matches!(
            undo_failure.remaining,
            Some(UndoOp::UnresolvedRename { .. })
        ));
    }

    #[test]
    fn ordinary_no_replace_rename_succeeds_and_removes_the_source() {
        let temp_dir = tmpdir("rename");
        let file_path = temp_dir.join("a.txt");
        fs::write(&file_path, b"x").unwrap();

        let operation = rename(&file_path, "b.txt").unwrap();

        assert_eq!(fs::read(temp_dir.join("b.txt")).unwrap(), b"x");
        assert!(!file_path.exists());
        assert!(matches!(operation, UndoOp::Rename { .. }));
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
        let operation = batch_rename(&paths, "Holiday ##").unwrap();
        assert!(temp_dir.join("Holiday 01.jpg").exists());
        assert!(temp_dir.join("Holiday 03.jpg").exists());
        assert!(paths.iter().all(|path| !path.exists()));
        assert!(matches!(operation, UndoOp::Move { .. }));
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn batch_rename_success_is_durable_in_undo_journal() {
        let temp_dir = tmpdir("batchundo");
        let file_path = temp_dir.join("one.txt");
        fs::write(&file_path, b"x").unwrap();
        let operation = batch_rename(std::slice::from_ref(&file_path), "n#").unwrap();
        assert!(temp_dir.join("n1.txt").exists());
        assert!(!file_path.exists());

        undo(&operation).unwrap();

        assert_eq!(fs::read(&file_path).unwrap(), b"x");
        assert!(!temp_dir.join("n1.txt").exists());
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn batch_partial_never_claims_success_or_discards_journal() {
        let temp_dir = tmpdir("batch-failure");
        let existing = temp_dir.join("existing.txt");
        let missing = temp_dir.join("missing.txt");
        fs::write(&existing, b"payload").unwrap();

        let failure = batch_rename(&[existing.clone(), missing], "renamed-#").unwrap_err();

        assert_eq!(fs::read(&existing).unwrap(), b"payload");
        assert!(!temp_dir.join("renamed-1.txt").exists());
        assert!(failure.remaining.is_none());
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn failed_move_undo_preserves_and_journals_every_created_parent() {
        let temp_dir = tmpdir("undo-parent-cleanup");
        let first_from = temp_dir.join("first/deep/item");
        let first_to = temp_dir.join("moved-first");
        let failing_from = temp_dir.join("failing/deep/item");
        let missing_to = temp_dir.join("missing-moved-item");
        fs::write(&first_to, b"payload").unwrap();
        let op = UndoOp::Move {
            moved_pairs: vec![
                (failing_from.clone(), missing_to),
                (first_from.clone(), first_to.clone()),
            ],
        };

        let failure = undo(&op).unwrap_err();

        assert_eq!(fs::read(&first_to).unwrap(), b"payload");
        assert!(!first_from.exists());
        assert!(temp_dir.join("first").exists());
        assert!(temp_dir.join("failing").exists());
        assert!(matches!(
            failure.remaining,
            Some(UndoOp::RetryCleanup { .. })
        ));
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn incomplete_batch_rollback_reports_and_journals_only_committed_pairs() {
        let pairs = vec![
            (PathBuf::from("a"), PathBuf::from("renamed-a")),
            (PathBuf::from("b"), PathBuf::from("renamed-b")),
            (PathBuf::from("c"), PathBuf::from("renamed-c")),
        ];
        let failure = rollback_committed_moves(&pairs, "rename c failed".into(), |to, _| {
            if to == Path::new("renamed-b") {
                Err(RenameFailure::Untouched(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated rollback failure",
                )))
            } else {
                Ok(())
            }
        });

        assert!(failure.message.contains("rename c failed"));
        assert!(failure.message.contains("simulated rollback failure"));
        assert!(matches!(
            failure.remaining,
            Some(UndoOp::Move { moved_pairs }) if moved_pairs == vec![pairs[1].clone()]
        ));
    }

    #[test]
    fn incomplete_undo_rollback_drops_pairs_that_are_already_undone() {
        let pairs = vec![
            (PathBuf::from("a"), PathBuf::from("moved-a")),
            (PathBuf::from("b"), PathBuf::from("moved-b")),
            (PathBuf::from("c"), PathBuf::from("moved-c")),
        ];
        let undone = vec![(2, Vec::new()), (1, Vec::new())];
        let failure = rollback_undone_moves(&pairs, &undone, "undo a failed".into(), |from, _| {
            if from == Path::new("b") {
                Err(RenameFailure::Untouched(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "simulated rollback failure",
                )))
            } else {
                Ok(())
            }
        });

        assert!(failure.message.contains("undo a failed"));
        assert!(failure.message.contains("simulated rollback failure"));
        assert!(matches!(
            failure.remaining,
            Some(UndoOp::Move { moved_pairs })
                if moved_pairs == vec![pairs[0].clone(), pairs[2].clone()]
        ));
    }

    #[test]
    fn parent_cleanup_is_conservatively_preserved_and_journaled() {
        let root = tmpdir("parent-cleanup");
        let parent = root.join("parent");
        let child = parent.join("child");
        let created = create_missing_parents(&child).unwrap();
        let errors = cleanup_created_parents(&created);

        assert_eq!(errors.errors.len(), 2);
        assert_eq!(errors.remaining.len(), 2);
        assert!(errors
            .errors
            .iter()
            .all(|error| error.contains("preserved")));
        assert!(errors
            .errors
            .iter()
            .all(|error| error.contains("identity-bound")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn parent_cleanup_effect_is_durable_and_identity_safe_across_retries() {
        let root = tmpdir("durable-parent-cleanup");
        let parent = root.join("created");
        let effects = create_missing_parents(&parent).unwrap();
        fs::write(parent.join("blocker"), b"external").unwrap();
        let op = UndoOp::RetryCleanup {
            parents: effects.clone(),
            operation: None,
        };

        let first = undo(&op).unwrap_err();
        assert!(matches!(first.remaining, Some(UndoOp::RetryCleanup { .. })));
        fs::remove_file(parent.join("blocker")).unwrap();
        fs::remove_dir(&parent).unwrap();
        undo(first.remaining.as_ref().unwrap()).unwrap();
        assert!(!parent.exists());

        fs::create_dir(&parent).unwrap();
        let effects = create_missing_parents(&parent).unwrap();
        // The existing directory was not created by us, so no effect is owned.
        assert!(effects.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn already_absent_parent_cleanup_completes_and_resumes_nested_operation() {
        let root = tmpdir("absent-parent-cleanup");
        let parent = root.join("created-parent");
        let effects = create_missing_parents(&parent).unwrap();
        fs::remove_dir(&parent).unwrap();
        let created_file = root.join("nested-operation-file");
        fs::write(&created_file, b"created").unwrap();

        let outcome = undo(&UndoOp::RetryCleanup {
            parents: effects,
            operation: Some(Box::new(UndoOp::Create {
                path: created_file.clone(),
            })),
        })
        .unwrap();

        assert!(outcome.message.contains("Removed"));
        assert!(!created_file.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn substituted_parent_is_preserved_and_remains_in_retry_journal() {
        let root = tmpdir("substituted-parent-cleanup");
        let parent = root.join("created");
        let effects = create_missing_parents(&parent).unwrap();
        fs::remove_dir(&parent).unwrap();
        fs::create_dir(&parent).unwrap();
        let failure = undo(&UndoOp::RetryCleanup {
            parents: effects,
            operation: None,
        })
        .unwrap_err();
        assert!(parent.exists());
        assert!(failure.message.contains("identity changed"));
        assert!(matches!(
            failure.remaining,
            Some(UndoOp::RetryCleanup { .. })
        ));
        fs::remove_dir_all(root).unwrap();
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

    #[cfg(target_os = "linux")]
    #[test]
    fn destination_created_after_name_check_is_not_clobbered() {
        let temp_dir = tmpdir("move-name-race");
        let source = temp_dir.join("source");
        let candidate = unique_target(&temp_dir.join("target")).unwrap();
        fs::write(&source, b"source contents").unwrap();

        // Simulate another process claiming the checked name before rename.
        fs::write(&candidate, b"other process contents").unwrap();
        let error = rename_noreplace(&source, &candidate).unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&candidate).unwrap(), b"other process contents");
        assert_eq!(fs::read(&source).unwrap(), b"source contents");
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn panic_after_effect_preserves_the_partial_outcome() {
        let mut progress = new_progress(TransferKind::Copy, "test".into());
        progress.worker = Some(spawn_transfer_worker(|outcome| {
            outcome.committed.push(TransferEffect {
                source: PathBuf::from("source"),
                target: PathBuf::from("target"),
                trash_ref: None,
                source_removed: false,
            });
            panic!("simulated worker panic");
        }));
        while !progress.is_finished() {
            thread::yield_now();
        }

        let TransferCompletion::Panicked { outcome, message } = progress.join().unwrap() else {
            panic!("expected a structured panic completion");
        };
        assert_eq!(outcome.committed.len(), 1);
        assert_eq!(message, "simulated worker panic");
    }

    #[test]
    fn dropping_an_unfinished_transfer_cancels_without_joining() {
        let mut progress = new_progress(TransferKind::Copy, "test".into());
        let state = Arc::clone(&progress.state);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        progress.worker = Some(spawn_transfer_worker(move |_| {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        started_rx.recv().unwrap();

        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let dropper = thread::spawn(move || {
            drop(progress);
            dropped_tx.send(()).unwrap();
        });
        let returned_without_worker = dropped_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .is_ok();
        release_tx.send(()).unwrap();
        dropper.join().unwrap();

        assert!(returned_without_worker, "Progress::drop joined its worker");
        assert!(state.cancel_requested.load(Ordering::Relaxed));
    }

    #[test]
    fn unsupported_directory_rename_uses_safe_copy_fallback() {
        let temp_dir = tmpdir("move-rename");
        let source = temp_dir.join("source/tree");
        let destination = temp_dir.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        fs::write(source.join("file"), b"contents").unwrap();
        let mut progress = start_transfer(
            vec![source.clone()],
            destination.clone(),
            TransferKind::Move,
        );
        while !progress.is_finished() {
            std::thread::yield_now();
        }
        let outcome = progress.join().unwrap().expect_completed();
        let target = destination.join("tree");

        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.committed.len(), 1);
        assert!(source.exists());
        assert_eq!(fs::read(target.join("file")).unwrap(), b"contents");
        assert_eq!(outcome.retained_output.len(), 1);
        assert!(!outcome.committed[0].source_removed);
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn injected_exdev_selects_manifest_copy_fallback_without_touching_paths() {
        let mut called = false;
        let should_fallback =
            try_rename_move(false, Path::new("source"), Path::new("target"), |_, _| {
                called = true;
                Err(RenameFailure::Untouched(io::Error::new(
                    io::ErrorKind::CrossesDevices,
                    "injected EXDEV",
                )))
            })
            .unwrap();
        assert!(called);
        assert!(!should_fallback);

        let skipped = try_rename_move(true, Path::new("source"), Path::new("target"), |_, _| {
            panic!("cross-device capability check must skip rename")
        })
        .unwrap();
        assert!(!skipped);
    }

    #[test]
    fn copy_tree_reproduces_the_whole_subtree() {
        let temp_dir = tmpdir("copytree");
        fs::create_dir_all(temp_dir.join("src/sub")).unwrap();
        fs::write(temp_dir.join("src/sub/f"), b"hello").unwrap();
        let state = TransferState::new();
        copy_tree(
            &temp_dir.join("src"),
            &temp_dir.join("dst"),
            &state,
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(fs::read(temp_dir.join("dst/sub/f")).unwrap(), b"hello");
        assert_eq!(
            state.snapshot(),
            TransferProgress::Discovering {
                completed_entries: 3,
                current_entry: "f".into(),
            }
        );
        fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn copy_collision_does_not_claim_or_remove_an_external_target() {
        let temp_dir = tmpdir("copy-race");
        fs::write(temp_dir.join("source"), b"source").unwrap();
        fs::write(temp_dir.join("target"), b"external").unwrap();
        let state = TransferState::new();
        let mut manifest = Vec::new();

        let error = copy_tree(
            &temp_dir.join("source"),
            &temp_dir.join("target"),
            &state,
            &mut manifest,
        )
        .unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert!(manifest.is_empty());
        assert_eq!(fs::read(temp_dir.join("target")).unwrap(), b"external");
        fs::remove_dir_all(temp_dir).unwrap();
    }

    fn assert_substitution_is_preserved(kind: CreatedKind, make: impl Fn(&Path)) {
        let root = tmpdir("manifest-substitution");
        let path = root.join("output");
        match kind {
            CreatedKind::Directory => fs::create_dir(&path).unwrap(),
            CreatedKind::Other => fs::write(&path, b"owned").unwrap(),
        }
        let mut manifest = Vec::new();
        record_created(&path, kind, &mut manifest);
        match kind {
            CreatedKind::Directory => fs::remove_dir(&path).unwrap(),
            CreatedKind::Other => fs::remove_file(&path).unwrap(),
        }
        make(&path);

        let (failures, retained) = cleanup_created_nodes(&manifest);
        assert_eq!(failures.len(), 1);
        assert_eq!(retained, vec![path.clone()]);
        assert!(path.exists() || fs::symlink_metadata(&path).is_ok());
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn manifest_recording_never_stats_a_path_substituted_after_creation() {
        let root = tmpdir("manifest-create-stat-window");
        let path = root.join("published");
        // Simulate exclusive creation followed by immediate removal before the
        // recorder runs, then a foreign replacement in the old stat window.
        fs::write(&path, b"created").unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"foreign").unwrap();
        let mut manifest = Vec::new();
        record_created(&path, CreatedKind::Other, &mut manifest);
        let (_failures, retained) = cleanup_created_nodes(&manifest);
        assert_eq!(retained, vec![path.clone()]);
        assert_eq!(fs::read(&path).unwrap(), b"foreign");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_never_deletes_file_directory_or_symlink_substitutions() {
        assert_substitution_is_preserved(CreatedKind::Other, |path| {
            fs::write(path, b"external").unwrap();
        });
        assert_substitution_is_preserved(CreatedKind::Directory, |path| {
            fs::create_dir(path).unwrap();
        });
        assert_substitution_is_preserved(CreatedKind::Other, |path| {
            std::os::unix::fs::symlink("external-target", path).unwrap();
        });
    }

    #[test]
    fn manifest_preserves_unowned_children_and_reports_retained_root() {
        let root = tmpdir("manifest-child-race");
        let output = root.join("output");
        fs::create_dir(&output).unwrap();
        let mut manifest = Vec::new();
        record_created(&output, CreatedKind::Directory, &mut manifest);
        fs::write(output.join("external"), b"external").unwrap();

        let (failures, retained) = cleanup_created_nodes(&manifest);
        assert_eq!(failures.len(), 1);
        assert_eq!(retained, vec![output.clone()]);
        assert_eq!(fs::read(output.join("external")).unwrap(), b"external");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copied_move_conservatively_preserves_source_as_retained_output() {
        let mut outcome = TransferOutcome::default();
        let removed = record_source_cleanup(&mut outcome, Path::new("source"), Path::new("target"));
        assert!(!removed);
        assert!(outcome.failed.is_empty());
        assert_eq!(outcome.retained_output.len(), 1);
        assert!(outcome.retained_output[0]
            .message
            .contains("source preserved"));
    }

    #[test]
    fn cancelling_mid_copy_preserves_and_reports_partial_file() {
        let temp_dir = tmpdir("cancel");
        fs::write(temp_dir.join("big"), vec![0u8; 1024]).unwrap();
        let state = TransferState::new();
        state.cancel_requested.store(true, Ordering::Relaxed);
        let mut manifest = Vec::new();
        assert!(!copy_file_streaming(
            &temp_dir.join("big"),
            &temp_dir.join("out"),
            &state,
            &mut manifest,
        )
        .unwrap());
        let (failures, retained) = cleanup_created_nodes(&manifest);
        assert_eq!(failures.len(), 1);
        assert_eq!(retained, vec![temp_dir.join("out")]);
        assert!(temp_dir.join("out").exists());
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

        let mut progress = start_restore(deleted.committed, destination_dir.clone());
        while !progress.is_finished() {
            std::thread::yield_now();
        }
        let outcome = progress.join().unwrap().expect_completed();
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
