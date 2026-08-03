//! The Places panel: sections and rows exactly as `docs/UI_SPEC.md` records
//! them from the screenshot.

use std::path::PathBuf;

use crate::config::glyph;
use crate::fs;

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Dir(PathBuf),
    Trash,
    Network,
    /// A saved search: entries under `home` modified within N days.
    RecentDays(u32),
}

/// How much of a device is in use, for the Places free-space gauge.
#[derive(Clone, Copy)]
pub struct DiskUsage {
    pub used_bytes: u64,
    pub total_bytes: u64,
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
        /// Devices draw a free-space gauge behind the label.
        gauge: Option<DiskUsage>,
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
    let config_dir = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"));
    if let Ok(user_dirs_text) = std::fs::read_to_string(config_dir.join("user-dirs.dirs")) {
        for line in user_dirs_text.lines() {
            let line = line.trim();
            if let Some(after_key) = line
                .strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
            {
                let quoted_path = after_key.trim_matches('"');
                let xdg_path = if let Some(path_after_home) = quoted_path.strip_prefix("$HOME/") {
                    home().join(path_after_home)
                } else {
                    PathBuf::from(quoted_path)
                };
                return xdg_path.is_dir().then_some(xdg_path);
            }
        }
    }
    let fallback_path = home().join(fallback);
    fallback_path.is_dir().then_some(fallback_path)
}

/// Build the panel. Called at startup and whenever mounts change.
pub fn build() -> Vec<Row> {
    let mut rows = vec![Row::Heading("Places")];
    let home_dir = home();
    rows.push(place_row(
        "Home",
        glyph::HOME,
        Target::Dir(home_dir.clone()),
    ));

    // The directory name doubles as the label, so there is no second spelling
    // to keep in sync — `Download` vs `Downloads` is exactly how this row went
    // missing before.
    for xdg_dir_row in crate::config::XDG_DIRS {
        if let Some(p) = xdg_dir(xdg_dir_row.env_key, xdg_dir_row.name) {
            rows.push(place_row(
                xdg_dir_row.name,
                xdg_dir_row.glyph,
                Target::Dir(p),
            ));
        }
    }
    rows.push(place_row("Trash", glyph::TRASH, Target::Trash));

    section(&mut rows, "Remote");
    rows.push(place_row("Network", glyph::NETWORK, Target::Network));

    section(&mut rows, "Recent");
    rows.push(place_row(
        "Modified Today",
        glyph::CLOCK,
        Target::RecentDays(1),
    ));
    rows.push(place_row(
        "Modified Yesterday",
        glyph::CLOCK,
        Target::RecentDays(2),
    ));

    let device_rows = device_rows();
    if !device_rows.fixed.is_empty() {
        section(&mut rows, "Devices");
        rows.extend(device_rows.fixed);
    }
    if !device_rows.removable.is_empty() {
        section(&mut rows, "Removable Devices");
        rows.extend(device_rows.removable);
    }
    rows
}

/// A heading and the blank line that sets it off from the section above.
fn section(rows: &mut Vec<Row>, title: &'static str) {
    rows.push(Row::Gap);
    rows.push(Row::Heading(title));
}

fn place_row(label: &str, glyph: &'static str, target: Target) -> Row {
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
struct DeviceRows {
    fixed: Vec<Row>,
    removable: Vec<Row>,
}

fn device_rows() -> DeviceRows {
    let (mut fixed, mut removable) = (Vec::new(), Vec::new());
    for device in fs::devices() {
        // Dolphin names a partition by its label and falls back to its size,
        // which is the only name an unlabelled disk has.
        let label = if device.label.is_empty() {
            format!("{} Internal Drive", fs::format_size(device.size))
        } else {
            device.label.clone()
        };
        // Free space needs a mounted filesystem; an offline disk has no gauge.
        let gauge = device
            .mount
            .as_deref()
            .and_then(fs::disk_space)
            .map(|space| DiskUsage {
                used_bytes: space.total_bytes.saturating_sub(space.available_bytes),
                total_bytes: space.total_bytes,
            });
        let device_glyph = match (&device.mount, device.removable) {
            (None, _) => glyph::DEVICE_OFF,
            (Some(_), true) => glyph::DEVICE_USB,
            (Some(_), false) => glyph::DEVICE,
        };
        let row = Row::Item {
            label,
            glyph: device_glyph,
            // Nowhere to navigate until it is mounted, so point an offline
            // disk at itself: selectable, and it fails honestly when opened.
            target: Target::Dir(device.mount.clone().unwrap_or_else(|| PathBuf::from("/"))),
            gauge,
            offline: device.mount.is_none(),
            eject: device.removable && device.mount.is_some(),
        };
        if device.removable {
            removable.push(row);
        } else {
            fixed.push(row);
        }
    }
    // Dolphin lists each section by name, not by the order the kernel happens
    // to have enumerated the buses in.
    for group in [&mut fixed, &mut removable] {
        group.sort_by(|a, b| match (a, b) {
            (
                Row::Item {
                    label: left_label, ..
                },
                Row::Item {
                    label: right_label, ..
                },
            ) => left_label.cmp(right_label),
            _ => std::cmp::Ordering::Equal,
        });
    }
    DeviceRows { fixed, removable }
}

/// Index of the row whose target is `target`, for highlighting the current place.
pub fn index_of(rows: &[Row], target: &Target) -> Option<usize> {
    rows.iter().position(|row| row.target() == Some(target))
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
