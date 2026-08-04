//! Dolvim — KDE Dolphin, recreated in the terminal.
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
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::{App, Suspend};

fn main() {
    let start_dir = match std::env::args().nth(1) {
        Some(first_arg) if first_arg == "-h" || first_arg == "--help" => {
            println!(
                "usage: dolvim [DIR]\n\nKDE Dolphin, in the terminal. Press F1 inside for keys."
            );
            return;
        }
        Some(first_arg) => PathBuf::from(first_arg),
        None => std::env::current_dir().unwrap_or_else(|_| places::home()),
    };
    if !start_dir.is_dir() {
        eprintln!("dolvim: {}: not a directory", start_dir.display());
        std::process::exit(1);
    }
    let start_dir = start_dir.canonicalize().unwrap_or(start_dir);

    if let Err(run_error) = run(start_dir) {
        restore_terminal();
        eprintln!("dolvim: {run_error}");
        std::process::exit(1);
    }
}

/// Whether the keyboard enhancement flags are currently pushed, so the pop on
/// the way out matches the push and nothing else. A terminal that never got the
/// push must not be sent the pop.
static ENHANCED_KEYS: AtomicBool = AtomicBool::new(false);

/// Raw mode, alternate screen, mouse — and the kitty keyboard protocol where
/// the terminal speaks it.
///
/// The legacy encoding transmits Ctrl+letter as a bare control byte: 0x01 for
/// Ctrl+A, with no room in it for case or shift. Ctrl+Shift+A is therefore the
/// same byte as Ctrl+A, and Ctrl+I the same as Tab. `DISAMBIGUATE_ESCAPE_CODES`
/// asks for the real event instead, which is what makes `Ctrl+Shift+A` reach
/// `InvertSelect` rather than `SelectAll`. Terminals that do not speak it are
/// left exactly as they were — the keymap works there, minus those keys.
///
/// Pushed without asking first. `supports_keyboard_enhancement` asks by writing
/// a query and waiting for an answer, and a terminal that does not implement the
/// protocol never sends one — the wait is crossterm's full two-second timeout,
/// paid at startup and again after every shell hand-over. The request itself is
/// a private CSI sequence, which a terminal that does not know it discards; an
/// ignored push costs nothing and leaves us with exactly the legacy encoding we
/// would have had anyway. If some terminal ever prints it instead of dropping
/// it, this is the line to guard.
fn enter_raw_screen() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let pushed = execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    );
    ENHANCED_KEYS.store(pushed.is_ok(), Ordering::Relaxed);
    Ok(())
}

fn setup_terminal() -> io::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enter_raw_screen()?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

/// Idempotent: safe from the panic hook and again on the way out. The keyboard
/// flags are popped first, while the screen we pushed them on is still up.
fn restore_terminal() {
    if ENHANCED_KEYS.swap(false, Ordering::Relaxed) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = io::stdout().flush();
}

fn run(start: PathBuf) -> io::Result<()> {
    // A panic must not leave the user in a raw-mode alternate screen with no
    // echo. Restore first, then let the default hook print its report.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let mut terminal = setup_terminal()?;
    let mut app = App::new(start);
    let poll_timeout = Duration::from_millis(config::TICK_MS);

    while !app.quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        if event::poll(poll_timeout)? {
            match event::read()? {
                Event::Key(key_event) if key_event.kind != KeyEventKind::Release => {
                    app.status.clear();
                    app.status_is_error = false;
                    vim::handle_key_event(&mut app, key_event);
                }
                Event::Mouse(mouse_event) => mouse::handle_mouse_event(&mut app, mouse_event),
                _ => {}
            }
        }

        app.pump_fs_events();
        app.thumbs.pump_decoded_thumbs();
        finish_transfer(&mut app);

        if let Some(suspend_request) = app.suspend.take() {
            hand_over(&mut terminal, || match suspend_request {
                Suspend::Shell(dir) => {
                    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
                    println!("dolvim: shell in {} — exit to return", dir.display());
                    let _ = Command::new(shell).current_dir(&dir).status();
                }
                Suspend::Open(path) => {
                    let _ = Command::new("xdg-open").arg(&path).status();
                }
            })?;
            app.refresh_in_place();
        }
    }

    restore_terminal();
    Ok(())
}

/// Collect a finished background transfer: journal it, report it, relist.
fn finish_transfer(app: &mut App) {
    let done = app
        .transfer_progress
        .as_ref()
        .is_some_and(|transfer| transfer.finished.load(std::sync::atomic::Ordering::Relaxed));
    if !done {
        return;
    }
    let Some(active_transfer) = app.transfer_progress.take() else {
        return;
    };
    let outcome = active_transfer
        .outcome
        .lock()
        .ok()
        .and_then(|mut outcome_guard| outcome_guard.take());
    match outcome {
        Some(Ok(undo_op)) => {
            // A pure copy journals nothing — there is nothing to put back.
            if let ops::UndoOp::Move { moved_pairs } = &undo_op {
                if !moved_pairs.is_empty() {
                    app.undo.push(undo_op.clone());
                }
            }
            app.info(format!("{} — done", active_transfer.label));
        }
        Some(Err(transfer_error)) => app.error(transfer_error),
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
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    run_child: impl FnOnce(),
) -> io::Result<()> {
    restore_terminal();
    run_child();
    // Back through the same door we came in by: the child ran with the terminal
    // as it found it, and the enhancement flags have to be pushed again for us.
    enter_raw_screen()?;
    terminal.clear()?;
    Ok(())
}
