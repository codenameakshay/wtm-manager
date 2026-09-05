//! Full-screen TUI: a thin presentation layer over the shared command cores.
//!
//! Design constraints:
//! - NO git or business logic here: create/remove/prune dispatch to the
//!   shared `pub(crate)` cores in `crate::commands`, and all reads go
//!   through `crate::worktree`. The TUI only renders state and routes input.
//! - Rendering happens on **stderr** (alternate screen) so stdout and the
//!   cd file are never polluted; the cd-on-exit path print stays clean.
//! - The first paint never blocks on status: the fast (no-status) listing
//!   renders immediately, while a background thread computes the full
//!   listing and delivers it over an mpsc channel drained each tick.
//! - The module is a pure state machine (`app::App::update`) plus this
//!   runtime, which executes `app::Effect`s and owns the terminal.

mod app;
mod view;

use std::collections::VecDeque;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use crossterm::event::{self, Event};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cdfile;
use crate::cli::GlobalArgs;
use crate::commands::{add, open, prune, remove};
use crate::config::{self, Config};
use crate::error::{Error, Result};
use crate::repo::{self, RepoContext};
use crate::worktree::{self, ListOptions};

use app::{App, Effect, Msg};

type Tui = Terminal<CrosstermBackend<io::Stderr>>;

/// Launch the interactive TUI. Returns after the terminal is fully restored;
/// when the user picked a worktree to switch to, the cd file is written (or
/// the path printed to stdout with a `wtm init` hint when no wrapper is
/// active).
pub fn run(global: &GlobalArgs) -> Result<()> {
    if !(io::stdin().is_terminal() && io::stderr().is_terminal()) {
        return Err(Error::Other(
            "wtm tui requires an interactive terminal".to_string(),
        ));
    }

    let ctx = repo::discover(global.repo.as_deref())?;
    let config = config::load(&ctx.main_root)?;

    // Fast first paint: list without status before the terminal is even
    // entered, then kick off the full-status load in the background.
    let rows = worktree::list(
        &ctx,
        &ListOptions {
            with_status: false,
            base: None,
        },
    )?;
    let mut app = App::new(
        config.default_base.clone(),
        config.prune.protected_branches.clone(),
    );
    let mut pending = app.update(Msg::RowsLoaded {
        generation: 0,
        rows,
        with_status: false,
    });
    pending.push(app.request_rows(true));

    install_panic_hook();
    let guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stderr()))?;
    let result = event_loop(&mut app, &mut terminal, &ctx, &config, pending);
    drop(terminal);
    drop(guard);

    // Terminal is restored; now the switch target (if any) can be handled.
    if let Some(path) = result? {
        if !cdfile::request(&path)? {
            println!("{}", path.display());
            eprintln!(
                "hint: install the shell wrapper with `eval \"$(command wtm init zsh)\"` \
                 (or bash) to cd automatically"
            );
        }
    }
    Ok(())
}

/// The runtime loop: draw, poll input with a 100ms timeout, drain background
/// messages, and execute effects. Returns the switch target, if any.
fn event_loop(
    app: &mut App,
    terminal: &mut Tui,
    ctx: &RepoContext,
    config: &Config,
    pending: Vec<Effect>,
) -> Result<Option<PathBuf>> {
    let (tx, rx) = mpsc::channel::<Msg>();
    let mut effects: VecDeque<Effect> = pending.into();

    loop {
        // Execute queued effects; immediate outcomes feed straight back into
        // the model and may enqueue more effects.
        while let Some(effect) = effects.pop_front() {
            match effect {
                Effect::Quit => return Ok(None),
                Effect::Switch { path } => return Ok(Some(path)),
                other => {
                    if let Some(msg) = run_effect(other, terminal, ctx, config, &tx)? {
                        effects.extend(app.update(msg));
                    }
                }
            }
        }

        // Drain background results (status listings, detail loads).
        while let Ok(msg) = rx.try_recv() {
            effects.extend(app.update(msg));
        }
        if !effects.is_empty() {
            continue;
        }

        terminal.draw(|f| view::draw(f, app))?;

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                effects.extend(app.update(Msg::Key(key)));
            }
        }
    }
}

/// Execute one non-terminal effect. Background loads return `None` (their
/// result arrives over the channel); synchronous actions return the outcome
/// message to feed back into the model.
fn run_effect(
    effect: Effect,
    terminal: &mut Tui,
    ctx: &RepoContext,
    config: &Config,
    tx: &mpsc::Sender<Msg>,
) -> Result<Option<Msg>> {
    match effect {
        Effect::Quit | Effect::Switch { .. } => unreachable!("handled by the loop"),
        Effect::LoadRows {
            generation,
            with_status,
        } => {
            let ctx = ctx.clone();
            let base = config.default_base.clone();
            let tx = tx.clone();
            std::thread::spawn(move || {
                let msg = match worktree::list(&ctx, &ListOptions { with_status, base }) {
                    Ok(rows) => Msg::RowsLoaded {
                        generation,
                        rows,
                        with_status,
                    },
                    Err(e) => Msg::RowsFailed {
                        generation,
                        with_status,
                        text: format!("list failed: {e}"),
                    },
                };
                let _ = tx.send(msg);
            });
            Ok(None)
        }
        Effect::LoadDetails { generation, path } => {
            let tx = tx.clone();
            std::thread::spawn(move || {
                let details = worktree::details(&path);
                let _ = tx.send(Msg::Details {
                    generation,
                    path,
                    details,
                });
            });
            Ok(None)
        }
        Effect::Create { branch, base } => {
            // git streams "Preparing worktree" and setup commands inherit
            // stdio, so the alternate screen must be left first.
            let outcome = suspended(terminal, || {
                let request = add::CreateRequest {
                    branch: &branch,
                    base_override: Some(&base),
                    path_override: None,
                    cd: false,
                    run_setup: true,
                    announce: false,
                    quiet: false,
                    verbose: false,
                };
                add::create(ctx, config, &request)
            })?;
            Ok(Some(match outcome {
                Ok(dest) => Msg::ActionOutcome {
                    text: format!("created worktree '{branch}' at {}", dest.display()),
                    error: false,
                    refresh: true,
                },
                // A setup failure still leaves the worktree in place.
                Err(e) => Msg::ActionOutcome {
                    text: format!("create failed: {e}"),
                    error: true,
                    refresh: true,
                },
            }))
        }
        Effect::Remove { info, force } => {
            let msg = match remove::remove_worktree(ctx, &info, force, true) {
                Ok(()) => Msg::ActionOutcome {
                    text: format!("removed worktree '{}'", info.display_name()),
                    error: false,
                    refresh: true,
                },
                Err(e) => Msg::ActionOutcome {
                    text: format!("remove failed: {e}"),
                    error: true,
                    refresh: false,
                },
            };
            Ok(Some(msg))
        }
        Effect::Prune { candidates, force } => {
            let report = prune::execute(ctx, &candidates, force, false);
            let mut text = format!("pruned {} worktree(s)", report.removed);
            if !report.skipped.is_empty() {
                text.push_str(&format!("; skipped (dirty): {}", report.skipped.join(", ")));
            }
            if !report.failures.is_empty() {
                text.push_str(&format!("; failed: {}", report.failures.join("; ")));
            }
            let msg = Msg::ActionOutcome {
                text,
                error: !report.failures.is_empty(),
                refresh: true,
            };
            Ok(Some(msg))
        }
        Effect::OpenEditor { path } => {
            let msg = match open::spawn_editor(config, &path) {
                Ok(()) => Msg::ActionOutcome {
                    text: format!("opened {} in your editor", path.display()),
                    error: false,
                    refresh: false,
                },
                Err(e) => Msg::ActionOutcome {
                    text: format!("open failed: {e}"),
                    error: true,
                    refresh: false,
                },
            };
            Ok(Some(msg))
        }
        Effect::RunCommand { path, command } => {
            let status = suspended_wait_key(terminal, || {
                Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .current_dir(&path)
                    .status()
            })?;
            let msg = match status {
                Ok(s) if s.success() => Msg::ActionOutcome {
                    text: format!("`{command}` finished"),
                    error: false,
                    refresh: true,
                },
                Ok(s) => Msg::ActionOutcome {
                    text: format!("`{command}` failed with {s}"),
                    error: true,
                    refresh: true,
                },
                Err(e) => Msg::ActionOutcome {
                    text: format!("`{command}` could not be started: {e}"),
                    error: true,
                    refresh: false,
                },
            };
            Ok(Some(msg))
        }
        Effect::CopyPath { path } => {
            let msg = match copy_to_clipboard(&path) {
                Ok(how) => Msg::ActionOutcome {
                    text: format!("copied {} ({how})", path.display()),
                    error: false,
                    refresh: false,
                },
                Err(e) => Msg::ActionOutcome {
                    text: format!("copy failed: {e}"),
                    error: true,
                    refresh: false,
                },
            };
            Ok(Some(msg))
        }
    }
}

/// Run `f` with the terminal fully released (cooked mode, main screen), then
/// re-enter the alternate screen and force a redraw.
fn suspended<T>(terminal: &mut Tui, f: impl FnOnce() -> T) -> Result<T> {
    disable_raw_mode()?;
    crossterm::execute!(io::stderr(), LeaveAlternateScreen)?;
    let out = f();
    enable_raw_mode()?;
    crossterm::execute!(io::stderr(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(out)
}

/// Like [`suspended`], but waits for a keypress before returning to the
/// alternate screen so the user can read the command's output.
fn suspended_wait_key<T>(terminal: &mut Tui, f: impl FnOnce() -> T) -> Result<T> {
    disable_raw_mode()?;
    crossterm::execute!(io::stderr(), LeaveAlternateScreen)?;
    let out = f();
    eprintln!();
    eprint!("[wtm] press any key to return");
    let _ = io::stderr().flush();
    enable_raw_mode()?;
    loop {
        if let Event::Key(key) = event::read()? {
            if key.kind == event::KeyEventKind::Press {
                break;
            }
        }
    }
    crossterm::execute!(io::stderr(), EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(out)
}

/// Copy `path` to the system clipboard via `crate::clipboard::copy`,
/// falling back to an OSC 52 escape sequence written to the terminal when no
/// clipboard tool is available.
fn copy_to_clipboard(path: &Path) -> Result<&'static str> {
    let text = path.display().to_string();
    if let Ok(tool) = crate::clipboard::copy(&text) {
        return Ok(tool);
    }

    // OSC 52: ask the terminal emulator itself to set the clipboard.
    let mut stderr = io::stderr();
    write!(stderr, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    stderr.flush()?;
    Ok("OSC 52")
}

/// Standard base64 with padding — enough for OSC 52 without a dependency.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// RAII ownership of raw mode + the alternate screen on stderr. `Drop`
/// restores the terminal on every exit path, including `?` early returns.
struct TerminalGuard;

impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        if let Err(e) = crossterm::execute!(io::stderr(), EnterAlternateScreen) {
            let _ = disable_raw_mode();
            return Err(e.into());
        }
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = crossterm::execute!(io::stderr(), LeaveAlternateScreen);
    }
}

/// Panic hook that restores the terminal before the default hook prints the
/// panic message — without it the message would be lost to the alternate
/// screen (and, with `panic = "abort"`, `Drop` never runs). Installed once.
fn install_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = crossterm::execute!(io::stderr(), LeaveAlternateScreen);
            previous(info);
        }));
    });
}
