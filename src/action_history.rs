//! Persistent completed-operation history. This is an audit view, not undo state.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::shared_register::NativePath;

pub const VERSION: u32 = 1;
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_LOG_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Operation {
    Copy,
    Move,
    Rename,
    Trash,
    Restore,
    CreateFile,
    CreateDirectory,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ItemStatus {
    Succeeded,
    Failed,
    Incomplete,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Item {
    pub status: ItemStatus,
    pub source: NativePath,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destination: Option<NativePath>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub version: u32,
    pub id: String,
    pub completed_unix_ms: u64,
    pub operation: Operation,
    pub cancelled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<String>,
    pub items: Vec<Item>,
}

impl Record {
    pub fn summary(&self) -> String {
        let verb = match self.operation {
            Operation::Copy => "Copied",
            Operation::Move => "Moved",
            Operation::Rename => "Renamed",
            Operation::Trash => "Trashed",
            Operation::Restore => "Restored",
            Operation::CreateFile => "Created file",
            Operation::CreateDirectory => "Created folder",
        };
        let name = self
            .items
            .first()
            .and_then(|item| item.source.decode().ok())
            .map(|path| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_default();
        let succeeded = self
            .items
            .iter()
            .filter(|item| item.status == ItemStatus::Succeeded)
            .count();
        let result = if self.cancelled {
            "Cancelled".to_string()
        } else if !self.diagnostics.is_empty() {
            "Needs attention".to_string()
        } else if succeeded == self.items.len() {
            "Completed".to_string()
        } else {
            format!("{succeeded}/{} succeeded", self.items.len())
        };
        format!(
            "{verb} {name}{} — {result}",
            if self.items.len() > 1 {
                format!(" (+{})", self.items.len() - 1)
            } else {
                String::new()
            }
        )
    }

    pub fn completed(operation: Operation, cancelled: bool, items: Vec<Item>) -> io::Result<Self> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?;
        Ok(Self {
            version: VERSION,
            id: format!("{}-{}", std::process::id(), now.as_nanos()),
            completed_unix_ms: now.as_millis().try_into().unwrap_or(u64::MAX),
            operation,
            cancelled,
            diagnostics: Vec::new(),
            items,
        })
    }

    fn validate(&self) -> io::Result<()> {
        if self.version != VERSION || self.id.is_empty() || self.items.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid history record",
            ));
        }
        if self.items.iter().any(|item| {
            (item.status == ItemStatus::Failed && item.error.is_none())
                || (item.status == ItemStatus::Succeeded && item.error.is_some())
                || (item.status == ItemStatus::Succeeded
                    && item.destination.is_none()
                    && matches!(
                        self.operation,
                        Operation::Copy | Operation::Move | Operation::Rename | Operation::Restore
                    ))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid history item outcome",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Store {
    directory: PathBuf,
}

impl Store {
    pub fn for_app() -> Option<Self> {
        // Unit tests inject isolated stores and must not touch user state.
        if cfg!(test) {
            return None;
        }
        let directory = match std::env::var_os("XDG_STATE_HOME") {
            Some(value) if Path::new(&value).is_absolute() => PathBuf::from(value).join("dolvim"),
            _ => std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/"))
                .join(".local/state/dolvim"),
        };
        Self::new(directory).ok()
    }

    pub fn new(directory: PathBuf) -> io::Result<Self> {
        Ok(Self { directory })
    }

    fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.directory)?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.directory.join("history.lock"))?;
        file.lock()?;
        Ok(file)
    }

    pub fn append(&self, record: &Record) -> io::Result<()> {
        self.append_with_limit(record, MAX_LOG_BYTES)
    }

    fn append_with_limit(&self, record: &Record, log_limit: usize) -> io::Result<()> {
        record.validate()?;
        let mut line = serde_json::to_vec(record).map_err(io::Error::other)?;
        if line.len() > MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "history record exceeds 8 MiB",
            ));
        }
        line.push(b'\n');
        let _lock = self.lock()?;
        let (mut records, valid_end, unterminated) = self.read_locked()?;
        let path = self.directory.join("history.jsonl");
        if valid_end as usize + line.len() > log_limit {
            // Compact under the stable lock. Keep a bounded newest suffix,
            // leaving room for subsequent ordinary appends.
            records.push(record.clone());
            let mut retained = Vec::new();
            let mut bytes = 0;
            for entry in records.iter().rev().take(10_000) {
                let mut encoded = serde_json::to_vec(entry).map_err(io::Error::other)?;
                encoded.push(b'\n');
                if bytes + encoded.len() > log_limit / 2 {
                    break;
                }
                bytes += encoded.len();
                retained.push(encoded);
            }
            let pending = self.directory.join("history.pending");
            let mut options = OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&pending)?;
            for encoded in retained.iter().rev() {
                file.write_all(encoded)?;
            }
            file.sync_all()?;
            fs::rename(pending, path)?;
            File::open(&self.directory)?.sync_all()
        } else {
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            // Remove crash residue before appending, so it cannot become an
            // invalid middle line and hide future valid records.
            file.set_len(valid_end)?;
            if unterminated {
                file.write_all(b"\n")?;
            }
            file.write_all(&line)?;
            file.sync_data()
        }
    }

    /// Reads oldest-first. A truncated or malformed final line is ignored,
    /// while corruption before the final line remains an explicit error.
    pub fn read(&self) -> io::Result<Vec<Record>> {
        let _lock = self.lock()?;
        self.read_locked().map(|(records, _, _)| records)
    }

    fn read_locked(&self) -> io::Result<(Vec<Record>, u64, bool)> {
        let file = match File::open(self.directory.join("history.jsonl")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok((Vec::new(), 0, false))
            }
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take((MAX_LOG_BYTES + MAX_LINE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_LOG_BYTES + MAX_LINE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "history file exceeds read limit",
            ));
        }
        let mut records = Vec::new();
        let mut valid_end = 0;
        let mut offset = 0;
        for line in bytes.split_inclusive(|byte| *byte == b'\n') {
            let data = line.strip_suffix(b"\n").unwrap_or(line);
            if data.len() > MAX_LINE_BYTES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "history line exceeds 8 MiB",
                ));
            }
            if !data.is_empty() {
                let result = serde_json::from_slice::<Record>(data)
                    .map_err(io::Error::other)
                    .and_then(|record| {
                        record.validate()?;
                        Ok(record)
                    });
                match result {
                    Ok(record) => records.push(record),
                    Err(_) if offset + line.len() == bytes.len() => break,
                    Err(error) => return Err(io::Error::new(io::ErrorKind::InvalidData, error)),
                }
            }
            offset += line.len();
            valid_end = offset;
        }
        let unterminated = valid_end > 0 && bytes[valid_end - 1] != b'\n';
        Ok((records, valid_end as u64, unterminated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(label: &str) -> (Store, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "dolvim-history-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        (Store::new(root.clone()).unwrap(), root)
    }

    fn record(path: &Path) -> Record {
        Record::completed(
            Operation::CreateFile,
            false,
            vec![Item {
                status: ItemStatus::Succeeded,
                source: NativePath::encode(path),
                destination: None,
                error: None,
            }],
        )
        .unwrap()
    }

    #[test]
    fn append_round_trips_native_paths() {
        let (store, root) = store("round-trip");
        let expected = record(Path::new("/tmp/example"));
        store.append(&expected).unwrap();
        assert_eq!(store.read().unwrap(), vec![expected]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn truncated_final_line_does_not_hide_valid_records() {
        let (store, root) = store("truncated");
        let expected = record(Path::new("/tmp/example"));
        store.append(&expected).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join("history.jsonl"))
            .unwrap();
        file.write_all(b"{\"version\":1").unwrap();
        assert_eq!(store.read().unwrap(), vec![expected]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn append_repairs_crash_tail_before_new_record() {
        let (store, root) = store("repair");
        let first = record(Path::new("/tmp/first"));
        let second = record(Path::new("/tmp/second"));
        store.append(&first).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join("history.jsonl"))
            .unwrap();
        file.write_all(b"{broken").unwrap();
        store.append(&second).unwrap();
        assert_eq!(store.read().unwrap(), vec![first, second]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_handles_do_not_interleave_records() {
        let (store, root) = store("concurrent");
        let workers: Vec<_> = (0..4)
            .map(|worker| {
                let store = store.clone();
                std::thread::spawn(move || {
                    for index in 0..20 {
                        store
                            .append(&record(Path::new(&format!("/tmp/{worker}-{index}"))))
                            .unwrap();
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(store.read().unwrap().len(), 80);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retention_keeps_a_bounded_newest_suffix() {
        let (store, root) = store("retention");
        let mut last = record(Path::new("/tmp/first"));
        for index in 0..40 {
            last = record(Path::new(&format!("/tmp/{index}")));
            store.append_with_limit(&last, 4096).unwrap();
        }
        let records = store.read().unwrap();
        assert_eq!(records.last(), Some(&last));
        assert!(records.len() < 40);
        assert!(fs::metadata(root.join("history.jsonl")).unwrap().len() <= 4096);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_middle_line_is_reported() {
        let (store, root) = store("malformed");
        let expected = record(Path::new("/tmp/example"));
        store.append(&expected).unwrap();
        let mut file = OpenOptions::new()
            .append(true)
            .open(root.join("history.jsonl"))
            .unwrap();
        file.write_all(b"broken\n{}\n").unwrap();
        assert_eq!(store.read().unwrap_err().kind(), io::ErrorKind::InvalidData);
        fs::remove_dir_all(root).unwrap();
    }
}
