//! Generation-aware, debounced filesystem watching.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};

use crate::config;

#[derive(Clone)]
struct PendingDirty {
    generation: u64,
    path: PathBuf,
    at: Instant,
}

struct PendingError {
    generation: u64,
    path: Option<PathBuf>,
    message: String,
}

struct DebounceState {
    generation: u64,
    target: Option<PathBuf>,
    /// Every committed subscription identity remains available for late
    /// callbacks. Event paths are reduced against all matching generations, so
    /// a callback delayed across a transition cannot be misattributed.
    targets: VecDeque<(u64, PathBuf)>,
    dirty: VecDeque<PendingDirty>,
    error: Option<PendingError>,
    last_reported_error: Option<(String, Instant)>,
}

impl DebounceState {
    fn commit_target(&mut self, target: Option<PathBuf>) {
        self.generation += 1;
        self.target = target;
        if let Some(target) = &self.target {
            self.targets.push_back((self.generation, target.clone()));
        }
        // Deliberately retain prior subscription identities and pending records:
        // notify callbacks can arrive after an unwatch transition.
    }

    fn queue_dirty(&mut self, generation: u64, path: PathBuf, now: Instant) {
        if let Some(pending) = self
            .dirty
            .iter_mut()
            .find(|pending| pending.generation == generation && pending.path == path)
        {
            pending.at = now;
        } else {
            self.dirty.push_back(PendingDirty {
                generation,
                path,
                at: now,
            });
        }
    }

    fn record_event(&mut self, event: notify::Result<notify::Event>) {
        match event {
            Ok(event) if matches!(event.kind, notify::EventKind::Access(_)) => {}
            Ok(event) => {
                let now = Instant::now();
                let matches = if event.paths.is_empty() {
                    self.target
                        .clone()
                        .map(|path| vec![(self.generation, path)])
                        .unwrap_or_default()
                } else {
                    self.targets
                        .iter()
                        .filter(|(_, target)| {
                            event.paths.iter().any(|path| path.starts_with(target))
                        })
                        .cloned()
                        .collect::<Vec<_>>()
                };
                for (generation, path) in matches {
                    self.queue_dirty(generation, path, now);
                }
            }
            Err(error) => {
                let message = error.to_string();
                if self.error.as_ref().map(|error| error.message.as_str()) != Some(&message) {
                    self.error = Some(PendingError {
                        generation: self.generation,
                        path: self.target.clone(),
                        message,
                    });
                }
            }
        }
    }

    fn take_update(&mut self, now: Instant) -> WatchUpdate {
        let mut dirty_paths = Vec::new();
        let mut waiting = VecDeque::new();
        while let Some(dirty) = self.dirty.pop_front() {
            if now.saturating_duration_since(dirty.at)
                >= Duration::from_millis(config::WATCH_DEBOUNCE_MS)
            {
                dirty_paths.push(dirty.path);
            } else {
                waiting.push_back(dirty);
            }
        }
        self.dirty = waiting;
        let error = self.error.take().and_then(|error| {
            let context = match error.path {
                Some(path) => format!(
                    "{} [watch generation {}]: {}",
                    path.display(),
                    error.generation,
                    error.message
                ),
                None => format!("watch generation {}: {}", error.generation, error.message),
            };
            let repeated = self
                .last_reported_error
                .as_ref()
                .is_some_and(|(previous, at)| {
                    previous == &context
                        && now.saturating_duration_since(*at) < Duration::from_secs(1)
                });
            if repeated {
                None
            } else {
                self.last_reported_error = Some((context.clone(), now));
                Some(context)
            }
        });
        WatchUpdate { dirty_paths, error }
    }
}

pub struct WatchUpdate {
    pub dirty_paths: Vec<PathBuf>,
    pub error: Option<String>,
}

#[cfg(test)]
fn transition_subscription(
    watched: &mut Option<PathBuf>,
    state: &mut DebounceState,
    new_target: Option<&Path>,
    mut unwatch: impl FnMut(&Path) -> Result<(), String>,
    mut watch: impl FnMut(&Path) -> Result<(), String>,
) -> Result<(), String> {
    if watched.as_deref() == new_target {
        return Ok(());
    }
    if let Some(old) = watched.as_deref() {
        // Ownership is retained until the backend confirms unregistration.
        unwatch(old)?;
        *watched = None;
        state.commit_target(None);
    }
    let Some(new_target) = new_target else {
        return Ok(());
    };
    watch(new_target)?;
    *watched = Some(new_target.to_path_buf());
    state.commit_target(Some(new_target.to_path_buf()));
    Ok(())
}

pub struct Watcher {
    notify_watcher: Option<RecommendedWatcher>,
    initialization_error: Option<String>,
    watched: Option<PathBuf>,
    state: Arc<Mutex<DebounceState>>,
}

impl Watcher {
    pub fn new() -> Watcher {
        let state = Arc::new(Mutex::new(DebounceState {
            generation: 0,
            target: None,
            targets: VecDeque::new(),
            dirty: VecDeque::new(),
            error: None,
            last_reported_error: None,
        }));
        let callback_state = Arc::clone(&state);
        let watcher = notify::recommended_watcher(move |event| {
            if let Ok(mut state) = callback_state.lock() {
                state.record_event(event);
            }
        });
        let (notify_watcher, initialization_error) = match watcher {
            Ok(watcher) => (Some(watcher), None),
            Err(error) => (None, Some(error.to_string())),
        };
        Watcher {
            notify_watcher,
            initialization_error,
            watched: None,
            state,
        }
    }

    pub fn watch(&mut self, path: &Path) -> Result<(), String> {
        let Some(backend) = self.notify_watcher.as_mut() else {
            return Err(self
                .initialization_error
                .clone()
                .unwrap_or_else(|| "watcher is unavailable".into()));
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| "watcher state lock is poisoned".to_string())?;
        if self.watched.as_deref() == Some(path) {
            return Ok(());
        }
        if let Some(old) = self.watched.as_deref() {
            backend.unwatch(old).map_err(|error| error.to_string())?;
            self.watched = None;
            state.commit_target(None);
        }
        backend
            .watch(path, RecursiveMode::NonRecursive)
            .map_err(|error| error.to_string())?;
        self.watched = Some(path.to_path_buf());
        state.commit_target(Some(path.to_path_buf()));
        Ok(())
    }

    pub fn unwatch(&mut self) -> Result<(), String> {
        let Some(backend) = self.notify_watcher.as_mut() else {
            return Ok(());
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| "watcher state lock is poisoned".to_string())?;
        if let Some(old) = self.watched.as_deref() {
            backend.unwatch(old).map_err(|error| error.to_string())?;
            self.watched = None;
            state.commit_target(None);
        }
        Ok(())
    }

    pub fn take_update(&self) -> Result<WatchUpdate, String> {
        Ok(self
            .state
            .lock()
            .map_err(|_| "watcher state lock is poisoned".to_string())?
            .take_update(Instant::now()))
    }
}

impl Default for Watcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(path: &str) -> DebounceState {
        DebounceState {
            generation: 1,
            target: Some(path.into()),
            targets: VecDeque::from([(1, PathBuf::from(path))]),
            dirty: VecDeque::new(),
            error: None,
            last_reported_error: None,
        }
    }

    #[test]
    fn failed_unwatch_retains_subscription_and_pending_event() {
        let now = Instant::now();
        let mut state = state("/old");
        state.dirty.push_back(PendingDirty {
            generation: 1,
            path: "/old".into(),
            at: now - Duration::from_secs(1),
        });
        let mut watched = Some(PathBuf::from("/old"));
        let result = transition_subscription(
            &mut watched,
            &mut state,
            Some(Path::new("/new")),
            |_| Err("injected unwatch failure".into()),
            |_| Ok(()),
        );
        assert!(result.is_err());
        assert_eq!(watched.as_deref(), Some(Path::new("/old")));
        assert_eq!(state.target.as_deref(), Some(Path::new("/old")));
        assert_eq!(
            state
                .take_update(now)
                .dirty_paths
                .first()
                .map(PathBuf::as_path),
            Some(Path::new("/old"))
        );
    }

    #[test]
    fn old_event_transition_then_new_event_drains_both_paths_in_order() {
        let now = Instant::now();
        let mut state = state("/old");
        state.dirty.push_back(PendingDirty {
            generation: 1,
            path: "/old".into(),
            at: now - Duration::from_secs(1),
        });
        let mut watched = Some(PathBuf::from("/old"));
        transition_subscription(
            &mut watched,
            &mut state,
            Some(Path::new("/new")),
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(watched.as_deref(), Some(Path::new("/new")));
        state.dirty.push_back(PendingDirty {
            generation: state.generation,
            path: "/new".into(),
            at: now - Duration::from_secs(1),
        });
        assert_eq!(
            state.take_update(now).dirty_paths,
            vec![PathBuf::from("/old"), PathBuf::from("/new")]
        );
    }

    #[test]
    fn failed_registration_has_explicit_unwatched_state_and_can_retry() {
        let now = Instant::now();
        let mut state = state("/old");
        state.dirty.push_back(PendingDirty {
            generation: 1,
            path: "/old".into(),
            at: now - Duration::from_secs(1),
        });
        let mut watched = Some(PathBuf::from("/old"));
        let failed = transition_subscription(
            &mut watched,
            &mut state,
            Some(Path::new("/new")),
            |_| Ok(()),
            |_| Err("injected registration failure".into()),
        );
        assert!(failed.is_err());
        assert!(watched.is_none());
        assert!(state.target.is_none());

        transition_subscription(
            &mut watched,
            &mut state,
            Some(Path::new("/new")),
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        assert_eq!(watched.as_deref(), Some(Path::new("/new")));
        assert_eq!(
            state
                .take_update(now)
                .dirty_paths
                .first()
                .map(PathBuf::as_path),
            Some(Path::new("/old"))
        );
    }

    #[test]
    fn callback_delayed_across_transition_keeps_old_and_new_targets() {
        let now = Instant::now();
        let mut state = state("/old");
        state.commit_target(Some(PathBuf::from("/new")));

        state.record_event(Ok(
            notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from("/old/item"))
        ));
        state.record_event(Ok(
            notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from("/new/item"))
        ));
        for dirty in &mut state.dirty {
            dirty.at = now - Duration::from_secs(1);
        }

        let update = state.take_update(now);
        assert_eq!(
            update.dirty_paths,
            vec![PathBuf::from("/old"), PathBuf::from("/new")]
        );
    }

    #[test]
    fn pending_refreshes_are_not_silently_evicted() {
        let now = Instant::now();
        let mut state = state("/target-0");
        for index in 1..=40 {
            let path = PathBuf::from(format!("/target-{index}"));
            state.commit_target(Some(path.clone()));
            state.record_event(Ok(
                notify::Event::new(notify::EventKind::Any).add_path(path.join("item"))
            ));
        }
        for dirty in &mut state.dirty {
            dirty.at = now - Duration::from_secs(1);
        }

        assert_eq!(state.take_update(now).dirty_paths.len(), 40);
    }

    #[test]
    fn refresh_and_error_are_consumed_together_and_errors_are_bounded() {
        let now = Instant::now();
        let mut state = state("/watched");
        state.dirty.push_back(PendingDirty {
            generation: 1,
            path: "/watched".into(),
            at: now - Duration::from_secs(1),
        });
        state.error = Some(PendingError {
            generation: 1,
            path: Some("/watched".into()),
            message: "same".into(),
        });
        let update = state.take_update(now);
        assert_eq!(
            update.dirty_paths.first().map(PathBuf::as_path),
            Some(Path::new("/watched"))
        );
        assert!(update.error.is_some());
        state.error = Some(PendingError {
            generation: 1,
            path: Some("/watched".into()),
            message: "same".into(),
        });
        assert!(state.take_update(now).error.is_none());
    }
}
