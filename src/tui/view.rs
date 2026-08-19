//! Pure view functions: render the [`App`] model into a ratatui frame.
//!
//! Nothing here mutates the model or performs I/O beyond drawing; every
//! function is a straight projection of state, which keeps rendering
//! testable with `ratatui::backend::TestBackend`.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::Frame;

use crate::model::WorktreeInfo;
use crate::tui::app::{App, CreateField, Overlay};

/// Persistent footer hints; `?` shows the full legend.
const FOOTER_HINTS: &str =
    "enter switch  n new  d remove  space mark  p prune  o open  x cmd  y yank  / filter  r refresh  ? help  q quit";

/// Render the whole screen: panes, footer, and any open overlay.
pub(crate) fn draw(f: &mut Frame, app: &App) {
    let outer = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(f.area());
    let panes = Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(outer[0]);

    draw_list(f, app, panes[0]);
    draw_details(f, app, panes[1]);
    draw_message(f, app, outer[1]);
    f.render_widget(
        Paragraph::new(FOOTER_HINTS).style(Style::new().fg(Color::DarkGray)),
        outer[2],
    );

    match &app.overlay {
        Overlay::None => {}
        Overlay::Help => draw_help(f),
        Overlay::Notice { text } => draw_notice(f, text),
        Overlay::ConfirmRemove { info, force, dirty } => {
            draw_confirm_remove(f, info, *force, *dirty)
        }
        Overlay::ConfirmPrune {
            candidates,
            force,
            unsafe_count,
        } => draw_confirm_prune(f, candidates, *force, *unsafe_count),
        Overlay::Create {
            branch,
            base,
            field,
        } => draw_create(f, branch, base, *field),
        Overlay::Command { input } => draw_command(f, input),
    }
}

/// Left pane: every worktree with its status badges.
fn draw_list(f: &mut Frame, app: &App, area: Rect) {
    let mut title = String::from(" Worktrees ");
    if !app.filter.is_empty() || app.filter_editing {
        title = format!(
            " Worktrees — /{}{} ",
            app.filter,
            if app.filter_editing { "▏" } else { "" }
        );
    }
    if app.status_loading {
        title.push_str("(loading status…) ");
    }

    let items: Vec<ListItem> = app
        .filtered
        .iter()
        .map(|&i| {
            ListItem::new(row_line(
                &app.rows[i],
                app.marked.contains(&app.rows[i].path),
            ))
        })
        .collect();

    let mut state = ListState::default();
    if !app.filtered.is_empty() {
        state.select(Some(app.cursor));
    }

    let list = List::new(items)
        .block(Block::bordered().title(title))
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, &mut state);
}

/// One list row: mark, name, branch/HEAD, and status badges.
fn row_line(info: &WorktreeInfo, marked: bool) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::raw(if marked { "✓ " } else { "  " }));
    spans.push(Span::styled(
        format!("{:<20} ", info.display_name()),
        Style::new().add_modifier(Modifier::BOLD),
    ));

    let refname = match (&info.branch, &info.head) {
        (Some(b), _) => b.clone(),
        (None, Some(h)) => format!("({h})"),
        (None, None) => "(unknown)".to_string(),
    };
    spans.push(Span::styled(
        format!("{refname} "),
        Style::new().fg(Color::Cyan),
    ));

    if let Some(status) = &info.status {
        if let (Some(a), Some(b)) = (status.ahead, status.behind) {
            if a > 0 || b > 0 {
                spans.push(Span::styled(
                    format!("↑{a} ↓{b} "),
                    Style::new().fg(Color::Yellow),
                ));
            }
        }
        if status.dirty {
            spans.push(Span::styled("* ", Style::new().fg(Color::Red)));
        }
        if status.merged {
            spans.push(Span::styled("[merged] ", Style::new().fg(Color::Green)));
        }
        if status.upstream_gone {
            spans.push(Span::styled("[gone] ", Style::new().fg(Color::Magenta)));
        }
    }
    if info.is_main {
        spans.push(Span::styled("[main] ", Style::new().fg(Color::Blue)));
    }
    if info.is_missing {
        spans.push(Span::styled("[missing] ", Style::new().fg(Color::Red)));
    }
    if info.is_locked {
        spans.push(Span::raw("[locked] "));
    }

    Line::from(spans)
}

/// Right pane: everything known about the selected worktree.
fn draw_details(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(info) = app.selected() {
        let label = |s: &str| Span::styled(format!("{s:<10}"), Style::new().fg(Color::DarkGray));
        lines.push(Line::from(vec![
            label("branch"),
            Span::raw(info.branch.clone().unwrap_or_else(|| "(detached)".into())),
        ]));
        lines.push(Line::from(vec![
            label("path"),
            Span::raw(info.path.display().to_string()),
        ]));
        lines.push(Line::from(vec![
            label("head"),
            Span::raw(info.head.clone().unwrap_or_else(|| "?".into())),
        ]));
        if let Some(status) = &info.status {
            let mut s = String::new();
            if let (Some(a), Some(b)) = (status.ahead, status.behind) {
                s.push_str(&format!("↑{a} ↓{b} "));
            }
            if status.dirty {
                s.push_str("dirty ");
            }
            if status.merged {
                s.push_str("merged ");
            }
            if status.upstream_gone {
                s.push_str("upstream gone ");
            }
            if s.is_empty() {
                s.push_str("clean");
            }
            lines.push(Line::from(vec![label("status"), Span::raw(s)]));
        }

        if info.is_missing {
            lines.push(Line::raw(""));
            lines.push(Line::styled(
                "directory is missing from disk",
                Style::new().fg(Color::Red),
            ));
        } else {
            match app.details.get(&info.path) {
                Some(Some(details)) => {
                    lines.push(Line::from(vec![
                        label("upstream"),
                        Span::raw(details.upstream.clone().unwrap_or_else(|| "(none)".into())),
                    ]));

                    lines.push(Line::raw(""));
                    lines.push(Line::styled(
                        format!("changes ({})", details.dirty_total),
                        Style::new().add_modifier(Modifier::BOLD),
                    ));
                    for file in &details.dirty_files {
                        lines.push(Line::from(format!("  {file}")));
                    }
                    let more = details
                        .dirty_total
                        .saturating_sub(details.dirty_files.len());
                    if more > 0 {
                        lines.push(Line::styled(
                            format!("  … and {more} more"),
                            Style::new().fg(Color::DarkGray),
                        ));
                    }

                    lines.push(Line::raw(""));
                    lines.push(Line::styled(
                        "recent commits",
                        Style::new().add_modifier(Modifier::BOLD),
                    ));
                    for commit in &details.commits {
                        lines.push(Line::from(vec![
                            Span::styled(
                                format!("  {} ", commit.id),
                                Style::new().fg(Color::Yellow),
                            ),
                            Span::raw(commit.summary.clone()),
                        ]));
                    }
                }
                Some(None) => {
                    lines.push(Line::raw(""));
                    lines.push(Line::raw("details unavailable"));
                }
                None => {
                    lines.push(Line::raw(""));
                    lines.push(Line::styled(
                        "loading details…",
                        Style::new().fg(Color::DarkGray),
                    ));
                }
            }
        }
    } else {
        lines.push(Line::raw("no worktree selected"));
    }

    f.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(" Details ")),
        area,
    );
}

/// Footer message bar (errors in red, info in green).
fn draw_message(f: &mut Frame, app: &App, area: Rect) {
    let Some(message) = &app.message else {
        return;
    };
    let style = if message.error {
        Style::new().fg(Color::Red)
    } else {
        Style::new().fg(Color::Green)
    };
    f.render_widget(Paragraph::new(message.text.clone()).style(style), area);
}

/// Full keybinding legend.
fn draw_help(f: &mut Frame) {
    let bindings = [
        ("j/k, ↓/↑", "move selection"),
        ("g / G", "first / last"),
        ("enter", "switch to worktree (cd on exit)"),
        ("n", "create a new worktree"),
        ("d", "remove selected worktree"),
        ("space", "toggle multi-select"),
        ("p", "prune (selected rows, or stale worktrees)"),
        ("o", "open in editor"),
        ("x", "run a shell command in the worktree"),
        ("y", "copy path to clipboard"),
        ("/", "fuzzy filter (esc clears)"),
        ("r", "refresh status"),
        ("?", "this help"),
        ("q / esc", "quit"),
    ];
    let mut lines: Vec<Line> = Vec::new();
    for (keys, what) in bindings {
        lines.push(Line::from(vec![
            Span::styled(format!("  {keys:<12}"), Style::new().fg(Color::Yellow)),
            Span::raw(what),
        ]));
    }
    let area = centered(f.area(), 52, (bindings.len() + 2) as u16);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(" Help ")),
        area,
    );
}

/// A small, dismissible notice modal (rejected actions, etc.).
fn draw_notice(f: &mut Frame, text: &str) {
    let lines = vec![
        Line::from(text.to_string()),
        Line::raw(""),
        Line::styled(
            "press enter or esc to dismiss",
            Style::new().fg(Color::DarkGray),
        ),
    ];
    let width = (text.len() as u16 + 4).max(28).min(f.area().width);
    let area = centered(f.area(), width, (lines.len() + 2) as u16);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Notice ")),
        area,
    );
}

fn draw_confirm_remove(f: &mut Frame, info: &WorktreeInfo, force: bool, dirty: bool) {
    let mut lines = vec![
        Line::from(format!("Remove worktree '{}'?", info.display_name())),
        Line::styled(
            info.path.display().to_string(),
            Style::new().fg(Color::DarkGray),
        ),
        Line::raw(""),
    ];
    if dirty {
        lines.push(Line::styled(
            "this worktree has uncommitted changes",
            Style::new().fg(Color::Red),
        ));
    }
    lines.push(Line::from(format!(
        "force: [{}]",
        if force { "x" } else { " " }
    )));
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "enter confirm   f toggle force   esc cancel",
        Style::new().fg(Color::DarkGray),
    ));

    let area = centered(f.area(), 56, (lines.len() + 2) as u16);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Remove ")),
        area,
    );
}

fn draw_confirm_prune(
    f: &mut Frame,
    candidates: &[crate::commands::prune::PruneCandidate],
    force: bool,
    unsafe_count: usize,
) {
    let mut lines = vec![Line::from(format!(
        "Prune {} worktree(s)?",
        candidates.len()
    ))];
    lines.push(Line::raw(""));
    for c in candidates {
        lines.push(Line::from(vec![
            Span::raw(format!("  {} ", c.info.display_name())),
            Span::styled(
                format!("[{}]", c.reasons.join(", ")),
                Style::new().fg(Color::Yellow),
            ),
            Span::styled(
                if c.delete_branch {
                    " + delete branch"
                } else {
                    ""
                },
                Style::new().fg(Color::Red),
            ),
        ]));
    }
    if unsafe_count > 0 {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("{unsafe_count} worktree(s) have changes or unavailable status"),
            Style::new().fg(Color::Red),
        ));
        lines.push(Line::from(format!(
            "force: [{}]",
            if force { "x" } else { " " }
        )));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "enter confirm   f toggle force   esc cancel",
        Style::new().fg(Color::DarkGray),
    ));

    let area = centered(f.area(), 60, (lines.len() + 2) as u16);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(" Prune ")),
        area,
    );
}

fn draw_create(f: &mut Frame, branch: &str, base: &str, field: CreateField) {
    let input_line = |label: &str, value: &str, active: bool| {
        let marker = if active { "> " } else { "  " };
        let cursor = if active { "▏" } else { "" };
        Line::from(vec![
            Span::styled(
                format!("{marker}{label:<8}"),
                if active {
                    Style::new().fg(Color::Yellow)
                } else {
                    Style::new().fg(Color::DarkGray)
                },
            ),
            Span::raw(format!("{value}{cursor}")),
        ])
    };
    let lines = vec![
        input_line("branch", branch, field == CreateField::Branch),
        input_line("base", base, field == CreateField::Base),
        Line::raw(""),
        Line::styled("empty base = HEAD", Style::new().fg(Color::DarkGray)),
        Line::styled(
            "enter create   tab switch field   esc cancel",
            Style::new().fg(Color::DarkGray),
        ),
    ];
    let area = centered(f.area(), 56, (lines.len() + 2) as u16);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(" New worktree ")),
        area,
    );
}

fn draw_command(f: &mut Frame, input: &str) {
    let lines = vec![
        Line::from(vec![
            Span::styled("$ ", Style::new().fg(Color::Yellow)),
            Span::raw(format!("{input}▏")),
        ]),
        Line::raw(""),
        Line::styled(
            "enter run in worktree   esc cancel",
            Style::new().fg(Color::DarkGray),
        ),
    ];
    let area = centered(f.area(), 60, (lines.len() + 2) as u16);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(Text::from(lines)).block(Block::bordered().title(" Run command ")),
        area,
    );
}

/// A centered rect of at most `width` x `height`, clamped to `area`.
fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorktreeStatus;
    use crate::tui::app::Msg;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;

    fn info(name: &str, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/wt/{name}")),
            branch: Some(name.to_string()),
            head: Some("abc1234".to_string()),
            is_main,
            is_missing: false,
            is_locked: false,
            is_prunable: false,
            status: Some(WorktreeStatus {
                dirty: name.contains("dirty"),
                dirty_count: usize::from(name.contains("dirty")),
                ahead: Some(2),
                behind: Some(1),
                upstream_gone: false,
                merged: name.contains("done"),
            }),
        }
    }

    fn app() -> App {
        let mut app = App::new(Some("origin/main".to_string()), vec!["main".to_string()]);
        app.update(Msg::RowsLoaded {
            generation: 0,
            rows: vec![
                info("main", true),
                info("dirty-feat", false),
                info("done", false),
            ],
            with_status: true,
        });
        app
    }

    fn render(app: &App) -> String {
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn press(app: &mut App, code: KeyCode) {
        app.update(Msg::Key(KeyEvent::new(code, KeyModifiers::NONE)));
    }

    #[test]
    fn list_screen_shows_rows_badges_and_footer() {
        let app = app();
        let out = render(&app);
        assert!(out.contains("Worktrees"));
        assert!(out.contains("main"));
        assert!(out.contains("dirty-feat"));
        assert!(out.contains("[main]"));
        assert!(out.contains("[merged]"));
        assert!(out.contains("↑2 ↓1"));
        assert!(out.contains("* "), "dirty badge");
        assert!(out.contains("? help"));
        assert!(out.contains("q quit"));
    }

    #[test]
    fn detail_pane_shows_loaded_details() {
        let mut app = app();
        app.details.insert(
            PathBuf::from("/wt/main"),
            Some(crate::worktree::WorktreeDetails {
                upstream: Some("origin/main".to_string()),
                dirty_files: vec!["src/lib.rs".to_string()],
                dirty_total: 18,
                commits: vec![crate::worktree::CommitLine {
                    id: "abc1234".to_string(),
                    summary: "initial commit".to_string(),
                }],
            }),
        );
        let out = render(&app);
        assert!(out.contains("Details"));
        assert!(out.contains("origin/main"));
        assert!(out.contains("/wt/main"));
        assert!(out.contains("src/lib.rs"));
        assert!(out.contains("… and 17 more"));
        assert!(out.contains("initial commit"));
    }

    #[test]
    fn detail_pane_shows_loading_placeholder() {
        let app = app();
        let out = render(&app);
        assert!(out.contains("loading details…"));
    }

    #[test]
    fn confirm_remove_modal_renders_force_state() {
        let mut app = app();
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('d'));
        let out = render(&app);
        assert!(out.contains("Remove worktree 'dirty-feat'?"));
        assert!(out.contains("uncommitted changes"));
        assert!(out.contains("force: [ ]"));
        press(&mut app, KeyCode::Char('f'));
        let out = render(&app);
        assert!(out.contains("force: [x]"));
    }

    #[test]
    fn confirm_prune_modal_lists_candidates() {
        let mut app = app();
        press(&mut app, KeyCode::Char('p'));
        let out = render(&app);
        assert!(out.contains("Prune 1 worktree(s)?"));
        assert!(out.contains("done"));
        assert!(out.contains("[merged]"));
        assert!(out.contains("+ delete branch"));
    }

    #[test]
    fn create_form_renders_fields_and_prefill() {
        let mut app = app();
        press(&mut app, KeyCode::Char('n'));
        let out = render(&app);
        assert!(out.contains("New worktree"));
        assert!(out.contains("branch"));
        assert!(out.contains("origin/main"), "base prefilled from config");
        assert!(out.contains("empty base = HEAD"));
    }

    #[test]
    fn help_overlay_lists_bindings() {
        let mut app = app();
        press(&mut app, KeyCode::Char('?'));
        let out = render(&app);
        assert!(out.contains("Help"));
        assert!(out.contains("toggle multi-select"));
        assert!(out.contains("copy path to clipboard"));
        assert!(out.contains("fuzzy filter"));
    }

    #[test]
    fn filter_and_loading_indicator_show_in_title() {
        let mut app = app();
        app.status_loading = true;
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('d'));
        let out = render(&app);
        assert!(out.contains("/d"));
        assert!(out.contains("loading status…"));
    }
}
