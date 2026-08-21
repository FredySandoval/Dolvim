//! Thumbnails, rendered as unicode halfblocks.
//!
//! One worker thread decodes and downscales; the UI thread only ever reads a
//! finished cell grid. See docs/DECISIONS.md for why this is ~60 lines instead
//! of a rendering crate.

use std::collections::{HashMap, VecDeque};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use crate::config;

/// A decoded thumbnail as terminal cells. Each cell is the upper half-block
/// `▀`: `fg` paints the top pixel row, `bg` the bottom one, so one cell
/// carries two pixels of vertical resolution.
#[derive(Clone)]
pub struct Thumb {
    pub cell_width: u16,
    pub cell_height: u16,
    /// `cell_width * cell_height` entries of (top_rgb, bottom_rgb).
    pub cells: Vec<([u8; 3], [u8; 3])>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct ThumbKey {
    path: PathBuf,
    cell_width: u16,
    cell_height: u16,
    generation: Option<FileGeneration>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct FileGeneration {
    device: u64,
    inode: u64,
    size: u64,
    mtime_seconds: i64,
    mtime_nanoseconds: i64,
}

fn file_generation(path: &Path) -> Option<FileGeneration> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(FileGeneration {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        mtime_seconds: metadata.mtime(),
        mtime_nanoseconds: metadata.mtime_nsec(),
    })
}

enum State {
    Pending,
    Ready(Thumb),
    Failed,
}

/// One decode asked of the worker thread.
struct ThumbRequest {
    key: ThumbKey,
}

/// One finished decode coming back. `thumb` is `None` when the file could not
/// be decoded.
struct ThumbResult {
    key: ThumbKey,
    thumb: Option<Thumb>,
}

fn spawn_worker() -> (Sender<ThumbRequest>, Receiver<ThumbResult>, JoinHandle<()>) {
    let (req_tx, req_rx) = channel::<ThumbRequest>();
    let (res_tx, res_rx) = channel();
    let worker = thread::spawn(move || {
        for request in req_rx {
            let thumb = decode(
                &request.key.path,
                request.key.cell_width,
                request.key.cell_height,
            );
            let result = ThumbResult {
                key: request.key,
                thumb,
            };
            if res_tx.send(result).is_err() {
                return;
            }
        }
    });
    (req_tx, res_rx, worker)
}

pub struct Thumbs {
    cache: HashMap<ThumbKey, State>,
    /// Completion order, for FIFO eviction that keeps memory bounded.
    order: VecDeque<ThumbKey>,
    tx: Sender<ThumbRequest>,
    rx: Receiver<ThumbResult>,
    inflight_decodes: usize,
    worker: Option<JoinHandle<()>>,
    worker_failed: bool,
}

impl Thumbs {
    pub fn new() -> Thumbs {
        let (req_tx, res_rx, worker) = spawn_worker();
        Thumbs {
            cache: HashMap::new(),
            order: VecDeque::new(),
            tx: req_tx,
            rx: res_rx,
            inflight_decodes: 0,
            worker: Some(worker),
            worker_failed: false,
        }
    }

    /// Collect finished work. Called once per tick.
    pub fn pump_decoded_thumbs(&mut self) -> bool {
        let mut changed = false;
        loop {
            match self.rx.try_recv() {
                Ok(result) => {
                    changed = true;
                    self.inflight_decodes = self.inflight_decodes.saturating_sub(1);
                    let cache_key = result.key;
                    self.cache.insert(
                        cache_key.clone(),
                        match result.thumb {
                            Some(thumb) => State::Ready(thumb),
                            None => State::Failed,
                        },
                    );
                    self.order.push_back(cache_key);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    let pending_before = self.cache.len();
                    self.cache
                        .retain(|_, state| !matches!(state, State::Pending));
                    changed |= self.cache.len() != pending_before;
                    self.inflight_decodes = 0;
                    self.worker_failed = true;
                    if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                        let _ = self.worker.take().expect("worker was present").join();
                    }
                    let (tx, rx, worker) = spawn_worker();
                    self.tx = tx;
                    self.rx = rx;
                    self.worker = Some(worker);
                    break;
                }
            }
        }
        while self.order.len() > config::THUMB_CACHE_CAP {
            if let Some(evicted_key) = self.order.pop_front() {
                self.cache.remove(&evicted_key);
            }
        }
        changed
    }

    pub fn take_worker_failure(&mut self) -> bool {
        std::mem::take(&mut self.worker_failed)
    }

    /// Thumbnail for `path` at `w x h` cells, requesting one if absent.
    /// Returns `None` while it is still being decoded — the caller draws the
    /// glyph placeholder, and the thumbnail pops in later, like Dolphin.
    pub fn get_or_request(
        &mut self,
        path: &Path,
        cell_width: u16,
        cell_height: u16,
    ) -> Option<&Thumb> {
        if cell_width == 0 || cell_height == 0 {
            return None;
        }
        let cache_key = ThumbKey {
            path: path.to_path_buf(),
            cell_width,
            cell_height,
            generation: file_generation(path),
        };
        if !self.cache.contains_key(&cache_key) {
            self.cache.retain(|key, _| {
                key.path != cache_key.path
                    || key.cell_width != cell_width
                    || key.cell_height != cell_height
            });
            self.order.retain(|key| {
                key.path != cache_key.path
                    || key.cell_width != cell_width
                    || key.cell_height != cell_height
            });
            // Bound the queue: a directory of 5000 images must not enqueue
            // 5000 decodes ahead of the ones actually on screen.
            if self.inflight_decodes < config::THUMB_MAX_INFLIGHT
                && self
                    .tx
                    .send(ThumbRequest {
                        key: cache_key.clone(),
                    })
                    .is_ok()
            {
                self.inflight_decodes += 1;
                self.cache.insert(cache_key, State::Pending);
            }
            return None;
        }
        match self.cache.get(&cache_key) {
            Some(State::Ready(thumb)) => Some(thumb),
            _ => None,
        }
    }
}

impl Default for Thumbs {
    fn default() -> Self {
        Thumbs::new()
    }
}

fn decode(path: &Path, w: u16, h: u16) -> Option<Thumb> {
    // Two pixel rows per cell row.
    let (target_width_px, target_height_px) = (w as u32, h as u32 * 2);
    let image_reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let (source_width, source_height) = image_reader.into_dimensions().ok()?;
    if !dimensions_allowed(source_width, source_height) {
        return None;
    }
    let decoded_image = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;
    let rgb_thumbnail = decoded_image
        .thumbnail(target_width_px, target_height_px)
        .to_rgb8();
    let (image_width_px, image_height_px) = (rgb_thumbnail.width(), rgb_thumbnail.height());
    if image_width_px == 0 || image_height_px == 0 {
        return None;
    }
    // Centre the (aspect-preserved) image in the requested cell box.
    let clipped_width_px = image_width_px.min(target_width_px);
    let clipped_height_px = image_height_px.min(target_height_px);
    let cells_w = clipped_width_px as u16;
    let cells_h = (clipped_height_px as u16).div_ceil(2);
    let mut cells = Vec::with_capacity(cells_w as usize * cells_h as usize);
    for cy in 0..cells_h {
        for cx in 0..cells_w {
            let top = pixel_or_white(&rgb_thumbnail, cx as u32, cy as u32 * 2);
            let bot = pixel_or_white(&rgb_thumbnail, cx as u32, cy as u32 * 2 + 1);
            cells.push((top, bot));
        }
    }
    Some(Thumb {
        cell_width: cells_w,
        cell_height: cells_h,
        cells,
    })
}

fn dimensions_allowed(width: u32, height: u32) -> bool {
    width != 0
        && height != 0
        && u64::from(width)
            .checked_mul(u64::from(height))
            .is_some_and(|pixels| pixels <= config::THUMB_MAX_PIXELS)
}

fn pixel_or_white(img: &image::RgbImage, x: u32, y: u32) -> [u8; 3] {
    if x >= img.width() || y >= img.height() {
        return [255, 255, 255];
    }
    img.get_pixel(x, y).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn successful_request_is_counted_once_and_not_added_to_completion_order() {
        let mut thumbs = Thumbs::new();

        assert!(thumbs
            .get_or_request(Path::new("missing.png"), 4, 2)
            .is_none());
        assert_eq!(thumbs.inflight_decodes, 1);
        assert!(matches!(thumbs.cache.values().next(), Some(State::Pending)));
        assert!(thumbs.order.is_empty());
    }

    #[test]
    fn failed_request_send_does_not_claim_an_inflight_slot_or_cache_entry() {
        let mut thumbs = Thumbs::new();
        let (disconnected_tx, disconnected_rx) = channel();
        drop(disconnected_rx);
        thumbs.tx = disconnected_tx;

        assert!(thumbs
            .get_or_request(Path::new("missing.png"), 4, 2)
            .is_none());
        assert_eq!(thumbs.inflight_decodes, 0);
        assert!(thumbs.cache.is_empty());
        assert!(thumbs.order.is_empty());
    }

    #[test]
    fn disconnect_clears_pending_work_and_recovers_capacity() {
        let mut thumbs = Thumbs::new();
        thumbs.cache.insert(
            ThumbKey {
                path: "stranded.png".into(),
                cell_width: 4,
                cell_height: 2,
                generation: None,
            },
            State::Pending,
        );
        thumbs.inflight_decodes = config::THUMB_MAX_INFLIGHT;
        let (disconnected_tx, disconnected_rx) = channel();
        drop(disconnected_tx);
        thumbs.rx = disconnected_rx;

        assert!(thumbs.pump_decoded_thumbs());
        assert!(thumbs.cache.is_empty());
        assert_eq!(thumbs.inflight_decodes, 0);
        assert!(thumbs.take_worker_failure());
        assert!(!thumbs.take_worker_failure());

        assert!(thumbs
            .get_or_request(Path::new("missing-after-restart.png"), 4, 2)
            .is_none());
        assert_eq!(thumbs.inflight_decodes, 1);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while thumbs.inflight_decodes != 0 {
            thumbs.pump_decoded_thumbs();
            assert!(
                std::time::Instant::now() < deadline,
                "restarted worker stalled"
            );
            std::thread::yield_now();
        }
        assert!(matches!(thumbs.cache.values().next(), Some(State::Failed)));
    }

    #[test]
    fn replacement_invalidates_ready_and_failed_cache_entries() {
        let dir =
            std::env::temp_dir().join(format!("dolvim-thumb-generation-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("same.png");
        std::fs::write(&path, b"bad").unwrap();
        let first = file_generation(&path);

        let mut thumbs = Thumbs::new();
        let stale_key = ThumbKey {
            path: path.clone(),
            cell_width: 4,
            cell_height: 2,
            generation: first,
        };
        thumbs.cache.insert(stale_key.clone(), State::Failed);
        thumbs.order.push_back(stale_key);
        std::fs::write(&path, b"different and longer").unwrap();

        assert!(thumbs.get_or_request(&path, 4, 2).is_none());
        assert_eq!(thumbs.cache.len(), 1);
        assert!(thumbs.cache.keys().all(|key| key.generation != first));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn oversized_or_empty_images_are_rejected_before_decode() {
        assert!(!dimensions_allowed(0, 100));
        assert!(!dimensions_allowed(u32::MAX, u32::MAX));
        assert!(!dimensions_allowed(config::THUMB_MAX_PIXELS as u32 + 1, 1));
        assert!(dimensions_allowed(100, 100));
    }

    #[test]
    fn a_decoded_thumb_has_one_cell_per_two_pixel_rows() {
        let temp_dir = std::env::temp_dir().join(format!("dolvim-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let image_path = temp_dir.join("t.png");
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]));
        img.save(&image_path).unwrap();

        let thumb = decode(&image_path, 4, 2).unwrap();
        assert_eq!((thumb.cell_width, thumb.cell_height), (4, 2));
        assert_eq!(thumb.cells.len(), 8);
        assert_eq!(thumb.cells[0].0, [1, 2, 3]);
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn undecodable_files_fail_without_panicking() {
        let temp_dir = std::env::temp_dir().join(format!("dolvim-thumb2-{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let image_path = temp_dir.join("not-an-image.png");
        std::fs::write(&image_path, b"nope").unwrap();
        assert!(decode(&image_path, 4, 2).is_none());
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }
}
