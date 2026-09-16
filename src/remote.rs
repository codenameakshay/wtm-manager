//! Remote hosts: repositories and worktrees on another machine, reached
//! with the system `ssh` binary.
//!
//! Nothing is installed on the host. Each operation is ONE `ssh <host> sh -s`
//! round trip that pipes a POSIX shell script on stdin; the script runs
//! plain `git` (and `du`) on the host and prints tab-separated records that
//! `parse_scan` / `parse_remove` turn back into typed values. Using the
//! real `ssh` keeps `~/.ssh/config` aliases, agents, ProxyJump, and
//! known_hosts working for free.
//!
//! Connections run with `BatchMode=yes`: there is no password prompt, so a
//! host must accept key-based login and already be in known_hosts. Set
//! `$WTM_SSH` to use a different ssh program.
//!
//! Hosts are persisted in `hosts.json` next to the GUI's `repos.json`, so the
//! CLI and the app share one list.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::commands::prune::{self, PruneCandidate};
use crate::config;
use crate::error::{Error, Result};
use crate::model::{WorktreeInfo, WorktreeStatus};
use crate::registry;

const HOSTS_FILENAME: &str = "hosts.json";
const SCHEMA_VERSION: u32 = 1;

/// A machine whose repositories wtm can list and clean up over ssh.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    /// Unique label, used by `wtm host <name>` and shown in the app.
    pub name: String,
    /// What `ssh` connects to: a `~/.ssh/config` alias, `user@host`, or
    /// `ssh://user@host:port`.
    pub destination: String,
    /// Directories searched for repositories. Empty means the remote home
    /// directory. A leading `~/` is the remote home.
    #[serde(default)]
    pub roots: Vec<String>,
}

impl Host {
    /// Validate user input into a host.
    pub fn new(name: &str, destination: &str, roots: Vec<String>) -> Result<Host> {
        let name = name.trim();
        let destination = destination.trim();
        if name.is_empty() || name.contains(char::is_whitespace) {
            return Err(Error::Other(
                "host name must be non-empty and contain no spaces".to_string(),
            ));
        }
        if destination.is_empty()
            || destination.starts_with('-')
            || destination.contains(char::is_whitespace)
        {
            return Err(Error::Other(format!(
                "invalid ssh destination '{destination}' (use an ssh alias, user@host, or ssh://user@host:port)"
            )));
        }
        let roots: Vec<String> = roots
            .iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .collect();
        if let Some(root) = roots
            .iter()
            .find(|r| r.contains('\n') || r.starts_with('-'))
        {
            return Err(Error::Other(format!(
                "invalid root '{root}' (a root cannot start with '-' or contain a newline)"
            )));
        }
        Ok(Host {
            name: name.to_string(),
            destination: destination.to_string(),
            roots,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct HostsFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    hosts: Vec<Host>,
}

fn hosts_path() -> Option<PathBuf> {
    Some(config::global_config_dir()?.join(HOSTS_FILENAME))
}

/// Saved hosts in the order they were added. A missing, corrupt, or
/// newer-schema file reads as empty.
pub fn load_hosts() -> Vec<Host> {
    load_hosts_for_update().unwrap_or_default()
}

/// Saved hosts, or an error when the file exists but cannot be trusted, so a
/// write never replaces a hand-edited or newer file with a partial list.
fn load_hosts_for_update() -> Result<Vec<Host>> {
    let Some(path) = hosts_path() else {
        return Ok(Vec::new());
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    match serde_json::from_str::<HostsFile>(&raw) {
        Ok(file) if file.version <= SCHEMA_VERSION => Ok(file.hosts),
        Ok(file) => Err(Error::Config(format!(
            "{} was written by a newer wtm (schema {})",
            path.display(),
            file.version
        ))),
        Err(e) => Err(Error::Config(format!("{}: {e}", path.display()))),
    }
}

fn save_hosts(hosts: Vec<Host>) -> Result<()> {
    let Some(path) = hosts_path() else {
        return Err(Error::Other(
            "cannot resolve the wtm config directory".to_string(),
        ));
    };
    registry::write_json_atomic(
        &path,
        &HostsFile {
            version: SCHEMA_VERSION,
            hosts,
        },
    )
}

/// Add `host`, or replace the saved host with the same name.
pub fn upsert_host(host: Host) -> Result<()> {
    let mut hosts = load_hosts_for_update()?;
    match hosts.iter_mut().find(|h| h.name == host.name) {
        Some(existing) => *existing = host,
        None => hosts.push(host),
    }
    save_hosts(hosts)
}

/// Forget a saved host. Nothing on the host is touched.
pub fn forget_host(name: &str) -> Result<bool> {
    let mut hosts = load_hosts_for_update()?;
    let before = hosts.len();
    hosts.retain(|h| h.name != name);
    if hosts.len() == before {
        return Ok(false);
    }
    save_hosts(hosts)?;
    Ok(true)
}

/// The saved host called `name`.
pub fn find_host(name: &str) -> Result<Host> {
    load_hosts()
        .into_iter()
        .find(|h| h.name == name)
        .ok_or_else(|| {
            Error::Other(format!(
                "no host named '{name}' (add one with `wtm host add {name} <destination>`)"
            ))
        })
}

/// One non-bare repository found on a host.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteRepo {
    /// Directory name of the main working tree.
    pub name: String,
    /// Main working tree path on the host.
    pub path: PathBuf,
    /// Main worktree first, then linked worktrees in git's order.
    pub worktrees: Vec<RemoteWorktree>,
    /// `prune.protected_branches` from the local global config merged with
    /// this repository's `.worktree.toml` and `.worktree.local.toml` on the
    /// host, as `wtm prune` would resolve it there.
    #[serde(skip)]
    pub protected_branches: Vec<String>,
}

impl RemoteRepo {
    /// Disk usage of every worktree whose size is known.
    pub fn size_bytes(&self) -> u64 {
        self.worktrees.iter().filter_map(|w| w.size_bytes).sum()
    }
}

/// A worktree on a host, with its disk usage.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteWorktree {
    #[serde(flatten)]
    pub info: WorktreeInfo,
    /// `du` of the worktree directory. For the main worktree this includes
    /// `.git` but excludes linked worktrees nested inside it. `None` when the
    /// directory is missing or sizes were skipped.
    pub size_bytes: Option<u64>,
}

/// List every repository under the host's roots with worktree status and,
/// when `with_sizes`, disk usage. One ssh round trip.
/// Fails when a repository's config does not parse, as `wtm prune` would
/// there, rather than guess which branches it protects.
pub fn scan(host: &Host, with_sizes: bool) -> Result<Vec<RemoteRepo>> {
    let output = run_script(host, &scan_script(&host.roots, with_sizes))?;
    parse_scan(&output)
        .into_iter()
        .map(|(mut repo, files)| {
            repo.protected_branches = config::load_with_repo_files(
                &repo.path,
                files.repo.as_deref(),
                files.local.as_deref(),
            )?
            .prune
            .protected_branches;
            Ok(repo)
        })
        .collect()
}

/// Biggest first: repositories by total size, and within each, the main
/// worktree then linked worktrees by size (unknown sizes last). Ties sort by
/// path so the order is stable across rescans.
pub fn sort_by_size(repos: &mut [RemoteRepo]) {
    for repo in repos.iter_mut() {
        repo.worktrees.sort_by(|a, b| {
            b.info
                .is_main
                .cmp(&a.info.is_main)
                .then_with(|| b.size_bytes.cmp(&a.size_bytes))
                .then_with(|| a.info.path.cmp(&b.info.path))
        });
    }
    repos.sort_by(|a, b| {
        b.size_bytes()
            .cmp(&a.size_bytes())
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Prune candidates for a scanned repository, with the same rules as
/// `wtm prune` (the main worktree and protected branches are never
/// selected).
pub fn prune_candidates(
    repo: &RemoteRepo,
    merged: bool,
    gone: bool,
    detached: bool,
) -> Vec<PruneCandidate> {
    let infos: Vec<WorktreeInfo> = repo.worktrees.iter().map(|w| w.info.clone()).collect();
    prune::candidates(
        &infos,
        &repo.protected_branches,
        merged,
        gone,
        detached,
        false,
    )
}

/// What happened to one removal target.
#[derive(Debug, Clone, Serialize)]
pub struct RemoveOutcome {
    pub path: PathBuf,
    pub removed: bool,
    /// git's error when not removed, or a note (such as a kept branch) when
    /// removed.
    pub message: Option<String>,
}

/// Remove worktrees of the repository at `repo_path` on the host, deleting
/// each candidate's branch when `delete_branch` is set and the branch still
/// points at the scanned `head`, then run `git worktree prune`. Removal goes through `git worktree remove`, so git
/// refuses dirty worktrees unless `force`, and never deletes a directory it
/// does not manage. One ssh round trip; per-target failures are reported in
/// the outcomes, not as an `Err`.
pub fn remove_worktrees(
    host: &Host,
    repo_path: &Path,
    targets: &[PruneCandidate],
    force: bool,
) -> Result<Vec<RemoveOutcome>> {
    let (main, linked): (Vec<&PruneCandidate>, Vec<&PruneCandidate>) =
        targets.iter().partition(|t| t.info.is_main);
    let mut outcomes = if linked.is_empty() {
        Vec::new()
    } else {
        let output = run_script(host, &remove_script(repo_path, &linked, force))?;
        let reported = parse_remove(&output);
        linked
            .iter()
            .map(|t| {
                reported
                    .iter()
                    .find(|o| o.path == t.info.path)
                    .cloned()
                    .unwrap_or_else(|| RemoveOutcome {
                        path: t.info.path.clone(),
                        removed: false,
                        message: Some("the host did not report a result".to_string()),
                    })
            })
            .collect()
    };
    outcomes.extend(main.iter().map(|t| {
        RemoveOutcome {
            path: t.info.path.clone(),
            removed: false,
            message: Some(
                Error::MainWorktree {
                    action: "remove".to_string(),
                }
                .to_string(),
            ),
        }
    }));
    Ok(outcomes)
}

/// Round `bytes` to a short human size such as `812 KB` or `4.2 GB`.
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Options that override the user's ssh config for these one-shot scripts:
/// - `RemoteCommand=none`: a configured `RemoteCommand` makes ssh refuse the
///   `sh -s` command.
/// - `ClearAllForwardings=yes`: a configured `RemoteForward` fails when
///   another session already holds the port (ssh exits under
///   `ExitOnForwardFailure`), and would otherwise tie the port to wtm's
///   shared connection for `ControlPersist`.
/// - `-a -x`: the scripts need neither the agent nor X11 on the host.
const SSH_OPTIONS: &[&str] = &[
    "-T",
    "-a",
    "-x",
    "-o",
    "RemoteCommand=none",
    "-o",
    "ClearAllForwardings=yes",
    "-o",
    "BatchMode=yes",
    "-o",
    "ConnectTimeout=10",
    "-o",
    "ServerAliveInterval=15",
    "-o",
    "ServerAliveCountMax=3",
];

/// Reuse one connection for the app's scan, remove, rescan sequence. The
/// socket lives in `~/.ssh` (private to the user) and only when it exists.
/// ssh first binds `<ControlPath>.<16 random chars>`: the home directory plus
/// `MUX_PATH_OVERHEAD` bytes must fit a Unix socket path (104 on macOS).
const MUX_PATH_OVERHEAD: usize = "/.ssh/wtm-".len() + 40 + 17;
const SOCKET_PATH_MAX: usize = 104;
const SSH_MUX_OPTIONS: &[&str] = &[
    "-o",
    "ControlMaster=auto",
    "-o",
    "ControlPath=~/.ssh/wtm-%C",
    "-o",
    "ControlPersist=60",
];

fn ssh_program() -> OsString {
    std::env::var_os("WTM_SSH")
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| OsString::from("ssh"))
}

fn run_script(host: &Host, script: &str) -> Result<String> {
    let program = ssh_program();
    let mut cmd = Command::new(&program);
    cmd.args(SSH_OPTIONS);
    if directories::BaseDirs::new().is_some_and(|d| {
        d.home_dir().as_os_str().len() + MUX_PATH_OVERHEAD <= SOCKET_PATH_MAX
            && d.home_dir().join(".ssh").is_dir()
    }) {
        cmd.args(SSH_MUX_OPTIONS);
    }
    cmd.arg("--")
        .arg(&host.destination)
        .arg("sh -s")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| Error::Remote {
        host: host.name.clone(),
        message: format!("could not run {}: {e}", program.to_string_lossy()),
    })?;

    let mut stdin = child.stdin.take().expect("stdin is piped");
    let script = script.to_owned();
    let writer = std::thread::spawn(move || stdin.write_all(script.as_bytes()));
    let output = child.wait_with_output()?;
    let _ = writer.join();

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let message = if output.status.code() == Some(255) && needs_login_hint(&stderr) {
        format!(
            "{} (wtm connects with BatchMode=yes: run `ssh {}` in a terminal once to accept the host key and check that key-based login works)",
            if stderr.is_empty() { "ssh failed" } else { &stderr },
            host.destination
        )
    } else if stderr.is_empty() {
        format!("remote script failed with {}", output.status)
    } else {
        stderr
    };
    Err(Error::Remote {
        host: host.name.clone(),
        message,
    })
}

/// ssh exits 255 for every failure of its own, but only a login or host-key
/// failure is fixed by running `ssh` interactively once.
fn needs_login_hint(stderr: &str) -> bool {
    const LOGIN_FAILURES: &[&str] = &[
        "permission denied",
        "host key verification failed",
        "no more authentication methods",
        "too many authentication failures",
        "remote host identification has changed",
        "authentication failed",
    ];
    let stderr = stderr.to_lowercase();
    stderr.is_empty() || LOGIN_FAILURES.iter().any(|f| stderr.contains(f))
}

/// Single-quote `s` for POSIX `sh`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `~` and `~/…` are the remote home; anything else is quoted verbatim.
fn sh_root(root: &str) -> String {
    if root == "~" {
        "\"$HOME\"".to_string()
    } else if let Some(rest) = root.strip_prefix("~/") {
        format!("\"$HOME\"/{}", sh_quote(rest))
    } else {
        sh_quote(root)
    }
}

/// Starts with a newline so login-script output that lacks one cannot glue
/// itself to the first record.
const SCRIPT_PRELUDE: &str = r#"echo
LC_ALL=C
GIT_OPTIONAL_LOCKS=0
export LC_ALL GIT_OPTIONAL_LOCKS
command -v git >/dev/null 2>&1 || { echo "git is not installed on this host" >&2; exit 1; }
tab=$(printf '\t')
oneline() { printf '%s' "$1" | tr '\t\n' '  '; }
"#;

/// Record per line: `R<TAB>path` starts a repository. `F<TAB>name` starts
/// one of its config files, whose lines follow as `T<TAB>line`. Each
/// `W<TAB>…` line is one of its worktrees, fields in [`parse_worktree`]
/// order, path last so it may contain tabs.
const SCAN_BODY: &str = r#"reset() { wt=; head=; branch=; locked=0; lockr=; prunable=0; }
emit() {
  [ -n "$wt" ] || return 0
  missing=0; [ -d "$wt" ] || missing=1
  short=; ctime=
  if [ -n "$head" ]; then
    info=$(git -C "$repo" log -1 --format='%h %ct' "$head" 2>/dev/null)
    short=${info% *}; ctime=${info#* }
  fi
  dirty=; ahead=; behind=; gone=0; merged=0; size=
  if [ "$missing" = 0 ]; then
    if st=$(git -C "$wt" status --porcelain --ignore-submodules 2>/dev/null); then
      dirty=$(printf '%s\n' "$st" | grep -c .)
    fi
    if [ -n "$branch" ]; then
      track=$(git -C "$repo" for-each-ref --format='%(upstream)%09%(upstream:track,nobracket)' "refs/heads/$branch" 2>/dev/null)
      up=${track%%"$tab"*}; state=${track#*"$tab"}
      if [ "$state" = gone ]; then
        gone=1
      elif [ -n "$up" ]; then
        counts=$(git -C "$repo" rev-list --left-right --count "refs/heads/$branch...$up" 2>/dev/null)
        ahead=${counts%%"$tab"*}; behind=${counts##*"$tab"}
      fi
    fi
    if [ "$main" = 0 ] && [ -n "$base" ] && [ -n "$head" ] && [ "$branch" != "$basename" ]; then
      git -C "$repo" merge-base --is-ancestor "$head" "$base" 2>/dev/null && merged=1
    fi
    if [ "$sizes" = 1 ]; then size=$(du -sk "$wt" 2>/dev/null | cut -f1); fi
  fi
  printf 'W\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
    "$main" "$missing" "$locked" "$prunable" "$branch" "$short" "$ctime" \
    "$dirty" "$ahead" "$behind" "$gone" "$merged" "$size" "$(oneline "$lockr")" "$wt"
  main=0
}
for root in "$@"; do
  [ -d "$root" ] || continue
  find -H "$root" -maxdepth 5 \( -name node_modules -o -name .cache -o -name .npm \
    -o -name .cargo -o -name .rustup -o -name .local -o -name .venv -o -name .pub-cache \) -prune \
    -o -name .git -type d -print -prune 2>/dev/null
done | sort -u | while IFS= read -r gitdir; do
  repo=${gitdir%/.git}
  [ "$(git -C "$repo" rev-parse --is-inside-work-tree 2>/dev/null)" = true ] || continue
  [ "$(git -C "$repo" rev-parse --git-dir 2>/dev/null)" = .git ] || continue
  base=; basename=
  for spec in origin/HEAD origin/main origin/master; do
    if b=$(git -C "$repo" rev-parse -q --verify "$spec^{commit}" 2>/dev/null); then
      base=$b; basename=$spec; break
    fi
  done
  if [ -z "$base" ]; then
    base=$(git -C "$repo" rev-parse -q --verify 'HEAD^{commit}' 2>/dev/null)
    basename=$(git -C "$repo" symbolic-ref -q --short HEAD 2>/dev/null)
  fi
  printf 'R\t%s\n' "$repo"
  for f in .worktree.toml .worktree.local.toml; do
    [ -f "$repo/$f" ] || continue
    printf 'F\t%s\n' "$f"
    while IFS= read -r line || [ -n "$line" ]; do printf 'T\t%s\n' "$line"; done < "$repo/$f"
  done
  { git -C "$repo" worktree list --porcelain 2>/dev/null; echo; } | {
    main=1; reset
    while IFS= read -r line; do
      case $line in
        'worktree '*) wt=${line#worktree } ;;
        'HEAD '*) head=${line#HEAD } ;;
        'branch refs/heads/'*) branch=${line#branch refs/heads/} ;;
        locked) locked=1 ;;
        'locked '*) locked=1; lockr=${line#locked } ;;
        prunable|'prunable '*) prunable=1 ;;
        '') emit; reset ;;
      esac
    done
  }
done
"#;

fn scan_script(roots: &[String], with_sizes: bool) -> String {
    let args: Vec<String> = if roots.is_empty() {
        vec![sh_root("~")]
    } else {
        roots.iter().map(|r| sh_root(r)).collect()
    };
    format!(
        "{SCRIPT_PRELUDE}sizes={}\nset -- {}\n{SCAN_BODY}",
        u8::from(with_sizes),
        args.join(" ")
    )
}

/// A repository's config files as read on the host.
#[derive(Debug, Default)]
struct RepoConfigText {
    repo: Option<String>,
    local: Option<String>,
}

/// Parse [`SCAN_BODY`] output. Lines that are not records (for example
/// banner text from a login script) are ignored.
fn parse_scan(output: &str) -> Vec<(RemoteRepo, RepoConfigText)> {
    let mut repos: Vec<(RemoteRepo, RepoConfigText)> = Vec::new();
    let mut open_file: Option<&str> = None;
    for line in output.lines() {
        if let Some(path) = line.strip_prefix("R\t") {
            let path = PathBuf::from(path);
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            repos.push((
                RemoteRepo {
                    name,
                    path,
                    worktrees: Vec::new(),
                    protected_branches: Vec::new(),
                },
                RepoConfigText::default(),
            ));
            open_file = None;
        } else if let Some(name) = line.strip_prefix("F\t") {
            open_file = Some(name);
        } else if let Some(text) = line.strip_prefix("T\t") {
            let Some((_, files)) = repos.last_mut() else {
                continue;
            };
            let target = match open_file {
                Some(".worktree.toml") => &mut files.repo,
                Some(".worktree.local.toml") => &mut files.local,
                _ => continue,
            };
            let contents = target.get_or_insert_with(String::new);
            contents.push_str(text);
            contents.push('\n');
        } else if let Some(record) = line.strip_prefix("W\t") {
            open_file = None;
            if let (Some((repo, _)), Some(worktree)) = (repos.last_mut(), parse_worktree(record)) {
                repo.worktrees.push(worktree);
            }
        }
    }
    for (repo, _) in &mut repos {
        // git reports symlink-resolved paths; use them so the repository
        // and its worktrees agree (macOS `/tmp` is `/private/tmp`).
        if let Some(main) = repo.worktrees.iter().find(|w| w.info.is_main) {
            repo.path = main.info.path.clone();
        }
        subtract_nested_sizes(repo);
    }
    repos
}

fn parse_worktree(record: &str) -> Option<RemoteWorktree> {
    let f: Vec<&str> = record.splitn(15, '\t').collect();
    let [main, missing, locked, prunable, branch, short, time, dirty, ahead, behind, gone, merged, size_kb, lock_reason, path] =
        f[..]
    else {
        return None;
    };
    let flag = |s: &str| s == "1";
    let text = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let path = PathBuf::from(path);
    let is_main = flag(main);
    let name = if is_main {
        "main".to_string()
    } else {
        path.file_name()?.to_string_lossy().into_owned()
    };
    let status = dirty
        .parse::<usize>()
        .ok()
        .map(|dirty_count| WorktreeStatus {
            dirty: dirty_count > 0,
            dirty_count,
            ahead: ahead.parse().ok(),
            behind: behind.parse().ok(),
            upstream_gone: flag(gone),
            merged: flag(merged),
        });
    Some(RemoteWorktree {
        info: WorktreeInfo {
            name,
            path,
            branch: text(branch),
            head: text(short),
            is_main,
            is_missing: flag(missing),
            is_locked: flag(locked),
            lock_reason: flag(locked).then(|| lock_reason.to_string()),
            head_time: time.parse().ok(),
            is_prunable: flag(prunable),
            status,
        },
        size_bytes: size_kb.parse::<u64>().ok().map(|kb| kb * 1024),
    })
}

/// Linked worktrees inside the main working tree (such as
/// `.claude/worktrees/*`) are counted by `du` twice; keep them on their own
/// rows only.
fn subtract_nested_sizes(repo: &mut RemoteRepo) {
    let Some(main_index) = repo.worktrees.iter().position(|w| w.info.is_main) else {
        return;
    };
    let main_path = repo.worktrees[main_index].info.path.clone();
    let nested: u64 = repo
        .worktrees
        .iter()
        .filter(|w| !w.info.is_main && w.info.path.starts_with(&main_path))
        .filter_map(|w| w.size_bytes)
        .sum();
    if let Some(size) = repo.worktrees[main_index].size_bytes.as_mut() {
        *size = size.saturating_sub(nested);
    }
}

/// Output per target: `OK<TAB>note<TAB>path` or `ERR<TAB>message<TAB>path`.
/// A worktree the scan saw as missing is only forced when it is still gone,
/// and a branch is only deleted while it still points at the scanned commit.
const REMOVE_BODY: &str = r#"repo=$1; flag=$2; shift 2
cd / || exit 1
while [ $# -ge 4 ]; do
  path=$1; branch=$2; missing=$3; head=$4; shift 4
  f=$flag
  if [ "$missing" = 1 ] && [ ! -e "$path" ]; then f=--force; fi
  if out=$(git -C "$repo" worktree remove $f -- "$path" 2>&1); then
    note=
    if [ -n "$branch" ]; then
      tip=$(git -C "$repo" rev-parse -q --verify "refs/heads/$branch" 2>/dev/null)
      case $tip in
        "$head"?*) same=$head ;;
        *) same= ;;
      esac
      if [ -z "$same" ]; then
        note="worktree removed, but branch $branch was kept: it changed since the scan"
      elif ! out=$(git -C "$repo" branch -D -- "$branch" 2>&1); then
        note="worktree removed, but branch $branch was kept: $out"
      fi
    fi
    printf 'OK\t%s\t%s\n' "$(oneline "$note")" "$path"
  else
    printf 'ERR\t%s\t%s\n' "$(oneline "$out")" "$path"
  fi
done
git -C "$repo" worktree prune >/dev/null 2>&1
exit 0
"#;

fn remove_script(repo_path: &Path, targets: &[&PruneCandidate], force: bool) -> String {
    let mut args = vec![
        sh_quote(&repo_path.to_string_lossy()),
        sh_quote(if force { "--force" } else { "" }),
    ];
    for t in targets {
        let branch = if t.delete_branch {
            t.info.branch.as_deref().unwrap_or("")
        } else {
            ""
        };
        args.push(sh_quote(&t.info.path.to_string_lossy()));
        args.push(sh_quote(branch));
        args.push(sh_quote(if t.info.is_missing { "1" } else { "0" }));
        args.push(sh_quote(t.info.head.as_deref().unwrap_or("")));
    }
    format!("{SCRIPT_PRELUDE}set -- {}\n{REMOVE_BODY}", args.join(" "))
}

fn parse_remove(output: &str) -> Vec<RemoveOutcome> {
    output
        .lines()
        .filter_map(|line| {
            let mut f = line.splitn(3, '\t');
            let removed = match f.next()? {
                "OK" => true,
                "ERR" => false,
                _ => return None,
            };
            let message = f.next()?.trim();
            Some(RemoveOutcome {
                path: PathBuf::from(f.next()?),
                removed,
                message: (!message.is_empty()).then(|| message.to_string()),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testgit::{commit_file, git, init_repo};
    use std::fs;

    /// Run a generated script with the local `sh`, as `ssh host sh -s` would
    /// on the host.
    fn run_local(script: &str) -> String {
        let mut child = Command::new("sh")
            .arg("-s")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(script.as_bytes())
            .unwrap();
        let out = child.wait_with_output().unwrap();
        assert!(
            out.status.success(),
            "script failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    fn scan_local(root: &Path) -> Vec<RemoteRepo> {
        parse_scan(&run_local(&scan_script(
            &[root.to_string_lossy().into_owned()],
            true,
        )))
        .into_iter()
        .map(|(mut repo, _)| {
            repo.protected_branches = vec!["main".to_string()];
            repo
        })
        .collect()
    }

    fn worktree<'a>(repo: &'a RemoteRepo, name: &str) -> &'a RemoteWorktree {
        repo.worktrees
            .iter()
            .find(|w| w.info.display_name() == name)
            .unwrap_or_else(|| panic!("no worktree {name} in {repo:#?}"))
    }

    fn candidate(w: &RemoteWorktree, delete_branch: bool) -> PruneCandidate {
        PruneCandidate {
            info: w.info.clone(),
            reasons: Vec::new(),
            delete_branch,
        }
    }

    #[test]
    fn scan_reports_repos_worktrees_status_and_sizes() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let main = base.join("code").join("app");
        init_repo(&main);
        git(&main, &["worktree", "add", "-b", "done", "../app-done"]);
        git(&main, &["worktree", "add", "-b", "wip", "../app-wip"]);
        commit_file(&base.join("code/app-wip"), "wip.txt");
        fs::write(base.join("code/app-wip/scratch"), "x").unwrap();
        git(
            &main,
            &["worktree", "add", "--detach", ".claude/worktrees/agent"],
        );
        fs::write(
            base.join("code/app/.claude/worktrees/agent/big"),
            vec![7u8; 400_000],
        )
        .unwrap();
        git(&main, &["worktree", "add", "-b", "gone-dir", "../app-gone"]);
        fs::remove_dir_all(base.join("code/app-gone")).unwrap();
        git(
            &main,
            &["worktree", "lock", "--reason", "keep me", "../app-done"],
        );
        fs::create_dir_all(base.join("node_modules/pkg")).unwrap();
        init_repo(&base.join("node_modules/pkg"));

        let repos = scan_local(&base);

        assert_eq!(repos.len(), 1, "node_modules is skipped: {repos:#?}");
        let repo = &repos[0];
        assert_eq!(repo.name, "app");
        assert_eq!(repo.path, main);
        assert_eq!(repo.path, repo.worktrees[0].info.path);
        assert_eq!(repo.worktrees.len(), 5);

        let main_wt = &repo.worktrees[0];
        assert!(main_wt.info.is_main);
        assert_eq!(main_wt.info.name, "main");
        assert_eq!(main_wt.info.branch.as_deref(), Some("main"));
        assert!(main_wt.info.head.is_some() && main_wt.info.head_time.is_some());

        let done = worktree(repo, "done");
        let done_status = done.info.status.as_ref().unwrap();
        assert!(done_status.merged && !done_status.dirty);
        assert!(done.info.is_locked);
        assert_eq!(done.info.lock_reason.as_deref(), Some("keep me"));
        assert_eq!(done_status.ahead, None);

        let wip = worktree(repo, "wip").info.status.as_ref().unwrap();
        assert!(!wip.merged);
        assert_eq!(wip.dirty_count, 1);

        let agent = worktree(repo, "agent");
        assert_eq!(agent.info.branch, None);
        assert!(agent.info.status.as_ref().unwrap().merged);
        let agent_size = agent.size_bytes.unwrap();
        assert!(agent_size >= 400_000, "{agent_size}");
        assert!(
            main_wt.size_bytes.unwrap() < agent_size,
            "nested worktree is not counted twice: {:?} vs {agent_size}",
            main_wt.size_bytes
        );

        let gone = worktree(repo, "gone-dir");
        assert!(gone.info.is_missing && gone.info.is_prunable);
        assert!(gone.info.status.is_none() && gone.size_bytes.is_none());

        let mut names: Vec<String> = prune_candidates(repo, true, false, false)
            .iter()
            .map(|c| c.info.display_name().to_string())
            .collect();
        names.sort();
        assert_eq!(names, ["agent", "done", "gone-dir"]);
    }

    #[test]
    fn scan_reports_upstream_ahead_behind_and_gone() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let upstream = base.join("upstream");
        init_repo(&upstream);
        git(&upstream, &["branch", "tracked"]);
        git(&upstream, &["branch", "doomed"]);
        let clone = base.join("work").join("clone");
        git(
            &base,
            &[
                "clone",
                "-q",
                upstream.to_str().unwrap(),
                clone.to_str().unwrap(),
            ],
        );
        git(&clone, &["worktree", "add", "../tracked", "tracked"]);
        commit_file(&clone.parent().unwrap().join("tracked"), "ahead.txt");
        git(&clone, &["worktree", "add", "../doomed", "doomed"]);
        git(&upstream, &["branch", "-D", "doomed"]);
        git(&clone, &["fetch", "-q", "--prune"]);

        let repos = scan_local(&base.join("work"));
        let repo = &repos[0];
        let tracked = worktree(repo, "tracked").info.status.clone().unwrap();
        assert_eq!((tracked.ahead, tracked.behind), (Some(1), Some(0)));
        assert!(!tracked.upstream_gone);
        let doomed = worktree(repo, "doomed").info.status.clone().unwrap();
        assert!(doomed.upstream_gone);
        assert_eq!(doomed.ahead, None);
    }

    #[test]
    fn remove_script_removes_refuses_dirty_and_clears_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let main = base.join("repo");
        init_repo(&main);
        git(&main, &["worktree", "add", "-b", "clean", "../clean"]);
        git(&main, &["worktree", "add", "-b", "dirty", "../dir ty"]);
        fs::write(base.join("dir ty/new"), "x").unwrap();
        git(&main, &["worktree", "add", "-b", "lost", "../lost"]);
        fs::remove_dir_all(base.join("lost")).unwrap();

        let repo = &scan_local(&base)[0];
        let targets = [
            candidate(worktree(repo, "clean"), true),
            candidate(worktree(repo, "dirty"), false),
            candidate(worktree(repo, "lost"), false),
            candidate(&repo.worktrees[0], false),
        ];
        let refs: Vec<&PruneCandidate> = targets.iter().filter(|t| !t.info.is_main).collect();
        let outcomes = parse_remove(&run_local(&remove_script(&main, &refs, false)));

        assert_eq!(outcomes.len(), 3);
        assert!(outcomes[0].removed && outcomes[0].message.is_none());
        assert!(!base.join("clean").exists());
        assert!(git(&main, &["branch", "--list", "clean"]).is_empty());
        assert!(!outcomes[1].removed);
        assert_eq!(outcomes[1].path, base.join("dir ty"));
        assert!(
            outcomes[1].message.as_deref().unwrap().contains("--force"),
            "{:?}",
            outcomes[1].message
        );
        assert!(base.join("dir ty").exists());
        assert!(outcomes[2].removed);

        let repo = &scan_local(&base)[0];
        let names: Vec<&str> = repo
            .worktrees
            .iter()
            .map(|w| w.info.display_name())
            .collect();
        assert_eq!(names, ["main", "dirty"]);

        let forced = [candidate(worktree(repo, "dirty"), true)];
        let outcomes = parse_remove(&run_local(&remove_script(&main, &[&forced[0]], true)));
        assert!(outcomes[0].removed);
        assert!(!base.join("dir ty").exists());
    }

    #[test]
    fn remove_script_rechecks_missing_and_keeps_a_moved_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let main = base.join("repo");
        init_repo(&main);
        git(&main, &["worktree", "add", "-b", "back", "../back"]);
        git(&main, &["worktree", "add", "-b", "moved", "../moved"]);
        let parked = base.join("parked");
        fs::rename(base.join("back"), &parked).unwrap();
        let repo = &scan_local(&base)[0];
        let stale_missing = candidate(worktree(repo, "back"), false);
        let stale_tip = candidate(worktree(repo, "moved"), true);
        assert!(stale_missing.info.is_missing);

        fs::rename(&parked, base.join("back")).unwrap();
        fs::write(base.join("back/work.txt"), "unsaved").unwrap();
        commit_file(&base.join("moved"), "later.txt");

        let outcomes = parse_remove(&run_local(&remove_script(
            &main,
            &[&stale_missing, &stale_tip],
            false,
        )));
        assert!(!outcomes[0].removed, "{:?}", outcomes[0]);
        assert!(base.join("back/work.txt").exists());
        assert!(outcomes[1].removed);
        assert!(
            outcomes[1]
                .message
                .as_deref()
                .unwrap()
                .contains("changed since the scan"),
            "{:?}",
            outcomes[1]
        );
        assert_eq!(git(&main, &["branch", "--list", "moved"]), "moved");
    }

    #[test]
    fn scan_follows_a_symlinked_root_and_skips_broken_nested_git_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let main = base.join("real").join("app");
        init_repo(&main);
        fs::create_dir_all(main.join("vendor/broken/.git")).unwrap();
        std::os::unix::fs::symlink(base.join("real"), base.join("link")).unwrap();

        let repos = scan_local(&base.join("link"));

        let paths: Vec<&Path> = repos.iter().map(|r| r.path.as_path()).collect();
        assert_eq!(paths, [main.as_path()]);
    }

    #[test]
    fn scan_captures_repo_config_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path().canonicalize().unwrap();
        let main = base.join("app");
        init_repo(&main);
        fs::write(
            main.join(".worktree.toml"),
            "[prune]\nprotected_branches = [\"main\", \"keep\"]",
        )
        .unwrap();

        let output = run_local(&scan_script(&[base.to_string_lossy().into_owned()], false));
        let parsed = parse_scan(&format!("banner without newline{output}"));

        assert_eq!(parsed.len(), 1);
        let files = &parsed[0].1;
        assert_eq!(
            files.repo.as_deref(),
            Some("[prune]\nprotected_branches = [\"main\", \"keep\"]\n")
        );
        assert_eq!(files.local, None);
        assert_eq!(parsed[0].0.worktrees.len(), 1);
    }

    #[test]
    fn parse_scan_ignores_noise_and_bad_records() {
        let output = "Welcome!\nR\t/srv/a\nW\t1\t0\t0\t0\tmain\tabc\t10\t0\t\t\t0\t0\t2048\t\t/srv/a\nW\ttoo\tfew\nR\t/srv/b\n";
        let repos: Vec<RemoteRepo> = parse_scan(output).into_iter().map(|(r, _)| r).collect();
        assert_eq!(repos.len(), 2);
        assert_eq!(repos[0].worktrees.len(), 1);
        assert_eq!(repos[0].size_bytes(), 2_097_152);
        assert_eq!(repos[0].worktrees[0].info.head_time, Some(10));
        assert!(repos[1].worktrees.is_empty());
    }

    #[test]
    fn sort_by_size_puts_biggest_first_and_main_on_top() {
        let mut repos: Vec<RemoteRepo> = parse_scan(concat!(
            "R\t/srv/small\n",
            "W\t1\t0\t0\t0\tmain\t\t\t0\t\t\t0\t0\t1\t\t/srv/small\n",
            "R\t/srv/big\n",
            "W\t1\t0\t0\t0\tmain\t\t\t0\t\t\t0\t0\t1\t\t/srv/big\n",
            "W\t0\t1\t0\t1\tgone\t\t\t\t\t\t0\t0\t\t\t/srv/big-gone\n",
            "W\t0\t0\t0\t0\tb\t\t\t0\t\t\t0\t0\t9\t\t/srv/big-b\n",
            "W\t0\t0\t0\t0\ta\t\t\t0\t\t\t0\t0\t9\t\t/srv/big-a\n",
        ))
        .into_iter()
        .map(|(r, _)| r)
        .collect();
        sort_by_size(&mut repos);
        let order: Vec<Vec<&str>> = repos
            .iter()
            .map(|r| r.worktrees.iter().map(|w| w.info.display_name()).collect())
            .collect();
        assert_eq!(order, [vec!["main", "a", "b", "gone"], vec!["main"]]);
    }

    #[test]
    fn host_new_validates_input() {
        let host = Host::new(
            " vps ",
            "ubuntu@example.com",
            vec![" ~/code ".into(), "".into()],
        )
        .unwrap();
        assert_eq!(host.name, "vps");
        assert_eq!(host.roots, ["~/code"]);
        assert!(Host::new("", "vps", vec![]).is_err());
        assert!(Host::new("my vps", "vps", vec![]).is_err());
        assert!(Host::new("vps", "-oProxyCommand=x", vec![]).is_err());
        assert!(Host::new("vps", "a b", vec![]).is_err());
        assert!(Host::new("vps", "vps", vec!["-delete".into()]).is_err());
    }

    #[test]
    fn roots_are_quoted_and_tilde_is_the_remote_home() {
        assert_eq!(sh_root("~"), "\"$HOME\"");
        assert_eq!(sh_root("~/it's"), "\"$HOME\"/'it'\\''s'");
        assert_eq!(sh_root("/srv/$x"), "'/srv/$x'");
    }

    #[test]
    fn login_hint_only_for_login_and_host_key_failures() {
        assert!(needs_login_hint(
            "ubuntu@vps: Permission denied (publickey)."
        ));
        assert!(needs_login_hint("Host key verification failed."));
        assert!(needs_login_hint(""));
        assert!(!needs_login_hint(
            "Error: remote port forwarding failed for listen port 8317"
        ));
        assert!(!needs_login_hint(
            "ssh: connect to host vps port 22: Connection refused"
        ));
    }

    #[test]
    fn format_size_is_short() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(300 * 1024 * 1024), "300 MB");
        assert_eq!(format_size(5 * 1024 * 1024 * 1024), "5.0 GB");
    }
}
