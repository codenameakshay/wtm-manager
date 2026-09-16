//! `wtm host` — manage saved remote hosts and act on their repositories and
//! worktrees over ssh.

use std::path::Path;

use crate::cli::{GlobalArgs, HostArgs, HostCommand};
use crate::commands::prune::PruneCandidate;
use crate::error::{Error, Result};
use crate::output;
use crate::remote::{self, Host, RemoteRepo, RemoteWorktree};

pub fn run(args: &HostArgs, global: &GlobalArgs) -> Result<()> {
    match &args.command {
        HostCommand::Add {
            name,
            destination,
            roots,
        } => add(name, destination, roots.clone(), global),
        HostCommand::List { json } => list(*json),
        HostCommand::Forget { name } => forget(name, global),
        HostCommand::Scan {
            name,
            json,
            no_size,
        } => scan(name, *json, *no_size, global),
        HostCommand::Rm {
            name,
            path,
            force,
            with_branch,
            json,
        } => rm(name, path, *force, *with_branch, *json, global),
        HostCommand::Prune {
            name,
            merged,
            gone,
            detached,
            in_repo,
            dry_run,
            force,
            json,
        } => prune(
            name,
            *merged,
            *gone,
            *detached,
            in_repo.as_deref(),
            *dry_run,
            *force,
            *json,
            global,
        ),
    }
}

fn add(name: &str, destination: &str, roots: Vec<String>, global: &GlobalArgs) -> Result<()> {
    let host = Host::new(name, destination, roots)?;
    if !global.quiet {
        warn_about_locally_expanded_roots(&host.roots);
    }
    let (name, destination) = (host.name.clone(), host.destination.clone());
    remote::upsert_host(host)?;
    if !global.quiet {
        eprintln!("saved host {name} ({destination})");
    }
    Ok(())
}

/// An unquoted `--root ~/x` reaches wtm already expanded to the local home,
/// which is rarely a path on the host.
fn warn_about_locally_expanded_roots(roots: &[String]) {
    let Some(dirs) = directories::BaseDirs::new() else {
        return;
    };
    for root in roots {
        if let Ok(rest) = Path::new(root).strip_prefix(dirs.home_dir()) {
            eprintln!(
                "warning: root {root} is under your local home directory; to mean the remote home, quote it: --root '~/{}'",
                rest.display()
            );
        }
    }
}

fn list(json: bool) -> Result<()> {
    let hosts = remote::load_hosts();
    if json {
        output::print_json(&hosts);
        return Ok(());
    }
    if hosts.is_empty() {
        eprintln!("no hosts saved (add one with `wtm host add <name> <destination>`)");
        return Ok(());
    }
    let name_w = hosts.iter().map(|h| h.name.len()).max().unwrap_or(0);
    let dest_w = hosts.iter().map(|h| h.destination.len()).max().unwrap_or(0);
    for h in &hosts {
        let roots = roots_display(&h.roots);
        println!("{:<name_w$}  {:<dest_w$}  {roots}", h.name, h.destination);
    }
    Ok(())
}

fn forget(name: &str, global: &GlobalArgs) -> Result<()> {
    if !remote::forget_host(name)? {
        return Err(Error::Other(format!("no host named '{name}'")));
    }
    if !global.quiet {
        eprintln!("forgot host {name}");
    }
    Ok(())
}

fn roots_display(roots: &[String]) -> String {
    if roots.is_empty() {
        "~".to_string()
    } else {
        roots.join(", ")
    }
}

fn scan(name: &str, json: bool, no_size: bool, global: &GlobalArgs) -> Result<()> {
    let host = remote::find_host(name)?;
    let with_sizes = !no_size;
    let mut repos = remote::scan(&host, with_sizes)?;
    remote::sort_by_size(&mut repos);

    if json {
        output::print_json(&repos);
        return Ok(());
    }

    if repos.is_empty() {
        if !global.quiet {
            eprintln!(
                "no repositories found under {} on {}",
                roots_display(&host.roots),
                host.name
            );
        }
        return Ok(());
    }

    print_scan_table(&repos, with_sizes);

    if !global.quiet {
        let repo_count = repos.len();
        let worktree_count: usize = repos.iter().map(|r| r.worktrees.len()).sum();
        if with_sizes {
            let total: u64 = repos.iter().map(|r| r.size_bytes()).sum();
            eprintln!(
                "{}: {repo_count} repositories, {worktree_count} worktrees, {} on disk",
                host.name,
                remote::format_size(total)
            );
        } else {
            eprintln!(
                "{}: {repo_count} repositories, {worktree_count} worktrees",
                host.name
            );
        }
    }
    Ok(())
}

fn print_scan_table(repos: &[RemoteRepo], with_sizes: bool) {
    for repo in repos {
        let size = if with_sizes {
            remote::format_size(repo.size_bytes())
        } else {
            "-".to_string()
        };
        println!(
            "{}  {}  {} worktree(s)  {}",
            repo.name,
            size,
            repo.worktrees.len(),
            repo.path.display()
        );

        let name_w = repo
            .worktrees
            .iter()
            .map(|w| w.info.display_name().len())
            .max()
            .unwrap_or(0);
        let size_w = repo
            .worktrees
            .iter()
            .map(|w| worktree_size_cell(w).len())
            .max()
            .unwrap_or(0);
        let status_w = repo
            .worktrees
            .iter()
            .map(|w| status_summary(w).len())
            .max()
            .unwrap_or(0);
        for w in &repo.worktrees {
            println!(
                "  {:<name_w$}  {:<size_w$}  {:<status_w$}  {}",
                w.info.display_name(),
                worktree_size_cell(w),
                status_summary(w),
                w.info.path.display(),
            );
        }
    }
}

fn worktree_size_cell(w: &RemoteWorktree) -> String {
    match w.size_bytes {
        Some(bytes) => remote::format_size(bytes),
        None => "-".to_string(),
    }
}

/// Comma-joined status labels for one scanned worktree, in the fixed order:
/// main, missing, prunable, locked, dirty count, ahead, behind, gone, merged.
fn status_summary(w: &RemoteWorktree) -> String {
    let info = &w.info;
    let mut labels: Vec<String> = Vec::new();
    if info.is_main {
        labels.push("main".to_string());
    }
    if info.is_missing {
        labels.push("missing".to_string());
    }
    if info.is_prunable {
        labels.push("prunable".to_string());
    }
    if info.is_locked {
        labels.push("locked".to_string());
    }
    if let Some(status) = &info.status {
        if status.dirty_count > 0 {
            labels.push(format!("{} dirty", status.dirty_count));
        }
        if let Some(ahead) = status.ahead.filter(|a| *a > 0) {
            labels.push(format!("ahead {ahead}"));
        }
        if let Some(behind) = status.behind.filter(|b| *b > 0) {
            labels.push(format!("behind {behind}"));
        }
        if status.upstream_gone {
            labels.push("gone".to_string());
        }
        if status.merged {
            labels.push("merged".to_string());
        }
    }
    if labels.is_empty() {
        if info.status.is_some() {
            "clean".to_string()
        } else {
            "-".to_string()
        }
    } else {
        labels.join(", ")
    }
}

fn rm(
    name: &str,
    path: &Path,
    force: bool,
    with_branch: bool,
    json: bool,
    global: &GlobalArgs,
) -> Result<()> {
    let host = remote::find_host(name)?;
    let repos = remote::scan(&host, false)?;

    let mut found: Option<(RemoteRepo, RemoteWorktree)> = None;
    'search: for repo in &repos {
        for w in &repo.worktrees {
            if w.info.path == path {
                found = Some((repo.clone(), w.clone()));
                break 'search;
            }
        }
    }
    let Some((repo, worktree)) = found else {
        return Err(Error::Other(format!(
            "no worktree at {} on {}",
            path.display(),
            host.name
        )));
    };
    if worktree.info.is_main {
        return Err(Error::MainWorktree {
            action: "remove".to_string(),
        });
    }

    let branch = worktree.info.branch.clone();
    if let Some(b) = branch.as_deref().filter(|_| with_branch) {
        if repo.protected_branches.iter().any(|p| p == b) {
            return Err(Error::ProtectedBranch(b.to_string()));
        }
    }
    let display_name = worktree.info.display_name().to_string();
    let delete_branch = with_branch && branch.is_some();
    let candidate = PruneCandidate {
        info: worktree.info.clone(),
        reasons: Vec::new(),
        delete_branch,
    };
    let outcome = remote::remove_worktrees(&host, &repo.path, &[candidate], force)?
        .into_iter()
        .next()
        .expect("one target produces one outcome");

    if !outcome.removed {
        return Err(Error::Other(
            outcome
                .message
                .unwrap_or_else(|| "remove failed".to_string()),
        ));
    }
    let branch_deleted = delete_branch && outcome.message.is_none();

    if json {
        output::print_json(&serde_json::json!({
            "ok": true,
            "action": "remove",
            "host": host.name,
            "name": display_name,
            "branch": branch,
            "path": worktree.info.path,
            "branch_deleted": branch_deleted,
        }));
    } else {
        if !global.quiet {
            eprintln!(
                "removed {display_name} ({}) on {}",
                worktree.info.path.display(),
                host.name
            );
        }
        if let Some(message) = &outcome.message {
            eprintln!("{message}");
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn prune(
    name: &str,
    merged: bool,
    gone: bool,
    detached: bool,
    in_repo: Option<&Path>,
    dry_run: bool,
    force: bool,
    json: bool,
    global: &GlobalArgs,
) -> Result<()> {
    let host = remote::find_host(name)?;
    let repos_all = remote::scan(&host, dry_run)?;
    let repos: Vec<&RemoteRepo> = if let Some(target) = in_repo {
        let filtered: Vec<&RemoteRepo> = repos_all.iter().filter(|r| r.path == target).collect();
        if filtered.is_empty() {
            return Err(Error::Other(format!(
                "no repository at {} on {}",
                target.display(),
                host.name
            )));
        }
        filtered
    } else {
        repos_all.iter().collect()
    };

    let mut plan: Vec<(&RemoteRepo, Vec<PruneCandidate>, Vec<PruneCandidate>)> = Vec::new();
    for repo in repos {
        let candidates = remote::prune_candidates(repo, merged, gone, detached);
        let (kept, skipped) = if force {
            (candidates, Vec::new())
        } else {
            candidates
                .into_iter()
                .partition(|c| !c.info.status.as_ref().is_some_and(|s| s.dirty))
        };
        plan.push((repo, kept, skipped));
    }

    let candidate_names: Vec<String> = plan
        .iter()
        .flat_map(|(_, kept, skipped)| kept.iter().chain(skipped.iter()))
        .map(|c| c.info.display_name().to_string())
        .collect();
    let skipped_names: Vec<String> = plan
        .iter()
        .flat_map(|(_, _, skipped)| skipped.iter())
        .map(|c| c.info.display_name().to_string())
        .collect();

    if candidate_names.is_empty() {
        if json {
            output::print_json(&serde_json::json!({
                "ok": true,
                "action": "prune",
                "host": host.name,
                "removed": 0,
                "skipped": Vec::<String>::new(),
                "failures": Vec::<String>::new(),
                "candidates": Vec::<String>::new(),
            }));
        } else if !global.quiet {
            eprintln!("nothing to prune on {}", host.name);
        }
        return Ok(());
    }

    if dry_run {
        let reclaimable_bytes: u64 = plan
            .iter()
            .flat_map(|(repo, kept, _)| {
                kept.iter().filter_map(move |c| {
                    repo.worktrees
                        .iter()
                        .find(|w| w.info.path == c.info.path)
                        .and_then(|w| w.size_bytes)
                })
            })
            .sum();

        if json {
            output::print_json(&serde_json::json!({
                "ok": true,
                "action": "prune",
                "host": host.name,
                "removed": 0,
                "skipped": skipped_names,
                "failures": Vec::<String>::new(),
                "candidates": candidate_names,
                "reclaimable_bytes": reclaimable_bytes,
            }));
        } else {
            println!(
                "Would prune {} worktree(s) on {}, freeing about {}:",
                candidate_names.len() - skipped_names.len(),
                host.name,
                remote::format_size(reclaimable_bytes)
            );
            for (_, kept, skipped) in &plan {
                for c in kept {
                    println!(
                        "  {} ({}) [{}]{}",
                        c.info.display_name(),
                        c.info.path.display(),
                        c.reasons.join(", "),
                        if c.delete_branch {
                            " + delete branch"
                        } else {
                            ""
                        }
                    );
                }
                for c in skipped {
                    println!(
                        "  skip {} ({}): uncommitted changes (use --force)",
                        c.info.display_name(),
                        c.info.path.display()
                    );
                }
            }
        }
        return Ok(());
    }

    let mut removed = 0usize;
    let mut failures: Vec<String> = Vec::new();
    for (repo, kept, _skipped) in &plan {
        if kept.is_empty() {
            continue;
        }
        let outcomes = match remote::remove_worktrees(&host, &repo.path, kept, force) {
            Ok(outcomes) => outcomes,
            Err(e) => {
                for c in kept {
                    failures.push(format!("{}: {e}", c.info.display_name()));
                }
                continue;
            }
        };
        for (c, outcome) in kept.iter().zip(outcomes) {
            if outcome.removed {
                removed += 1;
                if !json {
                    println!(
                        "removed {} ({})",
                        c.info.display_name(),
                        c.info.path.display()
                    );
                }
                if let Some(message) = outcome.message {
                    failures.push(format!("{}: {message}", c.info.display_name()));
                }
            } else {
                failures.push(format!(
                    "{}: {}",
                    c.info.display_name(),
                    outcome.message.unwrap_or_else(|| "failed".to_string())
                ));
            }
        }
    }

    if json {
        output::print_json(&serde_json::json!({
            "ok": failures.is_empty(),
            "action": "prune",
            "host": host.name,
            "removed": removed,
            "skipped": skipped_names,
            "failures": failures,
            "candidates": candidate_names,
        }));
    } else if !global.quiet {
        eprintln!("pruned {removed} worktree(s) on {}", host.name);
    }

    if !failures.is_empty() {
        return Err(Error::Other(format!(
            "prune completed with {} failure(s): {}",
            failures.len(),
            failures.join("; ")
        )));
    }
    Ok(())
}
