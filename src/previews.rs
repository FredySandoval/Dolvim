//! Small, bounded previews for the information panel.
//!
//! Preview helpers are intentionally optional: a missing external command is
//! treated exactly like an unsupported file. Results are cached so drawing a
//! frame never repeatedly launches `bat`, `pdftotext`, or an archiver.

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Clone, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    size: u64,
    mtime: i64,
    lines: usize,
}

#[derive(Default)]
struct Cache {
    key: Option<Key>,
    value: Option<Vec<Line<'static>>>,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

/// Return at most `max_lines` display lines. Unsupported and unreadable files
/// return `None`, allowing the UI to retain its normal file icon.
pub fn file(path: &Path, max_lines: usize) -> Option<Vec<Line<'static>>> {
    if max_lines == 0 || !path.is_file() {
        return None;
    }
    let metadata = fs::metadata(path).ok()?;
    let key = Key {
        path: path.to_path_buf(),
        size: metadata.len(),
        mtime: metadata.mtime(),
        lines: max_lines,
    };
    let cache = CACHE.get_or_init(|| Mutex::new(Cache::default()));
    if let Ok(guard) = cache.lock() {
        if guard.key.as_ref() == Some(&key) {
            return guard.value.clone();
        }
    }

    let value = create(path, max_lines);
    if let Ok(mut guard) = cache.lock() {
        guard.key = Some(key);
        guard.value = value.clone();
    }
    value
}

fn create(path: &Path, max_lines: usize) -> Option<Vec<Line<'static>>> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    let (output, colored) = match extension.as_str() {
        "zip" | "jar" | "apk" | "epub" => (command("unzip", &["-l"], path), false),
        "7z" | "rar" => (command("7z", &["l", "-ba"], path), false),
        "tar" | "tgz" | "tbz" | "tbz2" | "txz" | "gz" | "bz2" | "xz" => {
            (command("bsdtar", &["-tf"], path), false)
        }
        "pdf" => (
            command_with_suffix("pdftotext", &["-layout"], path, &["-"]),
            false,
        ),
        "doc" => (command("antiword", &[], path), false),
        "docx" => (command("pandoc", &["-t", "plain"], path), false),
        _ => (
            command(
                "bat",
                &[
                    "--color=always",
                    "--decorations=never",
                    "--paging=never",
                    &format!("--line-range=1:{max_lines}"),
                ],
                path,
            ),
            true,
        ),
    };
    let output = output?;
    let text = String::from_utf8_lossy(&output).replace('\t', "    ");
    let lines: Vec<Line<'static>> = text
        .lines()
        .take(max_lines)
        .map(|line| {
            if colored {
                ansi_line(line)
            } else {
                Line::raw(line.to_string())
            }
        })
        .collect();
    (!lines.is_empty()).then_some(lines)
}

/// Translate the SGR subset emitted by bat into native ratatui spans.
fn ansi_line(input: &str) -> Line<'static> {
    let mut spans = Vec::new();
    let mut style = Style::default();
    let mut rest = input;
    while let Some(start) = rest.find("\x1b[") {
        if start > 0 {
            spans.push(Span::styled(rest[..start].to_string(), style));
        }
        let sequence = &rest[start + 2..];
        let Some(end) = sequence.find('m') else {
            // Malformed escape: omit the control byte but preserve its text.
            spans.push(Span::styled(sequence.to_string(), style));
            rest = "";
            break;
        };
        apply_sgr(&mut style, &sequence[..end]);
        rest = &sequence[end + 1..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), style));
    }
    Line::from(spans)
}

fn apply_sgr(style: &mut Style, sequence: &str) {
    let values: Vec<u16> = if sequence.is_empty() {
        vec![0]
    } else {
        sequence
            .split(';')
            .filter_map(|value| value.parse().ok())
            .collect()
    };
    let mut index = 0;
    while index < values.len() {
        match values[index] {
            0 => *style = Style::default(),
            1 => *style = style.add_modifier(Modifier::BOLD),
            2 => *style = style.add_modifier(Modifier::DIM),
            3 => *style = style.add_modifier(Modifier::ITALIC),
            4 => *style = style.add_modifier(Modifier::UNDERLINED),
            22 => *style = style.remove_modifier(Modifier::BOLD | Modifier::DIM),
            23 => *style = style.remove_modifier(Modifier::ITALIC),
            24 => *style = style.remove_modifier(Modifier::UNDERLINED),
            30..=37 => *style = style.fg(basic_color(values[index] - 30, false)),
            90..=97 => *style = style.fg(basic_color(values[index] - 90, true)),
            39 => style.fg = None,
            38 if values.get(index + 1) == Some(&2) && index + 4 < values.len() => {
                *style = style.fg(Color::Rgb(
                    values[index + 2] as u8,
                    values[index + 3] as u8,
                    values[index + 4] as u8,
                ));
                index += 4;
            }
            38 if values.get(index + 1) == Some(&5) && index + 2 < values.len() => {
                *style = style.fg(Color::Indexed(values[index + 2] as u8));
                index += 2;
            }
            _ => {}
        }
        index += 1;
    }
}

fn basic_color(number: u16, bright: bool) -> Color {
    match (number, bright) {
        (0, false) => Color::Black,
        (1, false) => Color::Red,
        (2, false) => Color::Green,
        (3, false) => Color::Yellow,
        (4, false) => Color::Blue,
        (5, false) => Color::Magenta,
        (6, false) => Color::Cyan,
        (7, false) => Color::Gray,
        (0, true) => Color::DarkGray,
        (1, true) => Color::LightRed,
        (2, true) => Color::LightGreen,
        (3, true) => Color::LightYellow,
        (4, true) => Color::LightBlue,
        (5, true) => Color::LightMagenta,
        (6, true) => Color::LightCyan,
        _ => Color::White,
    }
}

fn command(program: &str, arguments: &[&str], path: &Path) -> Option<Vec<u8>> {
    command_with_suffix(program, arguments, path, &[])
}

fn command_with_suffix(
    program: &str,
    arguments: &[&str],
    path: &Path,
    suffix: &[&str],
) -> Option<Vec<u8>> {
    let output = Command::new(program)
        .args(arguments)
        .arg(path)
        .args(suffix)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bat_truecolor_sgr_becomes_styled_spans() {
        let line = ansi_line("plain \x1b[38;2;10;20;30mcode\x1b[0m end");
        assert_eq!(line.spans.len(), 3);
        assert_eq!(line.spans[1].content, "code");
        assert_eq!(line.spans[1].style.fg, Some(Color::Rgb(10, 20, 30)));
        assert_eq!(line.spans[2].style.fg, None);
    }

    #[test]
    fn basic_and_modifier_sgr_are_understood() {
        let line = ansi_line("\x1b[1;31merror\x1b[22;39m plain");
        assert_eq!(line.spans[0].style.fg, Some(Color::Red));
        assert!(line.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(line.spans[1].style.fg, None);
        assert!(!line.spans[1].style.add_modifier.contains(Modifier::BOLD));
    }
}
