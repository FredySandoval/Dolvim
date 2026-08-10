//! Dolvim — KDE Dolphin, recreated in the terminal.
//!
//! `main` owns the terminal and the event loop, and nothing else.

#![forbid(unsafe_code)]

mod app;
mod config;
mod drag;
mod fs;
mod mouse;
mod open;
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
        app.pump_external_launches();
        app.thumbs.pump_decoded_thumbs();
        finish_transfer(&mut app);

        if let Some(suspend_request) = app.suspend.take() {
            let open_result = hand_over(&mut terminal, || match suspend_request {
                Suspend::Shell(dir) => {
                    let shell = std::env::var_os("SHELL").unwrap_or_else(|| "/bin/sh".into());
                    println!("dolvim: shell in {} — exit to return", dir.display());
                    let _ = Command::new(shell).current_dir(&dir).status();
                    None
                }
                Suspend::Open(plan) => Some(plan.run_foreground()),
            })?;
            if let Some(Err(error)) = open_result {
                app.error(error);
            }
            app.refresh_in_place();
        }
    }

    restore_terminal();
    Ok(())
}

/// Collect a finished background transfer: journal it, report it, relist.
fn finish_transfer(app: &mut App) {
    let done = app.active_transfer.as_ref().is_some_and(|transfer| {
        transfer
            .progress
            .finished
            .load(std::sync::atomic::Ordering::Relaxed)
    });
    if !done {
        return;
    }
    let Some(active_transfer) = app.active_transfer.take() else {
        return;
    };
    let reveal = active_transfer.reveal;
    let selection_pane_id = active_transfer.selection_pane_id;
    let progress = active_transfer.progress;
    let outcome = progress
        .outcome
        .lock()
        .ok()
        .and_then(|mut outcome_guard| outcome_guard.take());
    let Some(outcome) = outcome else {
        app.error("Transfer finished without an outcome");
        app.refresh_in_place();
        return;
    };

    let committed_sources: std::collections::HashSet<_> = outcome
        .committed
        .iter()
        .map(|effect| effect.source.clone())
        .collect();
    match progress.kind {
        ops::TransferKind::Move if !outcome.committed.is_empty() => {
            app.undo.push(ops::UndoOp::Move {
                moved_pairs: outcome
                    .committed
                    .iter()
                    .map(|effect| (effect.source.clone(), effect.target.clone()))
                    .collect(),
            });
        }
        ops::TransferKind::Restore if !outcome.committed.is_empty() => {
            app.undo.push(ops::UndoOp::Restore {
                restored_paths: outcome
                    .committed
                    .iter()
                    .map(|effect| effect.target.clone())
                    .collect(),
                previous_items: outcome
                    .committed
                    .iter()
                    .filter_map(|effect| effect.trash_ref.clone())
                    .collect(),
            });
        }
        _ => {}
    }

    if progress.expected_register.as_ref() == Some(&app.register) {
        match (&progress.kind, &progress.expected_register) {
            (ops::TransferKind::Move, Some(ops::UnnamedRegister::Live { paths, cut: true })) => {
                let remaining: Vec<_> = paths
                    .iter()
                    .filter(|path| !committed_sources.contains(*path))
                    .cloned()
                    .collect();
                app.register = if remaining.is_empty() {
                    ops::UnnamedRegister::Empty
                } else {
                    ops::UnnamedRegister::Live {
                        paths: remaining,
                        cut: true,
                    }
                };
            }
            (ops::TransferKind::Restore, Some(ops::UnnamedRegister::Deleted { items })) => {
                let committed_ids: std::collections::HashSet<_> = outcome
                    .committed
                    .iter()
                    .filter_map(|effect| effect.trash_ref.as_ref().map(|item| item.id.clone()))
                    .collect();
                let remaining: Vec<_> = items
                    .iter()
                    .filter(|item| !committed_ids.contains(&item.id))
                    .cloned()
                    .collect();
                app.register = if remaining.is_empty() {
                    ops::UnnamedRegister::Live {
                        paths: outcome
                            .committed
                            .iter()
                            .map(|effect| effect.target.clone())
                            .collect(),
                        cut: false,
                    }
                } else {
                    ops::UnnamedRegister::Deleted { items: remaining }
                };
            }
            _ => {}
        }
    }

    let committed_selection_keys: std::collections::HashSet<_> = outcome
        .committed
        .iter()
        .map(|effect| {
            effect
                .trash_ref
                .as_ref()
                .map_or_else(|| effect.source.clone(), ops::TrashRef::selection_key)
        })
        .collect();
    app.remove_selection_keys(selection_pane_id, &committed_selection_keys);

    let committed = outcome.committed.len();
    let failed = outcome.failed.len();
    if failed > 0 || outcome.cancelled {
        app.error(format!(
            "{} — {committed} committed, {failed} failed{}",
            progress.label,
            if outcome.cancelled { ", cancelled" } else { "" }
        ));
    } else {
        app.info(format!("{} — {committed} done", progress.label));
    }
    let reveal_pane_id = reveal.as_ref().map(|intent| intent.pane_id);
    if reveal_pane_id != Some(selection_pane_id) {
        app.refresh_pane_in_place(selection_pane_id);
    }
    if let (Some(intent), Some(target)) = (
        reveal,
        outcome
            .committed
            .first()
            .map(|effect| effect.target.clone()),
    ) {
        app.reveal_completed(intent, target);
    } else {
        app.refresh_in_place();
    }
}

/// Give the terminal to a child that needs all of it — a shell, an editor —
/// and take it back when the child exits. Running `f` to completion between a
/// `restore` and a re-entry is the whole mechanism, and the only correct one:
/// two programs in raw mode on one tty fight over the cursor and the screen.
/// See docs/DECISIONS.md for why there is no PTY here.
fn hand_over<T>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    run_child: impl FnOnce() -> T,
) -> io::Result<T> {
    restore_terminal();
    let result = run_child();
    // Back through the same door we came in by: the child ran with the terminal
    // as it found it, and the enhancement flags have to be pushed again for us.
    enter_raw_screen()?;
    terminal.clear()?;
    Ok(result)
}
