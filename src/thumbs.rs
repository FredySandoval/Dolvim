//! Thumbnails, rendered as unicode halfblocks.
//!
//! One worker thread decodes and downscales; the UI thread only ever reads a
//! finished cell grid. See docs/DECISIONS.md for why this is ~60 lines instead
//! of a rendering crate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

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
}

enum State {
    Pending,
    Ready(Thumb),
    Failed,
}

/// One decode asked of the worker thread.
struct ThumbRequest {
    path: PathBuf,
    cell_width: u16,
    cell_height: u16,
}

/// One finished decode coming back. `thumb` is `None` when the file could not
/// be decoded.
struct ThumbResult {
    path: PathBuf,
    cell_width: u16,
    cell_height: u16,
    thumb: Option<Thumb>,
}

pub struct Thumbs {
    cache: HashMap<ThumbKey, State>,
    /// Insertion order, for the LRU eviction that keeps memory bounded.
    order: Vec<ThumbKey>,
    tx: Sender<ThumbRequest>,
    rx: Receiver<ThumbResult>,
    inflight_decodes: Arc<Mutex<usize>>,
}

impl Thumbs {
    pub fn new() -> Thumbs {
        let (req_tx, req_rx) = channel::<ThumbRequest>();
        let (res_tx, res_rx) = channel();
        let inflight_decodes = Arc::new(Mutex::new(0usize));
        let inflight_decodes_for_worker = Arc::clone(&inflight_decodes);
        thread::spawn(move || {
            for request in req_rx {
                let thumb = decode(&request.path, request.cell_width, request.cell_height);
                if let Ok(mut inflight_guard) = inflight_decodes_for_worker.lock() {
                    *inflight_guard = inflight_guard.saturating_sub(1);
                }
                let result = ThumbResult {
                    path: request.path,
                    cell_width: request.cell_width,
                    cell_height: request.cell_height,
                    thumb,
                };
                if res_tx.send(result).is_err() {
                    return;
                }
            }
        });
        Thumbs {
            cache: HashMap::new(),
            order: Vec::new(),
            tx: req_tx,
            rx: res_rx,
            inflight_decodes,
        }
    }

    /// Collect finished work. Called once per tick.
    pub fn pump_decoded_thumbs(&mut self) {
        while let Ok(result) = self.rx.try_recv() {
            let cache_key = ThumbKey {
                path: result.path,
                cell_width: result.cell_width,
                cell_height: result.cell_height,
            };
            self.cache.insert(
                cache_key.clone(),
                match result.thumb {
                    Some(thumb) => State::Ready(thumb),
                    None => State::Failed,
                },
            );
            self.order.push(cache_key);
        }
        while self.order.len() > config::THUMB_CACHE_CAP {
            let evicted_key = self.order.remove(0);
            self.cache.remove(&evicted_key);
        }
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
        };
        if !self.cache.contains_key(&cache_key) {
            // Bound the queue: a directory of 5000 images must not enqueue
            // 5000 decodes ahead of the ones actually on screen.
            let mut claimed = false;
            if let Ok(mut inflight_guard) = self.inflight_decodes.lock() {
                claimed = *inflight_guard < config::THUMB_MAX_INFLIGHT;
                if claimed {
                    *inflight_guard += 1;
                }
            }
            if claimed {
                let _ = self.tx.send(ThumbRequest {
                    path: cache_key.path.clone(),
                    cell_width,
                    cell_height,
                });
                self.cache.insert(cache_key.clone(), State::Pending);
                self.order.push(cache_key);
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
    let decoded_image = image_reader.decode().ok()?;
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
