//! Persistent editor integration over a small newline-delimited JSON protocol.

use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const VERSION: u8 = 1;
const MAX_LINE: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Full,
    Sidebar,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum Incoming {
    #[serde(rename = "set_layout")]
    SetLayout { version: u8, layout: String },
    #[serde(rename = "set_focus")]
    SetFocus { version: u8, focused: bool },
    #[serde(rename = "opened")]
    Opened { version: u8, id: u64, path: String },
    #[serde(rename = "reveal")]
    Reveal { version: u8, path: String },
    #[serde(rename = "shutdown")]
    Shutdown { version: u8 },
}

#[derive(Debug)]
pub enum Event {
    Message(Incoming),
    Disconnected,
    Error(String),
}

#[derive(Serialize)]
struct Outgoing<'a> {
    version: u8,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    root: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'a str>,
}

enum Command {
    Line(String),
    Close,
}

#[derive(Clone)]
pub struct Handle {
    tx: Sender<Command>,
}

impl Handle {
    pub fn open(&self, id: u64, path: &Path) -> Result<(), String> {
        let path = path
            .to_str()
            .ok_or_else(|| "Editor integration cannot open a non-UTF-8 path".to_string())?;
        self.send(Outgoing {
            version: VERSION,
            kind: "open",
            token: None,
            root: None,
            id: Some(id),
            path: Some(path),
            reason: None,
        })
    }

    fn send(&self, message: Outgoing<'_>) -> Result<(), String> {
        let mut line = serde_json::to_string(&message).map_err(|error| error.to_string())?;
        if line.len() > MAX_LINE {
            return Err("Editor protocol line is too large".into());
        }
        line.push('\n');
        self.tx
            .send(Command::Line(line))
            .map_err(|_| "Editor connection is closed".into())
    }
}

pub struct Connection {
    handle: Handle,
    rx: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
}

impl Connection {
    pub fn connect(address: SocketAddr, token: &str, root: &Path) -> Result<Self, String> {
        if !address.ip().is_loopback() {
            return Err("--editor-connect must name a loopback address".into());
        }
        let root = root
            .to_str()
            .ok_or_else(|| "Editor integration root is not valid UTF-8".to_string())?;
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
            .map_err(|error| format!("cannot connect to editor at {address}: {error}"))?;
        stream
            .set_read_timeout(Some(Duration::from_millis(40)))
            .map_err(|error| format!("cannot configure editor connection: {error}"))?;
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || socket_worker(stream, command_rx, event_tx));
        let connection = Self {
            handle: Handle { tx: command_tx },
            rx: event_rx,
            worker: Some(worker),
        };
        connection.send_startup(token, root)?;
        Ok(connection)
    }

    fn send_startup(&self, token: &str, root: &str) -> Result<(), String> {
        self.handle.send(Outgoing {
            version: VERSION,
            kind: "hello",
            token: Some(token),
            root: None,
            id: None,
            path: None,
            reason: None,
        })?;
        self.handle.send(Outgoing {
            version: VERSION,
            kind: "ready",
            token: None,
            root: Some(root),
            id: None,
            path: None,
            reason: None,
        })
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }

    pub fn events(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }

    pub fn close(mut self, reason: &str) {
        let _ = self.handle.send(Outgoing {
            version: VERSION,
            kind: "exiting",
            token: None,
            root: None,
            id: None,
            path: None,
            reason: Some(reason),
        });
        let _ = self.handle.tx.send(Command::Close);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn socket_worker(mut stream: TcpStream, commands: Receiver<Command>, events: Sender<Event>) {
    let mut pending = Vec::new();
    let mut input = [0; 4096];
    loop {
        loop {
            match commands.try_recv() {
                Ok(Command::Line(line)) => {
                    if let Err(error) = stream.write_all(line.as_bytes()) {
                        let _ = events.send(Event::Error(format!(
                            "Editor connection write failed: {error}"
                        )));
                        return;
                    }
                }
                Ok(Command::Close) => {
                    let _ = stream.flush();
                    return;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        match stream.read(&mut input) {
            Ok(0) => {
                let _ = events.send(Event::Disconnected);
                return;
            }
            Ok(count) => {
                pending.extend_from_slice(&input[..count]);
                match take_lines(&mut pending) {
                    Ok(lines) => {
                        for line in lines {
                            match parse_message(&line) {
                                Ok(message) => {
                                    if events.send(Event::Message(message)).is_err() {
                                        return;
                                    }
                                }
                                Err(error) => {
                                    let _ = events.send(Event::Error(error));
                                    return;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        let _ = events.send(Event::Error(error));
                        return;
                    }
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) => {
                let _ = events.send(Event::Error(format!(
                    "Editor connection read failed: {error}"
                )));
                return;
            }
        }
    }
}

fn take_lines(buffer: &mut Vec<u8>) -> Result<Vec<Vec<u8>>, String> {
    if buffer.len() > MAX_LINE && !buffer.contains(&b'\n') {
        return Err("Editor protocol line is too large".into());
    }
    let mut lines = Vec::new();
    while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
        if end > MAX_LINE {
            return Err("Editor protocol line is too large".into());
        }
        let mut line: Vec<u8> = buffer.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        lines.push(line);
    }
    Ok(lines)
}

fn parse_message(line: &[u8]) -> Result<Incoming, String> {
    let message: Incoming = serde_json::from_slice(line)
        .map_err(|error| format!("Invalid editor protocol message: {error}"))?;
    let version = match &message {
        Incoming::SetLayout { version, .. }
        | Incoming::SetFocus { version, .. }
        | Incoming::Opened { version, .. }
        | Incoming::Reveal { version, .. }
        | Incoming::Shutdown { version } => *version,
    };
    if version != VERSION {
        return Err(format!("Unsupported editor protocol version: {version}"));
    }
    if let Incoming::SetLayout { layout, .. } = &message {
        if layout != "full" && layout != "sidebar" {
            return Err(format!("Unsupported editor layout: {layout}"));
        }
    }
    Ok(message)
}

#[cfg(test)]
pub fn test_handle() -> Handle {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || while rx.recv().is_ok() {});
    Handle { tx }
}

pub fn protocol_path(path: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        Err("Editor protocol paths must be absolute".into())
    } else {
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_handles_partial_and_multiple_lines() {
        let mut buffer = br#"{"version":1,"type":"shutdown"}
{"version":1"#
            .to_vec();
        let lines = take_lines(&mut buffer).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(matches!(
            parse_message(&lines[0]),
            Ok(Incoming::Shutdown { .. })
        ));
        assert!(!buffer.is_empty());
    }

    #[test]
    fn malformed_oversized_and_wrong_version_are_rejected() {
        assert!(parse_message(b"not json").is_err());
        assert!(parse_message(br#"{"version":2,"type":"shutdown"}"#).is_err());
        let mut oversized = vec![b'x'; MAX_LINE + 1];
        assert!(take_lines(&mut oversized).is_err());
    }

    #[test]
    fn connection_eof_is_reported() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut input = [0; 1024];
            let mut newlines = 0;
            while newlines < 2 {
                let count = stream.read(&mut input).unwrap();
                newlines += input[..count].iter().filter(|byte| **byte == b'\n').count();
            }
        });
        let root = std::env::temp_dir();
        let connection = Connection::connect(address, "token", &root).unwrap();
        server.join().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if connection
                .events()
                .iter()
                .any(|event| matches!(event, Event::Disconnected))
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "EOF was not reported");
            std::thread::sleep(Duration::from_millis(10));
        }
        connection.close("test");
    }

    #[test]
    fn authentication_message_contains_exact_token_and_version() {
        let message = serde_json::to_value(Outgoing {
            version: VERSION,
            kind: "hello",
            token: Some("secret"),
            root: None,
            id: None,
            path: None,
            reason: None,
        })
        .unwrap();
        assert_eq!(message["version"], VERSION);
        assert_eq!(message["type"], "hello");
        assert_eq!(message["token"], "secret");
    }
}
