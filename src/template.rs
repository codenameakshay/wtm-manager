//! Path template rendering for newly created worktrees.
//!
//! Templates are small strings containing `{placeholder}` markers that get
//! substituted with values known at worktree-creation time (repo name,
//! branch, etc.). Rendering never touches the filesystem: relative results
//! are joined onto the main worktree root and the whole thing is normalized
//! lexically (no symlink resolution, no existence checks).

use std::path::{Component, Path, PathBuf};

use crate::error::{Error, Result};

/// Values available for placeholder substitution in [`render`].
pub struct TemplateContext<'a> {
    /// `{repo}` — the main working tree's directory name.
    pub repo_name: &'a str,
    /// `{branch}` — the raw branch name, verbatim; may contain `/`.
    pub branch: &'a str,
    /// `{repo_dir}`; also the directory relative render results are joined onto.
    pub main_root: &'a Path,
}

/// Render `template`, substituting `{repo}`, `{branch}`, `{slug}`, `{home}`
/// and `{repo_dir}`. An unknown `{placeholder}` produces `Error::Template`.
/// A relative rendered path is joined onto `ctx.main_root`; the result
/// (relative or absolute) is then lexically normalized via [`normalize`] —
/// no filesystem access is performed.
pub fn render(template: &str, ctx: &TemplateContext<'_>) -> Result<PathBuf> {
    let rendered = substitute(template, ctx)?;
    let path = PathBuf::from(rendered);
    let joined = if path.is_absolute() {
        path
    } else {
        ctx.main_root.join(path)
    };
    Ok(normalize(&joined))
}

/// Filesystem-safe branch slug: `/` and any character outside
/// `[A-Za-z0-9._-]` become `-`, runs of `-` collapse to one, leading/trailing
/// `-` are trimmed. Never empty (falls back to `"branch"`). Case preserved.
pub fn slugify(branch: &str) -> String {
    let mut out = String::with_capacity(branch.len());
    let mut last_was_dash = false;
    for ch in branch.chars() {
        let mapped = if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            ch
        } else {
            '-'
        };
        if mapped == '-' {
            if last_was_dash {
                continue;
            }
            last_was_dash = true;
        } else {
            last_was_dash = false;
        }
        out.push(mapped);
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "branch".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Lexically normalize `path`: resolve `.` and `..` components without any
/// filesystem access. A `..` past the root (or past the start of a relative
/// path) is preserved verbatim rather than erroring.
pub fn normalize(path: &Path) -> PathBuf {
    let mut stack: Vec<Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match stack.last() {
                Some(Component::Normal(_)) => {
                    stack.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {
                    // ".." above the root is a no-op.
                }
                _ => stack.push(component),
            },
            other => stack.push(other),
        }
    }
    if stack.is_empty() {
        PathBuf::from(".")
    } else {
        stack.into_iter().collect()
    }
}

/// Substitute every `{placeholder}` in `template`, leaving all other text
/// untouched.
fn substitute(template: &str, ctx: &TemplateContext<'_>) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after_brace = &rest[start + 1..];
        let end = after_brace.find('}').ok_or_else(|| {
            Error::Template(format!("unterminated placeholder in template '{template}'"))
        })?;
        let name = &after_brace[..end];
        out.push_str(&resolve_placeholder(name, ctx)?);
        rest = &after_brace[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn resolve_placeholder(name: &str, ctx: &TemplateContext<'_>) -> Result<String> {
    match name {
        "repo" => Ok(ctx.repo_name.to_string()),
        "branch" => Ok(ctx.branch.to_string()),
        "slug" => Ok(slugify(ctx.branch)),
        "home" => directories::BaseDirs::new()
            .map(|dirs| dirs.home_dir().to_string_lossy().into_owned())
            .ok_or_else(|| Error::Template("could not resolve {home}: no home directory".into())),
        "repo_dir" => Ok(ctx.main_root.to_string_lossy().into_owned()),
        other => Err(Error::Template(format!(
            "unknown placeholder '{{{other}}}'"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx<'a>(repo_name: &'a str, branch: &'a str, main_root: &'a Path) -> TemplateContext<'a> {
        TemplateContext {
            repo_name,
            branch,
            main_root,
        }
    }

    #[test]
    fn renders_basic_placeholders() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "feature-x", root);
        let result = render("../{repo}-worktrees/{branch}", &c).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/Users/foo/myrepo-worktrees/feature-x")
        );
    }

    #[test]
    fn renders_nested_branch_names() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "feat/foo", root);
        let result = render("../{repo}-worktrees/{branch}", &c).unwrap();
        assert_eq!(
            result,
            PathBuf::from("/Users/foo/myrepo-worktrees/feat/foo")
        );
    }

    #[test]
    fn renders_deeply_nested_branch_names() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "team/feat/foo-bar", root);
        let result = render("{repo_dir}/../wt/{branch}", &c).unwrap();
        assert_eq!(result, PathBuf::from("/Users/foo/wt/team/feat/foo-bar"));
    }

    #[test]
    fn renders_slug_placeholder() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "feat/foo bar!", root);
        let result = render("{repo_dir}/{slug}", &c).unwrap();
        assert_eq!(result, PathBuf::from("/Users/foo/myrepo/feat-foo-bar"));
    }

    #[test]
    fn renders_repo_dir_placeholder_absolute() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "main", root);
        let result = render("{repo_dir}", &c).unwrap();
        assert_eq!(result, PathBuf::from("/Users/foo/myrepo"));
    }

    #[test]
    fn renders_home_placeholder() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "main", root);
        let result = render("{home}/worktrees/{repo}/{branch}", &c).unwrap();
        let home = directories::BaseDirs::new()
            .unwrap()
            .home_dir()
            .to_path_buf();
        assert_eq!(result, home.join("worktrees/myrepo/main"));
    }

    #[test]
    fn unknown_placeholder_errors() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "main", root);
        let err = render("{repo}/{oops}", &c).unwrap_err();
        match err {
            Error::Template(msg) => assert!(msg.contains("oops"), "message was: {msg}"),
            other => panic!("expected Error::Template, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_placeholder_errors() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "main", root);
        let err = render("{repo", &c).unwrap_err();
        assert!(matches!(err, Error::Template(_)));
    }

    #[test]
    fn slugify_replaces_slashes_and_collapses_runs() {
        assert_eq!(slugify("feat/foo"), "feat-foo");
        assert_eq!(slugify("feat//foo"), "feat-foo");
        assert_eq!(slugify("feat/ foo bar"), "feat-foo-bar");
    }

    #[test]
    fn slugify_preserves_case_and_allowed_chars() {
        assert_eq!(slugify("Feature_1.2-final"), "Feature_1.2-final");
    }

    #[test]
    fn slugify_trims_leading_and_trailing_dashes() {
        assert_eq!(slugify("/feat/foo/"), "feat-foo");
        assert_eq!(slugify("---feat---"), "feat");
    }

    #[test]
    fn slugify_never_empty() {
        assert_eq!(slugify("///"), "branch");
        assert_eq!(slugify(""), "branch");
        assert_eq!(slugify("!!!"), "branch");
    }

    #[test]
    fn normalize_resolves_parent_dir_components() {
        assert_eq!(normalize(Path::new("/a/b/../c")), PathBuf::from("/a/c"));
        assert_eq!(normalize(Path::new("a/./b/../../c")), PathBuf::from("c"));
    }

    #[test]
    fn normalize_preserves_leading_parent_dirs_on_relative_paths() {
        assert_eq!(normalize(Path::new("../a/b")), PathBuf::from("../a/b"));
        assert_eq!(normalize(Path::new("../../a")), PathBuf::from("../../a"));
    }

    #[test]
    fn normalize_parent_dir_above_root_is_noop() {
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"));
    }

    #[test]
    fn normalize_empty_relative_result_is_current_dir() {
        assert_eq!(normalize(Path::new("a/..")), PathBuf::from("."));
    }

    #[test]
    fn render_normalizes_absolute_results() {
        let root = Path::new("/Users/foo/myrepo");
        let c = ctx("myrepo", "main", root);
        let result = render("/tmp/../var/{repo}", &c).unwrap();
        assert_eq!(result, PathBuf::from("/var/myrepo"));
    }
}
