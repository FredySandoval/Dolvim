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
    notify_watcher: Option<RecommendedWatcher>,
    watched: Option<PathBuf>,
    dirty: Arc<AtomicBool>,
    last_event_at: Arc<Mutex<Instant>>,
}

impl Watcher {
    pub fn new() -> Watcher {
        let dirty = Arc::new(AtomicBool::new(false));
        let last_event_at = Arc::new(Mutex::new(Instant::now()));
        let (tx, rx) = channel();
        let notify_watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .ok();

        let dirty_flag = Arc::clone(&dirty);
        let last_event_at_for_thread = Arc::clone(&last_event_at);
        std::thread::spawn(move || {
            for event_result in rx {
                // Reading a directory is not a change to it. `notify` subscribes
                // to inotify's IN_OPEN/IN_ACCESS as well, so our own listing
                // arrives back here as an event: refresh, open, event, refresh,
                // for as long as the program runs. Everything else is a real
                // change and gets through.
                let Ok(event) = event_result else { continue };
                if matches!(event.kind, notify::EventKind::Access(_)) {
                    continue;
                }
                if let Ok(mut last_event_guard) = last_event_at_for_thread.lock() {
                    *last_event_guard = Instant::now();
                }
                dirty_flag.store(true, Ordering::Relaxed);
            }
        });

        Watcher {
            notify_watcher,
            watched: None,
            dirty,
            last_event_at,
        }
    }

    pub fn watch(&mut self, path: &Path) {
        if self.watched.as_deref() == Some(path) {
            return;
        }
        let Some(notify_watcher) = self.notify_watcher.as_mut() else {
            return;
        };
        if let Some(old) = self.watched.take() {
            let _ = notify_watcher.unwatch(&old);
        }
        if notify_watcher
            .watch(path, RecursiveMode::NonRecursive)
            .is_ok()
        {
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
            .last_event_at
            .lock()
            .map(|last_event| {
                last_event.elapsed() >= Duration::from_millis(config::WATCH_DEBOUNCE_MS)
            })
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
