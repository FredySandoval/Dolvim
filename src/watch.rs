//! inotify plumbing. Dolphin refreshes the view when the directory changes;
//! so do we, coalesced so an `unzip` does not cause a redraw storm.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::config;

pub struct Watcher {
    inner: Option<RecommendedWatcher>,
    watched: Option<PathBuf>,
    dirty: Arc<AtomicBool>,
    last: Arc<Mutex<Instant>>,
}

impl Watcher {
    pub fn new() -> Watcher {
        let dirty = Arc::new(AtomicBool::new(false));
        let last = Arc::new(Mutex::new(Instant::now()));
        let (tx, rx) = channel();
        let inner = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .ok();

        let d = Arc::clone(&dirty);
        let l = Arc::clone(&last);
        std::thread::spawn(move || {
            for ev in rx {
                // Reading a directory is not a change to it. `notify` subscribes
                // to inotify's IN_OPEN/IN_ACCESS as well, so our own listing
                // arrives back here as an event: refresh, open, event, refresh,
                // for as long as the program runs. Everything else is a real
                // change and gets through.
                let Ok(ev) = ev else { continue };
                if matches!(ev.kind, notify::EventKind::Access(_)) {
                    continue;
                }
                if let Ok(mut g) = l.lock() {
                    *g = Instant::now();
                }
                d.store(true, Ordering::Relaxed);
            }
        });

        Watcher {
            inner,
            watched: None,
            dirty,
            last,
        }
    }

    pub fn watch(&mut self, path: &Path) {
        if self.watched.as_deref() == Some(path) {
            return;
        }
        let Some(w) = self.inner.as_mut() else { return };
        if let Some(old) = self.watched.take() {
            let _ = w.unwatch(&old);
        }
        if w.watch(path, RecursiveMode::NonRecursive).is_ok() {
            self.watched = Some(path.to_path_buf());
        }
        self.dirty.store(false, Ordering::Relaxed);
    }

    /// True at most once per quiet period after a burst of events.
    pub fn take_dirty(&self) -> bool {
        if !self.dirty.load(Ordering::Relaxed) {
            return false;
        }
        let quiet = self
            .last
            .lock()
            .map(|g| g.elapsed() >= Duration::from_millis(config::WATCH_DEBOUNCE_MS))
            .unwrap_or(true);
        if quiet {
            self.dirty.store(false, Ordering::Relaxed);
        }
        quiet
    }
}

impl Default for Watcher {
    fn default() -> Self {
        Watcher::new()
    }
}
