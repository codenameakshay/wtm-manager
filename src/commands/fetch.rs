//! `wtm fetch` — update remote-tracking refs so ahead/behind and
//! upstream-gone status are honest.
//!
//! Shells out to `git fetch --prune` rather than git2 so SSH agents,
//! keychains, and `credential.helper` keep working.

use crate::cli::{FetchArgs, GlobalArgs};
use crate::error::{Error, Result};
use crate::gitcmd;
use crate::repo::RepoContext;

/// Result of a fetch: which remote ran and how many refs git reported as
/// updated, created, or deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchOutcome {
    pub remote: String,
    pub updated_refs: usize,
}

/// Fetch the default remote (or `--remote`) and print a one-line summary.
pub fn run(args: &FetchArgs, global: &GlobalArgs) -> Result<()> {
    let (ctx, _) = super::prepare(global)?;
    let outcome = fetch(&ctx, args.remote.as_deref())?;
    if !global.quiet {
        if outcome.updated_refs == 0 {
            println!("fetched {} (no refs updated)", outcome.remote);
        } else {
            println!(
                "fetched {} ({} ref{})",
                outcome.remote,
                outcome.updated_refs,
                if outcome.updated_refs == 1 { "" } else { "s" }
            );
        }
    }
    Ok(())
}

/// Run `git fetch --prune` against `remote`, or the default remote when
/// `None`.
pub fn fetch(ctx: &RepoContext, remote: Option<&str>) -> Result<FetchOutcome> {
    let remote_name = match remote {
        Some(r) => r.to_string(),
        None => default_remote_name(ctx)?,
    };

    let output = gitcmd::run_capture(&ctx.main_root, &["fetch", "--prune", &remote_name])?;

    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() {
        let trimmed = combined.trim();
        return Err(Error::GitCommand {
            args: format!("fetch --prune {remote_name}"),
            status: output.status.to_string(),
            stderr: if trimmed.is_empty() {
                format!("git fetch exited with {}", output.status)
            } else {
                trimmed.to_string()
            },
        });
    }

    Ok(FetchOutcome {
        updated_refs: count_updated_refs(&combined),
        remote: remote_name,
    })
}

/// `origin` if configured, else the first remote name alphabetically.
pub fn default_remote_name(ctx: &RepoContext) -> Result<String> {
    let git_repo = ctx.open_main()?;
    let mut names: Vec<String> = git_repo
        .remotes()?
        .iter()
        .filter_map(|entry| entry.ok().flatten())
        .map(str::to_owned)
        .collect();
    if names.iter().any(|n| n == "origin") {
        return Ok("origin".to_string());
    }
    names.sort();
    names
        .into_iter()
        .next()
        .ok_or_else(|| Error::Other("this repository has no configured remotes".to_string()))
}

fn count_updated_refs(output: &str) -> usize {
    output.lines().filter(|line| line.contains(" -> ")).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgit::{git, init_repo};
    use std::path::PathBuf;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::TempDir::new().unwrap();
        let main = tmp.path().join("main");
        init_repo(&main);
        (tmp, main)
    }

    #[test]
    fn count_updated_refs_counts_arrow_lines_only() {
        let output = "\
From github.com:owner/repo
   1234abc..5678def  main       -> origin/main
 * [new branch]      feat       -> origin/feat
 - [deleted]         (none)     -> origin/old
Fetching origin
";
        assert_eq!(count_updated_refs(output), 3);
    }

    #[test]
    fn count_updated_refs_is_honestly_zero_when_nothing_changed() {
        assert_eq!(count_updated_refs("From github.com:owner/repo\n"), 0);
        assert_eq!(count_updated_refs(""), 0);
    }

    #[test]
    fn default_remote_name_prefers_origin() {
        let (_tmp, main) = fixture();
        git(
            &main,
            &[
                "remote",
                "add",
                "zzz-other",
                "https://example.invalid/z.git",
            ],
        );
        git(
            &main,
            &["remote", "add", "origin", "https://example.invalid/o.git"],
        );
        let ctx = crate::repo::discover(Some(&main)).unwrap();
        assert_eq!(default_remote_name(&ctx).unwrap(), "origin");
    }

    #[test]
    fn default_remote_name_falls_back_to_first_alphabetically() {
        let (_tmp, main) = fixture();
        git(
            &main,
            &["remote", "add", "zzz", "https://example.invalid/z.git"],
        );
        git(
            &main,
            &["remote", "add", "aaa", "https://example.invalid/a.git"],
        );
        let ctx = crate::repo::discover(Some(&main)).unwrap();
        assert_eq!(default_remote_name(&ctx).unwrap(), "aaa");
    }

    #[test]
    fn default_remote_name_errors_clearly_with_no_remotes() {
        let (_tmp, main) = fixture();
        let ctx = crate::repo::discover(Some(&main)).unwrap();
        let err = default_remote_name(&ctx).unwrap_err();
        assert!(err.to_string().contains("no configured remotes"), "{err}");
    }

    #[test]
    fn fetch_from_a_local_path_remote_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        let origin = tmp.path().join("origin");
        let clone = tmp.path().join("clone");
        init_repo(&origin);
        git(
            tmp.path(),
            &["clone", origin.to_str().unwrap(), clone.to_str().unwrap()],
        );
        crate::testgit::commit_file(&origin, "from-origin.txt");

        let ctx = crate::repo::discover(Some(&clone)).unwrap();
        let outcome = fetch(&ctx, None).unwrap();
        assert_eq!(outcome.remote, "origin");
        assert!(outcome.updated_refs >= 1, "{outcome:?}");
    }
}
