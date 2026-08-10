//! MIME-aware routing and process launching for files opened outside Dolvim.
//!
//! Policy and execution deliberately live together here rather than in
//! filesystem mutation code. Resolution happens before `xdg-open` is invoked,
//! preventing xdg-utils from silently falling back to a web browser when the
//! desktop has no association for a file.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use crate::{config, places};

/// A fully resolved external command. The caller only decides whether Dolvim
/// must hand over its terminal before running it.
pub struct Plan {
    program: OsString,
    args: Vec<OsString>,
    terminal: bool,
}

impl Plan {
    pub fn needs_terminal(&self) -> bool {
        self.terminal
    }

    /// Start a graphical handler without allowing it to inherit the alternate
    /// screen. A small waiter thread reaps the short-lived launcher process.
    pub fn spawn_detached(self) -> Result<Receiver<Result<(), String>>, String> {
        let program = self.program.to_string_lossy().into_owned();
        let child = self
            .command()
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start {program}: {error}"))?;
        let (result_tx, result_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = match child.wait_with_output() {
                Ok(output) if output.status.success() => Ok(()),
                Ok(output) => {
                    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                    if stderr.is_empty() {
                        Err(format!("{program} exited with {}", output.status))
                    } else {
                        Err(format!("{program}: {stderr}"))
                    }
                }
                Err(error) => Err(format!("Could not wait for {program}: {error}")),
            };
            let _ = result_tx.send(result);
        });
        Ok(result_rx)
    }

    /// Run a terminal handler while Dolvim's terminal is suspended.
    pub fn run_foreground(&self) -> Result<(), String> {
        let program = self.program.to_string_lossy();
        let status = self
            .command()
            .status()
            .map_err(|error| format!("Could not start {program}: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{program} exited with {status}"))
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

#[derive(Debug)]
pub enum Error {
    MimeQuery(String),
    InvalidEditor {
        variable: &'static str,
        reason: String,
    },
    NoAssociation(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MimeQuery(reason) => {
                write!(formatter, "Could not query file associations: {reason}")
            }
            Self::InvalidEditor { variable, reason } => {
                write!(formatter, "Invalid ${variable} command: {reason}")
            }
            Self::NoAssociation(mime) => write!(formatter, "No application associated with {mime}"),
        }
    }
}

/// Resolve the desktop association or the editor fallback for `path`.
pub fn resolve(path: &Path) -> Result<Plan, Error> {
    let mime = query_mime(&[
        OsStr::new("query"),
        OsStr::new("filetype"),
        path.as_os_str(),
    ])?
    .ok_or_else(|| Error::MimeQuery("xdg-mime returned no MIME type".into()))?;
    validate_mime(&mime)?;
    let default = query_mime(&[
        OsStr::new("query"),
        OsStr::new("default"),
        OsStr::new(&mime),
    ])?;
    if let Some(desktop_id) = default.as_deref() {
        validate_desktop_id(desktop_id)?;
    }

    match route(&mime, default.as_deref())? {
        Route::System(desktop_id) => Ok(Plan {
            program: "xdg-open".into(),
            args: vec![path.as_os_str().to_owned()],
            terminal: desktop_entry(desktop_id).is_some_and(|entry| terminal_entry(&entry)),
        }),
        Route::Editor => editor_plan(path),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Route<'a> {
    System(&'a str),
    Editor,
}

fn route<'a>(mime: &str, default: Option<&'a str>) -> Result<Route<'a>, Error> {
    if let Some(desktop_id) = default.filter(|id| !id.trim().is_empty()) {
        return Ok(Route::System(desktop_id));
    }
    if is_editor_mime(mime) {
        Ok(Route::Editor)
    } else {
        Err(Error::NoAssociation(mime.to_owned()))
    }
}

fn is_editor_mime(mime: &str) -> bool {
    mime.starts_with("text/")
        || config::EDITOR_MIME_TYPES.contains(&mime)
        || config::EDITOR_MIME_SUFFIXES
            .iter()
            .any(|suffix| mime.ends_with(suffix))
}

fn validate_mime(mime: &str) -> Result<(), Error> {
    let valid = mime.is_ascii()
        && mime.bytes().all(|byte| byte.is_ascii_graphic())
        && mime.split_once('/').is_some_and(|(kind, subtype)| {
            !kind.is_empty() && !subtype.is_empty() && !subtype.contains('/')
        });
    if valid {
        Ok(())
    } else {
        Err(Error::MimeQuery(format!(
            "xdg-mime returned an invalid MIME type: {mime:?}"
        )))
    }
}

fn validate_desktop_id(id: &str) -> Result<(), Error> {
    let valid = id.ends_with(".desktop")
        && id.chars().all(|character| !character.is_control())
        && !id.contains(['/', '\\'])
        && id != ".desktop";
    if valid {
        Ok(())
    } else {
        Err(Error::MimeQuery(format!(
            "xdg-mime returned an invalid desktop ID: {id:?}"
        )))
    }
}

fn editor_plan(path: &Path) -> Result<Plan, Error> {
    let visual = std::env::var("VISUAL").ok();
    let editor = std::env::var("EDITOR").ok();
    editor_plan_with(path, visual.as_deref(), editor.as_deref())
}

fn editor_plan_with(
    path: &Path,
    visual: Option<&str>,
    editor: Option<&str>,
) -> Result<Plan, Error> {
    for (variable, value) in [("VISUAL", visual), ("EDITOR", editor)] {
        let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
            continue;
        };
        let words = shell_words::split(value).map_err(|error| Error::InvalidEditor {
            variable,
            reason: error.to_string(),
        })?;
        if let Some((program, args)) = words.split_first() {
            return Ok(editor_plan_from_parts(program, args, path));
        }
    }
    Ok(editor_plan_from_parts("vi", &[], path))
}

fn editor_plan_from_parts(program: &str, args: &[String], path: &Path) -> Plan {
    let mut args: Vec<OsString> = args.iter().map(OsString::from).collect();
    args.push(path.as_os_str().to_owned());
    Plan {
        program: program.into(),
        args,
        // Editors are given the terminal even if a user's chosen editor turns
        // out to be graphical. Waiting briefly is safe; sharing a raw terminal
        // with a TUI editor is not.
        terminal: true,
    }
}

fn query_mime(args: &[&OsStr]) -> Result<Option<String>, Error> {
    let output = Command::new("xdg-mime")
        .args(args)
        .output()
        .map_err(|error| Error::MimeQuery(format!("xdg-mime could not start: {error}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let detail = if stderr.is_empty() {
            format!("xdg-mime exited with {}", output.status)
        } else {
            format!("xdg-mime: {stderr}")
        };
        return Err(Error::MimeQuery(detail));
    }
    let value = String::from_utf8(output.stdout)
        .map_err(|_| Error::MimeQuery("xdg-mime returned non-UTF-8 output".into()))?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn desktop_entry(id: &str) -> Option<String> {
    data_dirs()
        .into_iter()
        .find_map(|dir| fs::read_to_string(dir.join("applications").join(id)).ok())
}

fn data_dirs() -> Vec<PathBuf> {
    let user = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| places::home().join(".local/share"));
    let system = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::iter::once(user)
        .chain(
            system
                .split(':')
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute()),
        )
        .collect()
}

/// Read `Terminal=` only from the main desktop-entry group. Action-specific or
/// unrelated groups must not change how the default application is launched.
fn terminal_entry(contents: &str) -> bool {
    let mut in_desktop_entry = false;
    let mut saw_desktop_entry = false;
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            if saw_desktop_entry {
                break;
            }
            in_desktop_entry = line.eq_ignore_ascii_case("[Desktop Entry]");
            saw_desktop_entry = in_desktop_entry;
            continue;
        }
        if !in_desktop_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("Terminal") {
            return value.trim().eq_ignore_ascii_case("true");
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registered_default_always_wins() {
        assert_eq!(
            route("text/plain", Some("writer.desktop")).unwrap(),
            Route::System("writer.desktop")
        );
        assert_eq!(
            route("application/octet-stream", Some("hex.desktop")).unwrap(),
            Route::System("hex.desktop")
        );
    }

    #[test]
    fn text_and_source_without_a_default_use_editor_role() {
        for mime in [
            "text/plain",
            "text/x-rust",
            "application/json",
            "application/javascript",
            "application/yaml",
            "application/ld+json",
            "inode/x-empty",
        ] {
            assert_eq!(route(mime, None).unwrap(), Route::Editor, "{mime}");
        }
    }

    #[test]
    fn unknown_file_without_a_default_is_rejected() {
        assert!(matches!(
            route("application/octet-stream", None),
            Err(Error::NoAssociation(_))
        ));
        assert_eq!(
            Error::NoAssociation("application/octet-stream".into()).to_string(),
            "No application associated with application/octet-stream"
        );
    }

    #[test]
    fn editor_role_honors_environment_precedence_and_fallback() {
        let path = Path::new("note.txt");
        let visual = editor_plan_with(path, Some("nvim --nofork"), Some("nano")).unwrap();
        assert_eq!(visual.program, OsStr::new("nvim"));
        assert_eq!(
            visual.args,
            [OsStr::new("--nofork"), OsStr::new("note.txt")]
        );

        let editor = editor_plan_with(path, Some("  "), Some("code --wait")).unwrap();
        assert_eq!(editor.program, OsStr::new("code"));
        assert_eq!(editor.args, [OsStr::new("--wait"), OsStr::new("note.txt")]);

        let fallback = editor_plan_with(path, None, None).unwrap();
        assert_eq!(fallback.program, OsStr::new("vi"));
        assert_eq!(fallback.args, [OsStr::new("note.txt")]);
    }

    #[test]
    fn editor_command_keeps_arguments_and_path_separate() {
        let path = Path::new("a file.rs");
        let plan = editor_plan_from_parts("nvim", &["-f".into(), "+set number".into()], path);
        assert_eq!(plan.program, OsStr::new("nvim"));
        assert_eq!(
            plan.args,
            [
                OsStr::new("-f"),
                OsStr::new("+set number"),
                OsStr::new("a file.rs")
            ]
        );
        assert!(plan.needs_terminal());
    }

    #[cfg(unix)]
    #[test]
    fn command_plan_preserves_non_utf8_paths() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};

        let path = PathBuf::from(OsString::from_vec(vec![b'f', 0xff]));
        let plan = editor_plan_from_parts("vi", &[], &path);
        assert_eq!(plan.args[0].as_bytes(), &[b'f', 0xff]);
    }

    #[test]
    fn detached_launcher_reports_a_nonzero_exit() {
        let plan = Plan {
            program: "sh".into(),
            args: vec!["-c".into(), "printf broken >&2; exit 7".into()],
            terminal: false,
        };
        let result = plan
            .spawn_detached()
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(result.unwrap_err(), "sh: broken");
    }

    #[test]
    fn association_output_cannot_escape_the_applications_directory() {
        for id in [
            "../evil.desktop",
            "/tmp/evil.desktop",
            "not-a-desktop-id",
            "two\nlines.desktop",
        ] {
            assert!(validate_desktop_id(id).is_err(), "{id:?}");
        }
        assert!(validate_desktop_id("org.kde.kate.desktop").is_ok());
    }

    #[test]
    fn mime_output_must_be_one_well_formed_value() {
        assert!(validate_mime("text/plain").is_ok());
        for mime in [
            "text",
            "text/plain\nimage/png",
            "text/",
            "/plain",
            "text/plain extra",
        ] {
            assert!(validate_mime(mime).is_err(), "{mime:?}");
        }
    }

    #[test]
    fn terminal_flag_is_scoped_to_desktop_entry_group() {
        assert!(terminal_entry(
            "[Desktop Entry]\nName=Vim\n Terminal = TRUE\n[Desktop Action New]\nTerminal=false\n"
        ));
        assert!(!terminal_entry(
            "[Desktop Entry]\nTerminal=false\n[Desktop Action Edit]\nTerminal=true\n"
        ));
        assert!(!terminal_entry("[Other]\nTerminal=true\n"));
        assert!(!terminal_entry(
            "[Desktop Entry]\nName=First\n[Other]\n[Desktop Entry]\nTerminal=true\n"
        ));
    }
}
