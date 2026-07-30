//! The Places panel: sections and rows exactly as `docs/UI_SPEC.md` records
//! them from the screenshot.

use std::path::PathBuf;

use crate::config::glyph as g;
use crate::fs;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Dir(PathBuf),
    Trash,
    Network,
    /// A saved search: entries under `home` modified within N days.
    RecentDays(u32),
}

#[derive(Clone)]
pub enum Row {
    /// Blank line above a heading. Dolphin separates its sections with space,
    /// and a row of nothing is cheaper than a margin every renderer must know.
    Gap,
    Heading(&'static str),
    Item {
        label: String,
        glyph: &'static str,
        target: Target,
        /// (used, total) bytes — devices draw a free-space gauge behind the label.
        gauge: Option<(u64, u64)>,
        /// A partition that is not mounted: reachable only by mounting it
        /// first, so its icon is flagged rather than its row hidden.
        offline: bool,
        /// Mounted removable media, which Dolphin gives an eject affordance
        /// at the right edge of the row.
        eject: bool,
    },
}

impl Row {
    pub fn is_selectable(&self) -> bool {
        matches!(self, Row::Item { .. })
    }

    pub fn target(&self) -> Option<&Target> {
        match self {
            Row::Item { target, .. } => Some(target),
            _ => None,
        }
    }
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg_dir(key: &str, fallback: &str) -> Option<PathBuf> {
    // Respect user-dirs.dirs without writing a config parser: it is
    // `KEY="$HOME/Name"` lines, which `split_once` handles in three lines.
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    if let Ok(txt) = std::fs::read_to_string(cfg.join("user-dirs.dirs")) {
        for line in txt.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
                let v = rest.trim_matches('"');
                let p = if let Some(tail) = v.strip_prefix("$HOME/") {
                    home().join(tail)
                } else {
                    PathBuf::from(v)
                };
                return p.is_dir().then_some(p);
            }
        }
    }
    let p = home().join(fallback);
    p.is_dir().then_some(p)
}

/// Build the panel. Called at startup and whenever mounts change.
pub fn build() -> Vec<Row> {
    let mut rows = vec![Row::Heading("Places")];
    let h = home();
    rows.push(item("Home", g::HOME, Target::Dir(h.clone())));

    // The directory name doubles as the label, so there is no second spelling
    // to keep in sync — `Download` vs `Downloads` is exactly how this row went
    // missing before. These are the xdg-user-dirs defaults.
    let standard = [
        ("XDG_DESKTOP_DIR", "Desktop", g::FOLDER),
        ("XDG_DOCUMENTS_DIR", "Documents", g::DOCUMENT),
        ("XDG_DOWNLOAD_DIR", "Downloads", g::DOWNLOAD),
        ("XDG_MUSIC_DIR", "Music", g::MUSIC),
        ("XDG_PICTURES_DIR", "Pictures", g::PICTURE),
        ("XDG_VIDEOS_DIR", "Videos", g::VIDEO),
    ];
    for (key, name, gl) in standard {
        if let Some(p) = xdg_dir(key, name) {
            rows.push(item(name, gl, Target::Dir(p)));
        }
    }
    rows.push(item("Trash", g::TRASH, Target::Trash));

    section(&mut rows, "Remote");
    rows.push(item("Network", g::NETWORK, Target::Network));

    section(&mut rows, "Recent");
    rows.push(item("Modified Today", g::CLOCK, Target::RecentDays(1)));
    rows.push(item("Modified Yesterday", g::CLOCK, Target::RecentDays(2)));

    let (fixed, removable) = device_rows();
    if !fixed.is_empty() {
        section(&mut rows, "Devices");
        rows.extend(fixed);
    }
    if !removable.is_empty() {
        section(&mut rows, "Removable Devices");
        rows.extend(removable);
    }
    rows
}

/// A heading and the blank line that sets it off from the section above.
fn section(rows: &mut Vec<Row>, title: &'static str) {
    rows.push(Row::Gap);
    rows.push(Row::Heading(title));
}

fn item(label: &str, glyph: &'static str, target: Target) -> Row {
    Row::Item {
        label: label.to_string(),
        glyph,
        target,
        gauge: None,
        offline: false,
        eject: false,
    }
}

/// Split partitions the way Dolphin does: hotpluggable buses under Removable
/// Devices, everything else under Devices. Unmounted partitions are listed
/// too — Dolphin shows them, badged, because mounting one is a click away.
fn device_rows() -> (Vec<Row>, Vec<Row>) {
    let (mut fixed, mut removable) = (Vec::new(), Vec::new());
    for d in fs::devices() {
        // Dolphin names a partition by its label and falls back to its size,
        // which is the only name an unlabelled disk has.
        let label = if d.label.is_empty() {
            format!("{} Internal Drive", fs::format_size(d.size))
        } else {
            d.label.clone()
        };
        // Free space needs a mounted filesystem; an offline disk has no gauge.
        let gauge = d
            .mount
            .as_deref()
            .and_then(fs::disk_space)
            .map(|(avail, total)| (total.saturating_sub(avail), total));
        let glyph = match (&d.mount, d.removable) {
            (None, _) => g::DEVICE_OFF,
            (Some(_), true) => g::DEVICE_USB,
            (Some(_), false) => g::DEVICE,
        };
        let row = Row::Item {
            label,
            glyph,
            // Nowhere to navigate until it is mounted, so point an offline
            // disk at itself: selectable, and it fails honestly when opened.
            target: Target::Dir(d.mount.clone().unwrap_or_else(|| PathBuf::from("/"))),
            gauge,
            offline: d.mount.is_none(),
            eject: d.removable && d.mount.is_some(),
        };
        if d.removable {
            removable.push(row);
        } else {
            fixed.push(row);
        }
    }
    // Dolphin lists each section by name, not by the order the kernel happens
    // to have enumerated the buses in.
    for section in [&mut fixed, &mut removable] {
        section.sort_by(|a, b| match (a, b) {
            (Row::Item { label: x, .. }, Row::Item { label: y, .. }) => x.cmp(y),
            _ => std::cmp::Ordering::Equal,
        });
    }
    (fixed, removable)
}

/// Index of the row whose target is `path`, for highlighting the current place.
pub fn index_of(rows: &[Row], target: &Target) -> Option<usize> {
    rows.iter().position(|r| r.target() == Some(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_starts_with_places_home() {
        let rows = build();
        assert!(matches!(&rows[0], Row::Heading("Places")));
        assert!(matches!(&rows[1], Row::Item { label, .. } if label == "Home"));
    }

    #[test]
    fn headings_are_not_selectable() {
        assert!(!Row::Heading("Places").is_selectable());
    }
}
