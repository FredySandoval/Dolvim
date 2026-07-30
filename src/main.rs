//! Dolvin — KDE Dolphin, recreated in the terminal.
//!
//! `main` owns the terminal and the event loop, and nothing else.

#![forbid(unsafe_code)]

mod app;
mod config;
mod drag;
mod fs;
mod mouse;
mod ops;
mod places;
mod thumbs;
mod ui;
mod vim;
mod watch;

use std::io::{self, Write};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use crossterm::event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, Suspend};

fn main() {
    let start = match std::env::args().nth(1) {
        Some(a) if a == "-h" || a == "--help" => {
            println!(
                "usage: dolvin [DIR]\n\nKDE Dolphin, in the terminal. Press F1 inside for keys."
            );
            return;
        }
        Some(a) => PathBuf::from(a),
        None => std::env::current_dir().unwrap_or_else(|_| places::home()),
    };
    if !start.is_dir() {
        eprintln!("dolvin: {}: not a directory", start.display());
        std::process::exit(1);
    }
    let start = start.canonicalize().unwrap_or(start);

    if let Err(e) = run(start) {
        restore();
        eprintln!("dolvin: {e}");
        std::process::exit(1);
    }
}

fn setup() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(out))
}

/// Idempotent: safe from the panic hook and again on the way out.
fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = io::stdout().flush();
}

fn run(start: PathBuf) -> io::Result<()> {
    // A panic must not leave the user in a raw-mode alternate screen with no
    // echo. Restore first, then let the default hook print its report.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        default_hook(info);
    }));

    let mut term = setup()?;
    let mut app = App::new(start);
    let tick = Duration::from_millis(config::TICK_MS);

    while !app.quit {
        term.draw(|f| ui::draw(f, &mut app))?;

        if event::poll(tick)? {
            match event::read()? {
                Event::Key(k) if k.kind != KeyEventKind::Release => {
                    app.status.clear();
                    app.status_is_error = false;
                    vim::key(&mut app, k);
                }
                Event::Mouse(m) => mouse::handle(&mut app, m),
                _ => {}
            }
        }

        app.settle_zoom();
        app.pump();
        app.thumbs.pump();
        finish_progress(&mut app);

        if let Some(what) = app.suspend.take() {
            hand_over(&mut term, || match what {
                Suspend::Shell(dir) => {
                    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
                    println!("dolvin: shell in {} — exit to return", dir.display());
                    let _ = Command::new(shell).current_dir(&dir).status();
                }
                Suspend::Open(path) => {
                    let _ = Command::new("xdg-open").arg(&path).status();
                }
            })?;
            app.refresh_in_place();
        }
    }

    restore();
    Ok(())
}

/// Collect a finished background transfer: journal it, report it, relist.
fn finish_progress(app: &mut App) {
    let done = app
        .progress
        .as_ref()
        .is_some_and(|p| p.finished.load(std::sync::atomic::Ordering::Relaxed));
    if !done {
        return;
    }
    let Some(p) = app.progress.take() else { return };
    let outcome = p.outcome.lock().ok().and_then(|mut g| g.take());
    match outcome {
        Some(Ok(op)) => {
            // A pure copy journals nothing — there is nothing to put back.
            if let ops::UndoOp::Move { pairs } = &op {
                if !pairs.is_empty() {
                    app.undo.push(op.clone());
                }
            }
            app.info(format!("{} — done", p.label));
        }
        Some(Err(e)) => app.error(e),
        None => {}
    }
    app.pane_mut().selected.clear();
    app.refresh_in_place();
}

/// Give the terminal to a child that needs all of it — a shell, an editor —
/// and take it back when the child exits. Running `f` to completion between a
/// `restore` and a re-entry is the whole mechanism, and the only correct one:
/// two programs in raw mode on one tty fight over the cursor and the screen.
/// See docs/DECISIONS.md for why there is no PTY here.
fn hand_over(
    term: &mut Terminal<CrosstermBackend<io::Stdout>>,
    f: impl FnOnce(),
) -> io::Result<()> {
    restore();
    f();
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    term.clear()?;
    Ok(())
}
