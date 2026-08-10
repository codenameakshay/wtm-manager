//! Output rendering: color decisions, the human table for `wtm list`, and
//! JSON output.

use std::ffi::OsStr;
use std::io::IsTerminal;
use std::path::Path;

use comfy_table::{Cell, Color, ContentArrangement, Table};

use crate::model::WorktreeInfo;

/// When to use colored output on stdout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorMode {
    /// Color only when stdout is a terminal and `NO_COLOR` is unset.
    Auto,
    /// Always emit color escapes.
    Always,
    /// Never emit color escapes.
    Never,
}

/// True when color should be used on stdout: `Always` ⇒ true; `Never` ⇒
/// false; `Auto` ⇒ stdout is a TTY and the `NO_COLOR` environment variable
/// is unset or empty.
pub fn use_color(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => auto_color(
            std::env::var_os("NO_COLOR").as_deref(),
            std::io::stdout().is_terminal(),
        ),
    }
}

/// Pure decision core for [`ColorMode::Auto`], split out for unit testing.
fn auto_color(no_color: Option<&OsStr>, stdout_is_tty: bool) -> bool {
    stdout_is_tty && no_color.is_none_or(|v| v.is_empty())
}

/// Render the human-readable table for `wtm list`.
///
/// Columns: NAME (branch or registry name, the main worktree marked with
/// `*`), PATH (with `~` abbreviation), HEAD, AHEAD/BEHIND (`↑2 ↓1`, `-`
/// when there is no upstream, `gone` when the upstream vanished), STATUS
/// (badges: dirty/merged/missing/locked/prunable). When `with_status` is
/// false the status-derived columns are omitted entirely.
pub fn render_table(items: &[WorktreeInfo], color: bool, with_status: bool) -> String {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::NOTHING);
    table.set_content_arrangement(ContentArrangement::Dynamic);
    if color {
        table.enforce_styling();
    } else {
        table.force_no_tty();
    }

    let mut header = vec!["NAME", "PATH", "HEAD"];
    if with_status {
        header.push("AHEAD/BEHIND");
        header.push("STATUS");
    }
    table.set_header(header);

    for info in items {
        let name = if info.is_main {
            format!("{} *", info.display_name())
        } else {
            info.display_name().to_string()
        };
        let mut row = vec![
            Cell::new(name),
            Cell::new(abbreviate_path(&info.path, home.as_deref())),
            Cell::new(info.head.as_deref().unwrap_or("-")),
        ];
        if with_status {
            row.push(ahead_behind_cell(info));
            row.push(status_cell(info));
        }
        table.add_row(row);
    }

    table.to_string()
}

/// Render the `--json` output: a pretty-printed array of [`WorktreeInfo`]
/// with the stable field names declared in `model.rs`.
pub fn render_json(items: &[WorktreeInfo]) -> String {
    serde_json::to_string_pretty(items).expect(
        "WorktreeInfo serialization is infallible (plain data, no maps with non-string keys)",
    )
}

/// Replace a leading `$HOME` prefix with `~`.
fn abbreviate_path(path: &Path, home: Option<&Path>) -> String {
    if let Some(home) = home {
        if let Ok(rest) = path.strip_prefix(home) {
            return if rest.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", rest.display())
            };
        }
    }
    path.display().to_string()
}

/// The AHEAD/BEHIND cell: `gone` when the upstream vanished, `↑a ↓b` when an
/// upstream exists, `-` otherwise (no upstream, or status not computed).
fn ahead_behind_cell(info: &WorktreeInfo) -> Cell {
    match &info.status {
        Some(s) if s.upstream_gone => Cell::new("gone").fg(Color::Red),
        Some(s) => match (s.ahead, s.behind) {
            (Some(a), Some(b)) => {
                let cell = Cell::new(format!("↑{a} ↓{b}"));
                if a > 0 || b > 0 {
                    cell.fg(Color::Cyan)
                } else {
                    cell
                }
            }
            _ => Cell::new("-"),
        },
        None if info.is_missing => Cell::new("-"),
        None => Cell::new("unavailable").fg(Color::Red),
    }
}

/// The STATUS cell: space-separated badges. The whole cell is tinted by the
/// most significant badge (missing > dirty > prunable > locked > merged);
/// comfy-table styles per cell, not per word.
fn status_cell(info: &WorktreeInfo) -> Cell {
    if info.status.is_none() && !info.is_missing {
        return Cell::new("unavailable").fg(Color::Red);
    }
    let mut badges: Vec<&str> = Vec::new();
    if let Some(s) = &info.status {
        if s.dirty {
            badges.push("dirty");
        }
        if s.merged {
            badges.push("merged");
        }
    }
    if info.is_missing {
        badges.push("missing");
    }
    if info.is_locked {
        badges.push("locked");
    }
    if info.is_prunable {
        badges.push("prunable");
    }
    if badges.is_empty() {
        return Cell::new("-");
    }
    let text = badges.join(" ");
    let color = if info.is_missing {
        Color::Red
    } else if info.status.as_ref().is_some_and(|s| s.dirty) {
        Color::Yellow
    } else if info.is_prunable {
        Color::Magenta
    } else if info.is_locked {
        Color::Blue
    } else {
        // Only "merged" remains possible here.
        Color::Green
    };
    Cell::new(text).fg(color)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;
    use crate::model::{WorktreeInfo, WorktreeStatus};

    fn info(name: &str, branch: Option<&str>, is_main: bool) -> WorktreeInfo {
        WorktreeInfo {
            name: name.to_string(),
            path: PathBuf::from(format!("/work/{name}")),
            branch: branch.map(str::to_string),
            head: Some("abc1234".to_string()),
            is_main,
            is_missing: false,
            is_locked: false,
            is_prunable: false,
            status: None,
        }
    }

    fn status(
        dirty: bool,
        ahead: Option<usize>,
        behind: Option<usize>,
        upstream_gone: bool,
        merged: bool,
    ) -> WorktreeStatus {
        WorktreeStatus {
            dirty,
            ahead,
            behind,
            upstream_gone,
            merged,
        }
    }

    #[test]
    fn auto_color_requires_tty() {
        assert!(!auto_color(None, false));
        assert!(auto_color(None, true));
    }

    #[test]
    fn auto_color_respects_no_color() {
        let set = OsString::from("1");
        assert!(!auto_color(Some(set.as_os_str()), true));
        // An empty NO_COLOR counts as unset per the informal spec.
        let empty = OsString::from("");
        assert!(auto_color(Some(empty.as_os_str()), true));
    }

    #[test]
    fn use_color_always_and_never_ignore_environment() {
        assert!(use_color(ColorMode::Always));
        assert!(!use_color(ColorMode::Never));
    }

    #[test]
    fn abbreviates_home_prefix() {
        let home = PathBuf::from("/Users/me");
        assert_eq!(
            abbreviate_path(Path::new("/Users/me/dev/x"), Some(&home)),
            "~/dev/x"
        );
        assert_eq!(abbreviate_path(Path::new("/Users/me"), Some(&home)), "~");
        assert_eq!(
            abbreviate_path(Path::new("/opt/dev/x"), Some(&home)),
            "/opt/dev/x"
        );
        assert_eq!(abbreviate_path(Path::new("/opt/dev/x"), None), "/opt/dev/x");
    }

    #[test]
    fn table_marks_main_and_shows_ahead_behind() {
        let mut main = info("main", Some("main"), true);
        main.status = Some(status(false, Some(0), Some(0), false, false));
        let mut feat = info("feature-x", Some("feature/x"), false);
        feat.status = Some(status(true, Some(2), Some(1), false, false));
        let rendered = render_table(&[main, feat], false, true);

        assert!(rendered.contains("NAME"));
        assert!(rendered.contains("AHEAD/BEHIND"));
        assert!(rendered.contains("STATUS"));
        assert!(rendered.contains("main *"));
        assert!(rendered.contains("feature/x"));
        assert!(rendered.contains("↑2 ↓1"));
        assert!(rendered.contains("↑0 ↓0"));
        assert!(rendered.contains("dirty"));
        assert!(rendered.contains("abc1234"));
        // No ANSI escapes when color is off.
        assert!(!rendered.contains('\u{1b}'));
    }

    #[test]
    fn table_shows_gone_and_missing_badges() {
        let mut gone = info("old", Some("old-branch"), false);
        gone.status = Some(status(false, None, None, true, true));
        let mut missing = info("lost", None, false);
        missing.is_missing = true;
        missing.head = None;
        let rendered = render_table(&[gone, missing], false, true);

        assert!(rendered.contains("gone"));
        assert!(rendered.contains("merged"));
        assert!(rendered.contains("missing"));
        // Missing worktree without a branch falls back to the registry name.
        assert!(rendered.contains("lost"));
    }

    #[test]
    fn table_without_status_omits_status_columns() {
        let rendered = render_table(&[info("main", Some("main"), true)], false, false);
        assert!(rendered.contains("NAME"));
        assert!(rendered.contains("HEAD"));
        assert!(!rendered.contains("AHEAD/BEHIND"));
        assert!(!rendered.contains("STATUS"));
    }

    #[test]
    fn table_with_color_emits_ansi() {
        let mut dirty = info("d", Some("d"), false);
        dirty.status = Some(status(true, None, None, false, false));
        let rendered = render_table(&[dirty], true, true);
        assert!(rendered.contains('\u{1b}'));
    }

    #[test]
    fn json_is_a_stable_pretty_array() {
        let mut item = info("feature-x", Some("feature/x"), false);
        item.status = Some(status(true, Some(2), Some(1), false, false));
        let json = render_json(&[item]);
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let arr = value.as_array().expect("top-level array");
        assert_eq!(arr.len(), 1);
        let obj = &arr[0];
        assert_eq!(obj["name"], "feature-x");
        assert_eq!(obj["branch"], "feature/x");
        assert_eq!(obj["is_main"], false);
        assert_eq!(obj["status"]["dirty"], true);
        assert_eq!(obj["status"]["ahead"], 2);
        assert_eq!(obj["status"]["behind"], 1);
        // Pretty-printed (multi-line) output.
        assert!(json.contains('\n'));
    }

    #[test]
    fn json_of_empty_list_is_empty_array() {
        assert_eq!(render_json(&[]), "[]");
    }
}
