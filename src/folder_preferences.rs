//! Per-directory sorting state. Updates merge under a stable sidecar lock.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::fs::Sort;
use crate::shared_register::NativePath;

const MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
struct Preference {
    path: NativePath,
    sort: Sort,
}

#[derive(Serialize, Deserialize)]
struct State {
    version: u32,
    folders: Vec<Preference>,
}

pub fn directory_key(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn for_app() -> Option<Self> {
        if cfg!(test) {
            return None;
        }
        let root = std::env::var_os("XDG_STATE_HOME")
            .filter(|value| Path::new(value).is_absolute())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })?;
        Some(Self::new(root.join("dolvim")))
    }

    fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.directory)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.directory.join("folder-preferences.lock"))?;
        file.lock()?;
        Ok(file)
    }

    fn read_locked(&self) -> io::Result<State> {
        let file = match File::open(self.directory.join("folder-preferences.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(State {
                    version: 1,
                    folders: Vec::new(),
                });
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "folder preferences exceed 8 MiB",
            ));
        }
        let state: State = serde_json::from_slice(&bytes)?;
        if state.version != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported folder preferences version",
            ));
        }
        for folder in &state.folders {
            if !folder.path.decode()?.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "folder preference path must be absolute",
                ));
            }
        }
        Ok(state)
    }

    pub fn get(&self, path: &Path) -> io::Result<Sort> {
        let key = NativePath::encode(&directory_key(path));
        let _lock = self.lock()?;
        Ok(self
            .read_locked()?
            .folders
            .into_iter()
            .find(|folder| folder.path == key)
            .map(|folder| folder.sort)
            .unwrap_or_default())
    }

    pub fn set(&self, path: &Path, sort: Sort) -> io::Result<()> {
        let path = directory_key(path);
        if !path.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "folder preference path must be absolute",
            ));
        }
        let key = NativePath::encode(&path);
        let _lock = self.lock()?;
        let mut state = self.read_locked()?;
        state.folders.retain(|folder| folder.path != key);
        // The default needs no override, including when resetting a folder.
        if sort != Sort::default() {
            state.folders.push(Preference { path: key, sort });
        }
        let bytes = serde_json::to_vec_pretty(&state)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "folder preferences exceed 8 MiB",
            ));
        }
        let temporary = self.directory.join("folder-preferences.pending");
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let result = (|| {
            let mut options = OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, self.directory.join("folder-preferences.json"))?;
            #[cfg(unix)]
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::fs::SortKey;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Sandbox(PathBuf);
    impl Sandbox {
        fn new() -> Self {
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "dolvim-folder-preferences-{}-{}",
                std::process::id(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Store {
            Store::new(self.0.join("state"))
        }
    }
    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn date_sort() -> Sort {
        Sort {
            key: SortKey::Date,
            reverse: true,
            dirs_first: false,
        }
    }

    #[test]
    fn independent_handles_preserve_other_folders_and_reset_defaults() {
        let sandbox = Sandbox::new();
        let a = sandbox.0.join("a");
        let b = sandbox.0.join("b");
        assert_eq!(sandbox.store().get(&a).unwrap(), Sort::default());
        sandbox.store().set(&a, date_sort()).unwrap();
        sandbox
            .store()
            .set(
                &b,
                Sort {
                    key: SortKey::Size,
                    ..Sort::default()
                },
            )
            .unwrap();
        assert_eq!(sandbox.store().get(&a).unwrap(), date_sort());
        sandbox.store().set(&b, Sort::default()).unwrap();
        assert_eq!(sandbox.store().get(&a).unwrap(), date_sort());
        assert_eq!(sandbox.store().get(&b).unwrap(), Sort::default());
    }

    #[test]
    fn concurrent_updates_merge_under_lock() {
        let sandbox = Sandbox::new();
        std::thread::scope(|scope| {
            for i in 0..12 {
                let directory = sandbox.0.clone();
                scope.spawn(move || {
                    Store::new(directory.join("state"))
                        .set(&directory.join(i.to_string()), date_sort())
                        .unwrap();
                });
            }
        });
        for i in 0..12 {
            assert_eq!(
                sandbox.store().get(&sandbox.0.join(i.to_string())).unwrap(),
                date_sort()
            );
        }
    }

    #[test]
    fn invalid_state_is_reported_and_never_overwritten() {
        let sandbox = Sandbox::new();
        let store = sandbox.store();
        store.set(&sandbox.0, date_sort()).unwrap();
        let path = sandbox.0.join("state/folder-preferences.json");
        for bytes in [b"broken".as_slice(), b"{\"version\":2,\"folders\":[]}"] {
            fs::write(&path, bytes).unwrap();
            assert!(store.get(&sandbox.0).is_err());
            assert!(store.set(&sandbox.0, Sort::default()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    #[test]
    fn navigation_tabs_splits_and_restart_restore_all_sort_fields() {
        let sandbox = Sandbox::new();
        let downloads = sandbox.0.join("Downloads");
        let child = downloads.join("child");
        fs::create_dir_all(&child).unwrap();
        let mut app = App::new(downloads.clone());
        app.folder_preferences = Some(sandbox.store());
        app.set_sort(SortKey::Date);
        app.toggle_dirs_first();
        assert_eq!(app.pane().sort, date_sort());
        app.open_dir(child.clone());
        assert_eq!(app.pane().sort, Sort::default());
        app.open_dir(downloads.clone());
        assert_eq!(app.pane().sort, date_sort());
        app.toggle_split();
        assert_eq!(app.pane().sort, date_sort());
        app.new_tab(downloads.clone());
        assert_eq!(app.pane().sort, date_sort());
        drop(app);
        let mut reopened = App::new(child.clone());
        reopened.folder_preferences = Some(sandbox.store());
        reopened.open_dir(downloads.clone());
        assert_eq!(reopened.pane().sort, date_sort());
        reopened.set_sort(SortKey::Date);
        assert!(!reopened.pane().sort.reverse);
        reopened.open_dir(child);
        reopened.open_dir(downloads);
        assert!(!reopened.pane().sort.reverse);
    }

    #[cfg(unix)]
    #[test]
    fn native_paths_and_symlink_aliases_round_trip() {
        use std::os::unix::{ffi::OsStringExt, fs::symlink};
        let sandbox = Sandbox::new();
        let path = sandbox
            .0
            .join(std::ffi::OsString::from_vec(b"folder-\xff".to_vec()));
        fs::create_dir(&path).unwrap();
        let alias = sandbox.0.join("alias");
        symlink(&path, &alias).unwrap();
        sandbox.store().set(&alias, date_sort()).unwrap();
        assert_eq!(sandbox.store().get(&path).unwrap(), date_sort());
        assert_eq!(sandbox.store().get(&alias).unwrap(), date_sort());
    }
}
