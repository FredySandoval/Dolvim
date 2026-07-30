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
    pub w: u16,
    pub h: u16,
    /// `w * h` entries of (top_rgb, bottom_rgb).
    pub cells: Vec<([u8; 3], [u8; 3])>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    path: PathBuf,
    w: u16,
    h: u16,
}

enum State {
    Pending,
    Ready(Thumb),
    Failed,
}

pub struct Thumbs {
    cache: HashMap<Key, State>,
    /// Insertion order, for the LRU eviction that keeps memory bounded.
    order: Vec<Key>,
    tx: Sender<(PathBuf, u16, u16)>,
    rx: Receiver<(PathBuf, u16, u16, Option<Thumb>)>,
    inflight: Arc<Mutex<usize>>,
}

impl Thumbs {
    pub fn new() -> Thumbs {
        let (req_tx, req_rx) = channel::<(PathBuf, u16, u16)>();
        let (res_tx, res_rx) = channel();
        let inflight = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&inflight);
        thread::spawn(move || {
            for (path, w, h) in req_rx {
                let t = decode(&path, w, h);
                if let Ok(mut g) = counter.lock() {
                    *g = g.saturating_sub(1);
                }
                if res_tx.send((path, w, h, t)).is_err() {
                    return;
                }
            }
        });
        Thumbs {
            cache: HashMap::new(),
            order: Vec::new(),
            tx: req_tx,
            rx: res_rx,
            inflight,
        }
    }

    /// Collect finished work. Called once per tick.
    pub fn pump(&mut self) -> bool {
        let mut any = false;
        while let Ok((path, w, h, t)) = self.rx.try_recv() {
            let k = Key { path, w, h };
            self.cache.insert(
                k.clone(),
                match t {
                    Some(t) => State::Ready(t),
                    None => State::Failed,
                },
            );
            self.order.push(k);
            any = true;
        }
        while self.order.len() > config::THUMB_CACHE_CAP {
            let k = self.order.remove(0);
            self.cache.remove(&k);
        }
        any
    }

    /// Thumbnail for `path` at `w x h` cells, requesting one if absent.
    /// Returns `None` while it is still being decoded — the caller draws the
    /// glyph placeholder, and the thumbnail pops in later, like Dolphin.
    pub fn get(&mut self, path: &Path, w: u16, h: u16) -> Option<&Thumb> {
        if w == 0 || h == 0 {
            return None;
        }
        let k = Key {
            path: path.to_path_buf(),
            w,
            h,
        };
        if !self.cache.contains_key(&k) {
            // Bound the queue: a directory of 5000 images must not enqueue
            // 5000 decodes ahead of the ones actually on screen.
            let busy = self.inflight.lock().map(|g| *g).unwrap_or(0);
            if busy < 32 {
                if let Ok(mut g) = self.inflight.lock() {
                    *g += 1;
                }
                let _ = self.tx.send((k.path.clone(), w, h));
                self.cache.insert(k.clone(), State::Pending);
                self.order.push(k);
            }
            return None;
        }
        match self.cache.get(&k) {
            Some(State::Ready(t)) => Some(t),
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
    let (pw, ph) = (w as u32, h as u32 * 2);
    let img = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let img = img.decode().ok()?;
    let img = img.thumbnail(pw, ph).to_rgb8();
    let (iw, ih) = (img.width(), img.height());
    if iw == 0 || ih == 0 {
        return None;
    }
    // Centre the (aspect-preserved) image in the requested cell box.
    let cw = iw.min(pw);
    let ch = ih.min(ph);
    let cells_w = cw as u16;
    let cells_h = (ch as u16).div_ceil(2);
    let mut cells = Vec::with_capacity(cells_w as usize * cells_h as usize);
    for cy in 0..cells_h {
        for cx in 0..cells_w {
            let top = px(&img, cx as u32, cy as u32 * 2);
            let bot = px(&img, cx as u32, cy as u32 * 2 + 1);
            cells.push((top, bot));
        }
    }
    Some(Thumb {
        w: cells_w,
        h: cells_h,
        cells,
    })
}

fn px(img: &image::RgbImage, x: u32, y: u32) -> [u8; 3] {
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
        let d = std::env::temp_dir().join(format!("dolvin-thumb-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("t.png");
        let img = image::RgbImage::from_pixel(8, 8, image::Rgb([1, 2, 3]));
        img.save(&p).unwrap();

        let t = decode(&p, 4, 2).unwrap();
        assert_eq!((t.w, t.h), (4, 2));
        assert_eq!(t.cells.len(), 8);
        assert_eq!(t.cells[0].0, [1, 2, 3]);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn undecodable_files_fail_without_panicking() {
        let d = std::env::temp_dir().join(format!("dolvin-thumb2-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("not-an-image.png");
        std::fs::write(&p, b"nope").unwrap();
        assert!(decode(&p, 4, 2).is_none());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
