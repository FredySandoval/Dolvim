//! Shared-register storage and wire contract. UI integration follows separately.
//! Native path units are authoritative; display strings are never decoded.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "encoding", content = "data", rename_all = "kebab-case")]
pub enum NativePath {
    UnixBytes(Vec<u8>),
    WindowsUtf16(Vec<u16>),
}

impl NativePath {
    pub fn encode(path: &Path) -> Self {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            Self::UnixBytes(path.as_os_str().as_bytes().to_vec())
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            Self::WindowsUtf16(path.as_os_str().encode_wide().collect())
        }
    }

    pub fn decode(&self) -> io::Result<PathBuf> {
        #[cfg(unix)]
        if let Self::UnixBytes(bytes) = self {
            use std::os::unix::ffi::OsStringExt;
            return Ok(PathBuf::from(OsString::from_vec(bytes.clone())));
        }
        #[cfg(windows)]
        if let Self::WindowsUtf16(units) = self {
            use std::os::windows::ffi::OsStringExt;
            return Ok(PathBuf::from(OsString::from_wide(units)));
        }
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "register path encoding belongs to another platform",
        ))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Intent {
    Copy,
    Cut,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Selection {
    pub version: u32,
    pub revision: String,
    pub intent: Intent,
    pub paths: Vec<NativePath>,
    #[serde(default)]
    pub clipboard: Option<String>,
}

impl Selection {
    pub fn new(revision: String, intent: Intent, paths: &[PathBuf]) -> Self {
        Self {
            version: 1,
            revision,
            intent,
            paths: paths.iter().map(|path| NativePath::encode(path)).collect(),
            clipboard: None,
        }
    }

    pub fn clipboard_changed(&self, current: Option<&str>) -> bool {
        current.is_some_and(|current| {
            self.clipboard.as_deref().is_none_or(|published| {
                current.trim_end_matches(['\r', '\n']) != published.trim_end_matches(['\r', '\n'])
            })
        })
    }

    fn validate(&self) -> io::Result<()> {
        if self.version != 1 || self.revision.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "unsupported register version or empty revision",
            ));
        }
        for path in &self.paths {
            let decoded = path.decode()?;
            if !decoded.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "register paths must be absolute",
                ));
            }
            match path {
                NativePath::UnixBytes(units) if units.contains(&0) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "NUL in path"));
                }
                NativePath::WindowsUtf16(units) if units.contains(&0) => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "NUL in path"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn from_json(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "register exceeds 8 MiB",
            ));
        }
        let selection: Self = serde_json::from_slice(bytes)?;
        selection.validate()?;
        Ok(selection)
    }

    pub fn to_json(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "register exceeds 8 MiB",
            ));
        }
        Ok(bytes)
    }
}

/// A stable sidecar lock protects the JSON file even when replacement changes
/// its inode. Dropping the lock handle releases it, including after a crash.
pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub fn for_app() -> Option<Self> {
        // Unit tests must never touch a user's register or share global state.
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
        let lock = options.open(self.directory.join("register.lock"))?;
        lock.lock()?;
        Ok(lock)
    }

    fn read_locked(&self) -> io::Result<Option<Selection>> {
        let file = match File::open(self.directory.join("register.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        Selection::from_json(&bytes).map(Some)
    }

    pub fn read(&self) -> io::Result<Option<Selection>> {
        let _lock = self.lock()?;
        self.read_locked()
    }

    /// Generates the revision internally: publishing identical paths is still
    /// a new selection, not permission for an older paste to consume them.
    pub fn publish(&self, intent: Intent, paths: &[PathBuf]) -> io::Result<Selection> {
        let clipboard = Some(crate::ops::clipboard_text(paths));
        self.publish_with_clipboard(intent, paths, clipboard)
    }

    /// Publishes while recording the desktop clipboard contents associated
    /// with this selection. Cuts use the pre-existing clipboard snapshot
    /// because Dolvim deliberately does not export ambiguous desktop cuts.
    pub fn publish_with_clipboard(
        &self,
        intent: Intent,
        paths: &[PathBuf],
        clipboard: Option<String>,
    ) -> io::Result<Selection> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let revision = format!(
            "{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos(),
            SERIAL.fetch_add(1, Ordering::Relaxed)
        );
        let mut selection = Selection::new(revision, intent, paths);
        selection.clipboard = clipboard;
        self.publish_selection(&selection)?;
        Ok(selection)
    }

    fn publish_selection(&self, selection: &Selection) -> io::Result<()> {
        let bytes = selection.to_json()?;
        let _lock = self.lock()?;
        self.replace_locked(&bytes)
    }

    fn replace_locked(&self, bytes: &[u8]) -> io::Result<()> {
        let temporary = self.directory.join("register.pending");
        // A crash may leave a pending file, but never makes it authoritative.
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
            file.write_all(bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, self.directory.join("register.json"))?;
            #[cfg(unix)]
            File::open(&self.directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    /// Failed and cancelled sources remain retryable. Copy selections are never
    /// consumed. An empty cut selection remains an explicit empty register.
    pub fn finish_cut(&self, revision: &str, moved: &[PathBuf]) -> io::Result<bool> {
        let _lock = self.lock()?;
        let Some(mut selection) = self.read_locked()? else {
            return Ok(false);
        };
        if selection.revision != revision || selection.intent != Intent::Cut {
            return Ok(false);
        }
        let moved: Vec<_> = moved.iter().map(|path| NativePath::encode(path)).collect();
        selection.paths.retain(|path| !moved.contains(path));
        self.replace_locked(&selection.to_json()?)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestStore(Store);

    impl TestStore {
        fn new() -> Self {
            static SERIAL: AtomicU64 = AtomicU64::new(0);
            Self(Store::new(std::env::temp_dir().join(format!(
                "dolvim-register-{}-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
                SERIAL.fetch_add(1, Ordering::Relaxed)
            ))))
        }
    }

    impl Drop for TestStore {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.directory);
        }
    }

    #[test]
    fn stores_selection_across_independent_handles() {
        let test = TestStore::new();
        assert!(test.0.read().unwrap().is_none());
        let published = test
            .0
            .publish(Intent::Copy, &[PathBuf::from("/tmp/one")])
            .unwrap();
        let other = Store::new(test.0.directory.clone());
        assert_eq!(other.read().unwrap(), Some(published.clone()));
        assert!(!other
            .finish_cut(&published.revision, &[PathBuf::from("/tmp/one")])
            .unwrap());
        assert_eq!(other.read().unwrap(), Some(published));
    }

    #[test]
    fn partial_cut_preserves_failed_sources_and_newer_selections() {
        let test = TestStore::new();
        let paths = vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")];
        let first = test.0.publish(Intent::Cut, &paths).unwrap();
        assert!(test.0.finish_cut(&first.revision, &paths[..1]).unwrap());
        assert_eq!(
            test.0.read().unwrap().unwrap().paths,
            vec![NativePath::encode(&paths[1])]
        );
        let newer = test.0.publish(Intent::Cut, &paths).unwrap();
        assert_ne!(first.revision, newer.revision);
        assert!(!test.0.finish_cut(&first.revision, &paths).unwrap());
        assert_eq!(test.0.read().unwrap(), Some(newer.clone()));
        assert!(test.0.finish_cut(&newer.revision, &paths).unwrap());
        assert!(test.0.read().unwrap().unwrap().paths.is_empty());
    }

    #[test]
    fn invalid_publish_preserves_previous_state_and_pending_is_ignored() {
        let test = TestStore::new();
        let first = test
            .0
            .publish(Intent::Copy, &[PathBuf::from("/tmp/one")])
            .unwrap();
        fs::write(test.0.directory.join("register.pending"), b"broken").unwrap();
        assert_eq!(test.0.read().unwrap(), Some(first.clone()));
        assert!(test
            .0
            .publish(Intent::Cut, &[PathBuf::from("relative")])
            .is_err());
        assert_eq!(test.0.read().unwrap(), Some(first));
        test.0
            .publish(Intent::Copy, &[PathBuf::from("/tmp/two")])
            .unwrap();
        assert!(!test.0.directory.join("register.pending").exists());
    }

    #[test]
    fn subprocess_cut_worker() {
        let Some(directory) = std::env::var_os("DOLVIM_TEST_REGISTER_DIRECTORY") else {
            return;
        };
        let revision = std::env::var("DOLVIM_TEST_REGISTER_REVISION").unwrap();
        let source = std::env::var_os("DOLVIM_TEST_REGISTER_SOURCE").unwrap();
        assert!(Store::new(PathBuf::from(directory))
            .finish_cut(&revision, &[PathBuf::from(source)])
            .unwrap());
    }

    #[test]
    fn independent_processes_serialize_cut_updates() {
        let test = TestStore::new();
        let paths: Vec<_> = (0..4)
            .map(|index| PathBuf::from(format!("/tmp/process-{index}")))
            .collect();
        let first = test.0.publish(Intent::Cut, &paths).unwrap();
        let mut children = Vec::new();
        for path in paths {
            children.push(
                std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", "shared_register::tests::subprocess_cut_worker"])
                    .env("DOLVIM_TEST_REGISTER_DIRECTORY", &test.0.directory)
                    .env("DOLVIM_TEST_REGISTER_REVISION", &first.revision)
                    .env("DOLVIM_TEST_REGISTER_SOURCE", path)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert!(test.0.read().unwrap().unwrap().paths.is_empty());
    }

    #[test]
    fn concurrent_updates_do_not_lose_cut_results() {
        let test = TestStore::new();
        let paths: Vec<_> = (0..12)
            .map(|index| PathBuf::from(format!("/tmp/{index}")))
            .collect();
        let first = test.0.publish(Intent::Cut, &paths).unwrap();
        std::thread::scope(|scope| {
            for path in paths {
                let store = Store::new(test.0.directory.clone());
                let revision = first.revision.clone();
                scope.spawn(move || assert!(store.finish_cut(&revision, &[path]).unwrap()));
            }
        });
        assert!(test.0.read().unwrap().unwrap().paths.is_empty());
    }

    #[test]
    fn corrupt_storage_is_reported_without_replacing_it() {
        let test = TestStore::new();
        test.0
            .publish(Intent::Copy, &[PathBuf::from("/tmp/one")])
            .unwrap();
        let path = test.0.directory.join("register.json");
        fs::write(&path, b"broken").unwrap();
        assert!(test.0.read().is_err());
        assert!(test.0.finish_cut("old", &[]).is_err());
        assert_eq!(fs::read(path).unwrap(), b"broken");
    }

    fn selection() -> Selection {
        Selection::new(
            "revision-1".into(),
            Intent::Cut,
            &[PathBuf::from("/tmp/file")],
        )
    }

    #[test]
    fn clipboard_precedence_distinguishes_unavailable_empty_and_external() {
        let mut selection = selection();
        selection.clipboard = Some("file:///tmp/file\n".into());
        assert!(!selection.clipboard_changed(None));
        assert!(!selection.clipboard_changed(Some("file:///tmp/file")));
        assert!(selection.clipboard_changed(Some("")));
        assert!(selection.clipboard_changed(Some("file:///tmp/new\n")));
        assert!(selection.clipboard_changed(Some("ordinary text")));
        selection.clipboard = None;
        assert!(selection.clipboard_changed(Some("file:///tmp/file")));
    }

    #[test]
    fn json_round_trip() {
        let selection = selection();
        assert_eq!(
            Selection::from_json(&selection.to_json().unwrap()).unwrap(),
            selection
        );
    }

    #[test]
    fn rejects_bad_version_revision_and_relative_paths() {
        let mut selection = selection();
        selection.version = 2;
        assert!(selection.to_json().is_err());
        selection.version = 1;
        selection.revision.clear();
        assert!(selection.to_json().is_err());
        selection.revision = "valid".into();
        selection.paths = vec![NativePath::encode(Path::new("relative"))];
        assert!(selection.to_json().is_err());
    }

    #[test]
    fn rejects_malformed_and_oversized_input() {
        assert!(Selection::from_json(b"{\"version\":").is_err());
        assert!(Selection::from_json(&vec![b' '; MAX_BYTES + 1]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_paths_round_trip_without_loss() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(OsString::from_vec(b"/tmp/\xff".to_vec()));
        let selection = Selection::new("native".into(), Intent::Copy, std::slice::from_ref(&path));
        let decoded = Selection::from_json(&selection.to_json().unwrap()).unwrap();
        assert_eq!(decoded.paths[0].decode().unwrap(), path);
        assert!(NativePath::WindowsUtf16(vec![0xd800]).decode().is_err());
    }

    #[cfg(windows)]
    #[test]
    fn unpaired_surrogates_round_trip_without_loss() {
        use std::os::windows::ffi::OsStringExt;
        let path = PathBuf::from(OsString::from_wide(&[67, 58, 92, 0xd800]));
        let selection = Selection::new("native".into(), Intent::Copy, std::slice::from_ref(&path));
        let decoded = Selection::from_json(&selection.to_json().unwrap()).unwrap();
        assert_eq!(decoded.paths[0].decode().unwrap(), path);
        assert!(NativePath::UnixBytes(vec![255]).decode().is_err());
    }
}
