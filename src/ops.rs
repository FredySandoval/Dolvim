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

#[derive(Default, Clone)]
pub struct Clipboard {
    pub paths: Vec<PathBuf>,
    pub cut: bool,
}

impl Clipboard {
    pub fn set(&mut self, paths: Vec<PathBuf>, cut: bool) {
        export_uris(&paths);
        self.paths = paths;
        self.cut = cut;
    }
}

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
    let cmds: [(&str, &[&str]); 3] = [
        ("wl-copy", &["--type", "text/uri-list"]),
        ("xclip", &["-selection", "clipboard", "-t", "text/uri-list"]),
        ("xsel", &["--clipboard", "--input"]),
    ];
    for (bin, args) in cmds {
        if which(bin).is_none() {
            continue;
        }
        let child = std::process::Command::new(bin)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        if let Ok(mut c) = child {
            use std::io::Write;
            if let Some(mut si) = c.stdin.take() {
                let _ = si.write_all(uris.as_bytes());
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
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
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
    let cmds: [(&str, &[&str]); 3] = [
        ("wl-paste", &["--type", "text/uri-list", "--no-newline"]),
        (
            "xclip",
            &["-selection", "clipboard", "-o", "-t", "text/uri-list"],
        ),
        ("xsel", &["--clipboard", "--output"]),
    ];
    for (bin, args) in cmds {
        if which(bin).is_none() {
            continue;
        }
        if let Ok(o) = std::process::Command::new(bin).args(args).output() {
            let txt = String::from_utf8_lossy(&o.stdout);
            let v: Vec<PathBuf> = txt
                .lines()
                .filter_map(|l| l.trim().strip_prefix("file://"))
                .map(|l| PathBuf::from(percent_decode(l)))
                .collect();
            if !v.is_empty() {
                return v;
            }
        }
    }
    Vec::new()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ---------------------------------------------------------------------------
// Undo journal
// ---------------------------------------------------------------------------

/// Exactly the set Dolphin can undo: renames, moves, trashing, and creation.
/// A recursive copy is *not* undoable in Dolphin either — it would mean
/// deleting files the user may have since edited.
#[derive(Clone, Debug)]
pub enum UndoOp {
    Rename { from: PathBuf, to: PathBuf },
    Move { pairs: Vec<(PathBuf, PathBuf)> },
    Trash { originals: Vec<PathBuf> },
    Create { path: PathBuf },
}

pub fn undo(op: &UndoOp) -> Result<String, String> {
    match op {
        UndoOp::Rename { from, to } => {
            fs::rename(to, from).map_err(|e| e.to_string())?;
            Ok(format!("Renamed back to {}", name(from)))
        }
        UndoOp::Move { pairs } => {
            for (from, to) in pairs {
                if let Some(p) = from.parent() {
                    let _ = fs::create_dir_all(p);
                }
                fs::rename(to, from).map_err(|e| e.to_string())?;
            }
            Ok(format!("Moved {} item(s) back", pairs.len()))
        }
        UndoOp::Trash { originals } => {
            let restored = restore_from_trash(originals)?;
            Ok(format!("Restored {restored} item(s) from Trash"))
        }
        UndoOp::Create { path } => {
            if path.is_dir() {
                fs::remove_dir(path).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(path).map_err(|e| e.to_string())?;
            }
            Ok(format!("Removed {}", name(path)))
        }
    }
}

pub fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// Trash
// ---------------------------------------------------------------------------

pub fn trash(paths: &[PathBuf]) -> Result<UndoOp, String> {
    trash::delete_all(paths).map_err(trash_error)?;
    Ok(UndoOp::Trash {
        originals: paths.to_vec(),
    })
}

/// Trash contents as view entries, so the Trash place browses like a folder.
pub fn list_trash() -> Vec<Entry> {
    let Ok(items) = trash::os_limited::list() else {
        return Vec::new();
    };
    items
        .into_iter()
        .map(|it| {
            let original = it.original_parent.join(&it.name);
            let meta = fs::symlink_metadata(&original).ok();
            Entry {
                name: it.name.to_string_lossy().into_owned(),
                path: original,
                kind: match meta.as_ref().map(|m| m.is_dir()) {
                    Some(true) => Kind::Dir,
                    _ => Kind::File,
                },
                size: meta.as_ref().map(|m| m.len()).unwrap_or(0),
                mtime: it.time_deleted,
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
            format!("{}: no longer there", name(&original))
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
        .filter(|it| {
            originals
                .iter()
                .any(|o| it.original_parent.join(&it.name) == *o)
        })
        .collect();
    if wanted.is_empty() {
        return Err("No matching items in Trash".into());
    }
    Ok(wanted)
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
    let hashes = pattern.chars().filter(|c| *c == '#').count();
    let mut pairs = Vec::new();
    for (i, p) in paths.iter().enumerate() {
        let num = format!("{:0width$}", i + 1, width = hashes);
        let mut name = String::new();
        let mut consumed = false;
        for c in pattern.chars() {
            if c == '#' {
                if !consumed {
                    name.push_str(&num);
                    consumed = true;
                }
            } else {
                name.push(c);
            }
        }
        // Keep the original extension when the pattern does not supply one.
        let name = if !name.contains('.') {
            match p.extension() {
                Some(e) => format!("{name}.{}", e.to_string_lossy()),
                None => name,
            }
        } else {
            name
        };
        let to = p.with_file_name(&name);
        if to.exists() && to != *p {
            return Err(format!("{name} already exists"));
        }
        fs::rename(p, &to).map_err(|e| format!("{}: {e}", self::name(p)))?;
        pairs.push((p.clone(), to));
    }
    Ok(UndoOp::Move { pairs })
}

// ---------------------------------------------------------------------------
// Copy / move with progress
// ---------------------------------------------------------------------------

pub struct Progress {
    pub label: String,
    pub total: Arc<AtomicU64>,
    pub done: Arc<AtomicU64>,
    pub current: Arc<Mutex<String>>,
    pub cancel_requested: Arc<AtomicBool>,
    pub finished: Arc<AtomicBool>,
    pub outcome: Arc<Mutex<Option<Result<UndoOp, String>>>>,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        let t = self.total.load(Ordering::Relaxed);
        if t == 0 {
            return 0.0;
        }
        (self.done.load(Ordering::Relaxed) as f64 / t as f64).clamp(0.0, 1.0)
    }
}

/// Start a copy or move in the background. Returns immediately.
pub fn start_transfer(sources: Vec<PathBuf>, dest: PathBuf, move_it: bool) -> Progress {
    let p = Progress {
        label: format!(
            "{} {} item(s) to {}",
            if move_it { "Moving" } else { "Copying" },
            sources.len(),
            name(&dest)
        ),
        total: Arc::new(AtomicU64::new(0)),
        done: Arc::new(AtomicU64::new(0)),
        current: Arc::new(Mutex::new(String::new())),
        cancel_requested: Arc::new(AtomicBool::new(false)),
        finished: Arc::new(AtomicBool::new(false)),
        outcome: Arc::new(Mutex::new(None)),
    };
    let (total, done, current, cancel_requested, finished, outcome) = (
        Arc::clone(&p.total),
        Arc::clone(&p.done),
        Arc::clone(&p.current),
        Arc::clone(&p.cancel_requested),
        Arc::clone(&p.finished),
        Arc::clone(&p.outcome),
    );

    thread::spawn(move || {
        total.store(
            sources.iter().map(|s| tree_size(s)).sum(),
            Ordering::Relaxed,
        );
        let mut pairs = Vec::new();
        let mut err = None;
        for src in &sources {
            if cancel_requested.load(Ordering::Relaxed) {
                break;
            }
            let target = unique_target(&dest.join(name(src)));
            // A rename within one filesystem is instant; try it first.
            if move_it && fs::rename(src, &target).is_ok() {
                done.fetch_add(tree_size(src), Ordering::Relaxed);
                pairs.push((src.clone(), target));
                continue;
            }
            if let Err(e) = copy_tree(src, &target, &done, &current, &cancel_requested) {
                err = Some(format!("{}: {e}", name(src)));
                break;
            }
            if move_it && !cancel_requested.load(Ordering::Relaxed) {
                if let Err(e) = remove_tree(src) {
                    err = Some(format!("{}: {e}", name(src)));
                    break;
                }
                pairs.push((src.clone(), target));
            }
        }
        let result = match err {
            Some(e) => Err(e),
            None if cancel_requested.load(Ordering::Relaxed) => Err("Cancelled".into()),
            None if move_it => Ok(UndoOp::Move { pairs }),
            // A copy leaves nothing to undo, so report it as a no-op journal
            // entry the caller drops.
            None => Ok(UndoOp::Move { pairs: Vec::new() }),
        };
        if let Ok(mut g) = outcome.lock() {
            *g = Some(result);
        }
        finished.store(true, Ordering::Relaxed);
    });
    p
}

fn tree_size(p: &Path) -> u64 {
    let Ok(m) = fs::symlink_metadata(p) else {
        return 0;
    };
    if m.is_dir() {
        fs::read_dir(p)
            .map(|d| d.flatten().map(|e| tree_size(&e.path())).sum())
            .unwrap_or(0)
    } else {
        m.len()
    }
}

/// Never clobber silently: `file.txt` becomes `file (1).txt`, as Dolphin does
/// when you paste into the directory a file already lives in.
fn unique_target(p: &Path) -> PathBuf {
    if !p.exists() {
        return p.to_path_buf();
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    for n in 1..10_000 {
        let cand = p.with_file_name(format!("{stem} ({n}){ext}"));
        if !cand.exists() {
            return cand;
        }
    }
    p.to_path_buf()
}

fn copy_tree(
    src: &Path,
    dst: &Path,
    done: &Arc<AtomicU64>,
    current: &Arc<Mutex<String>>,
    cancel_requested: &Arc<AtomicBool>,
) -> io::Result<()> {
    if cancel_requested.load(Ordering::Relaxed) {
        return Ok(());
    }
    let meta = fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        let target = fs::read_link(src)?;
        std::os::unix::fs::symlink(target, dst)?;
        return Ok(());
    }
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            copy_tree(
                &e.path(),
                &dst.join(e.file_name()),
                done,
                current,
                cancel_requested,
            )?;
            if cancel_requested.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
        return Ok(());
    }
    if let Ok(mut g) = current.lock() {
        *g = name(src);
    }
    copy_file_streaming(src, dst, done, cancel_requested)
}

/// Copy in chunks so the progress bar moves during a 4 GiB file and cancel is
/// honoured mid-file rather than only between files.
fn copy_file_streaming(
    src: &Path,
    dst: &Path,
    done: &Arc<AtomicU64>,
    cancel_requested: &Arc<AtomicBool>,
) -> io::Result<()> {
    use io::{Read, Write};
    let mut r = fs::File::open(src)?;
    let mut w = fs::File::create(dst)?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if cancel_requested.load(Ordering::Relaxed) {
            // A half-written file is worse than none; take it back out.
            drop(w);
            let _ = fs::remove_file(dst);
            return Ok(());
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        w.write_all(&buf[..n])?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    if let Ok(m) = fs::metadata(src) {
        let _ = fs::set_permissions(dst, m.permissions());
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
        remove_tree(p).map_err(|e| format!("{}: {e}", name(p)))?;
        n += 1;
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Archives — shelled out, presence-checked, as PLAN.md specifies
// ---------------------------------------------------------------------------

pub fn extract(archive: &Path, into: &Path) -> Result<String, String> {
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
                into.to_string_lossy().into(),
            ],
        ),
        "tar" | "gz" | "tgz" | "bz2" | "xz" | "zst" => (
            "tar",
            vec![
                "-xaf".into(),
                archive.to_string_lossy().into(),
                "-C".into(),
                into.to_string_lossy().into(),
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
        Ok(format!("Extracted {}", name(archive)))
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
        cmd.arg(name(p));
    }
    let out = cmd.output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(format!("Created {}", name(dest)))
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("dolvim-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn rename_round_trips_through_undo() {
        let d = tmpdir("rename");
        let a = d.join("a.txt");
        fs::write(&a, b"x").unwrap();
        let op = rename(&a, "b.txt").unwrap();
        assert!(d.join("b.txt").exists() && !a.exists());
        undo(&op).unwrap();
        assert!(a.exists() && !d.join("b.txt").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn rename_refuses_to_clobber() {
        let d = tmpdir("clobber");
        fs::write(d.join("a"), b"1").unwrap();
        fs::write(d.join("b"), b"2").unwrap();
        assert!(rename(&d.join("a"), "b").is_err());
        assert_eq!(fs::read(d.join("b")).unwrap(), b"2");
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn batch_rename_pads_to_the_hash_count() {
        let d = tmpdir("batch");
        let paths: Vec<PathBuf> = (0..3)
            .map(|i| {
                let p = d.join(format!("src{i}.jpg"));
                fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        batch_rename(&paths, "Holiday ##").unwrap();
        assert!(d.join("Holiday 01.jpg").exists());
        assert!(d.join("Holiday 03.jpg").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn batch_rename_is_undoable_as_a_move() {
        let d = tmpdir("batchundo");
        let p = d.join("one.txt");
        fs::write(&p, b"x").unwrap();
        let op = batch_rename(std::slice::from_ref(&p), "n#").unwrap();
        assert!(d.join("n1.txt").exists());
        undo(&op).unwrap();
        assert!(p.exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn unique_target_never_overwrites() {
        let d = tmpdir("unique");
        let a = d.join("f.txt");
        fs::write(&a, b"x").unwrap();
        assert_eq!(unique_target(&a), d.join("f (1).txt"));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn copy_tree_reproduces_the_whole_subtree() {
        let d = tmpdir("copytree");
        fs::create_dir_all(d.join("src/sub")).unwrap();
        fs::write(d.join("src/sub/f"), b"hello").unwrap();
        let done = Arc::new(AtomicU64::new(0));
        let cur = Arc::new(Mutex::new(String::new()));
        let cancel_requested = Arc::new(AtomicBool::new(false));
        copy_tree(
            &d.join("src"),
            &d.join("dst"),
            &done,
            &cur,
            &cancel_requested,
        )
        .unwrap();
        assert_eq!(fs::read(d.join("dst/sub/f")).unwrap(), b"hello");
        assert_eq!(done.load(Ordering::Relaxed), 5);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn cancelling_mid_copy_leaves_no_partial_file() {
        let d = tmpdir("cancel");
        fs::write(d.join("big"), vec![0u8; 1024]).unwrap();
        let done = Arc::new(AtomicU64::new(0));
        let cancel_requested = Arc::new(AtomicBool::new(true));
        copy_file_streaming(&d.join("big"), &d.join("out"), &done, &cancel_requested).unwrap();
        assert!(!d.join("out").exists());
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn base64_matches_rfc4648_padding() {
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
    }

    #[test]
    fn uri_encoding_round_trips_spaces() {
        assert_eq!(percent_encode("/a b/c"), "/a%20b/c");
        assert_eq!(percent_decode("/a%20b/c"), "/a b/c");
    }
}
