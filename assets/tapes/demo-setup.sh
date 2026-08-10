#!/usr/bin/env bash
# Build a realistic demo repo with a spread of worktree statuses for VHS recordings.
# The repo lives under $ROOT, and $ROOT is used as $HOME at record time so paths
# render as clean ~/acme/... in wtm's output.
set -euo pipefail

ROOT="${1:?usage: demo-setup.sh <root-dir>}"
rm -rf "$ROOT"
mkdir -p "$ROOT"
cd "$ROOT"

export GIT_AUTHOR_NAME="Ada Lovelace"  GIT_AUTHOR_EMAIL="ada@example.com"
export GIT_COMMITTER_NAME="Ada Lovelace" GIT_COMMITTER_EMAIL="ada@example.com"

commit() { git commit -q --no-gpg-sign -m "$1"; }

# a self-contained gitconfig so record-time git (HOME=$ROOT) has an identity
cat > "$ROOT/.gitconfig" <<'EOF'
[user]
	name = Ada Lovelace
	email = ada@example.com
[init]
	defaultBranch = main
[commit]
	gpgsign = false
[advice]
	detachedHead = false
EOF

# --- origin (bare) ---
git init -q --bare origin.git

# --- main working repo: "acme" ---
git clone -q origin.git acme
cd acme
git config user.name "Ada Lovelace"; git config user.email "ada@example.com"; git config commit.gpgsign false

mkdir -p src
printf 'export const app = "acme";\n'        > src/app.js
printf '# acme\n\nA small web app.\n'          > README.md
printf 'node_modules\n.env\n.worktree.local.toml\n' > .gitignore
printf 'API_URL=https://api.acme.dev\n'        > .env
git add -A; commit "Initial commit"
printf 'export const version = "1.0.0";\n'    > src/version.js
git add -A; commit "Add version module"
printf 'export function login(){/* ... */}\n' > src/auth.js
git add -A; commit "Scaffold auth module"

# Shared repo config stays data-only. Executable setup commands belong in the
# trusted, git-ignored local layer.
cat > .worktree.toml <<'EOF'
path_template = "../{repo}-worktrees/{branch}"
default_base = "main"

[[setup.copy]]
path = ".env"
mode = "copy"
EOF
git add -A; commit "Add wtm config"

cat > .worktree.local.toml <<'EOF'
[setup]
commands = ["echo '  ▸ installing dependencies' && echo '  ▸ ready'"]
EOF
git branch -M main
git push -q -u origin main

# puppet clone to create "remote-side" commits (behind states)
cd "$ROOT"; git clone -q origin.git puppet
cd puppet; git config user.name "Grace Hopper"; git config user.email "grace@example.com"; git config commit.gpgsign false

cd "$ROOT/acme"
WT="$ROOT/acme-worktrees"

# 1) feature/login — clean, AHEAD 2
git worktree add -q -b feature/login "$WT/feature/login" main
git -C "$WT/feature/login" push -q -u origin feature/login
( cd "$WT/feature/login"
  printf 'export function loginForm(){}\n' > src/login-form.js; git add -A; commit "Add login form"
  printf 'export function validate(){}\n'  > src/validate.js;  git add -A; commit "Validate credentials" )

# 2) feature/search — DIRTY (one commit so it's not "merged", plus uncommitted changes)
git worktree add -q -b feature/search "$WT/feature/search" main
( cd "$WT/feature/search"
  printf 'export function search(q){ return q; }\n' > src/search.js; git add -A; commit "Add search endpoint"
  printf 'export const index = [];\n' >> src/app.js   # modify tracked file (unstaged)
  printf 'TODO: ranking\n'            > NOTES.txt )    # untracked

# 3) feature/payments — clean, no upstream (shows "-")
git worktree add -q -b feature/payments "$WT/feature/payments" main
( cd "$WT/feature/payments"; printf 'export function charge(){}\n' > src/payments.js; git add -A; commit "Start payments" )

# 4) hotfix/crash — MERGED into local main (prune --merged candidate)
git worktree add -q -b hotfix/crash "$WT/hotfix/crash" main
( cd "$WT/hotfix/crash"; printf 'export function guard(){}\n' > src/guard.js; git add -A; commit "Fix crash on empty input" )
git merge -q --no-ff hotfix/crash -m "Merge branch 'hotfix/crash'"   # advance local main
git push -q origin main

# 5) experiment/new-ui — upstream GONE
git worktree add -q -b experiment/new-ui "$WT/experiment/new-ui" main
git -C "$WT/experiment/new-ui" push -q -u origin experiment/new-ui
( cd "$WT/experiment/new-ui"; printf 'export const ui="v2";\n' > src/ui.js; git add -A; commit "Prototype new UI" )
git push -q origin --delete experiment/new-ui

# 6) release/1.2 — DIVERGED: 1 local commit ahead, 2 behind upstream
git worktree add -q -b release/1.2 "$WT/release/1.2" main
git -C "$WT/release/1.2" push -q -u origin release/1.2
( cd "$WT/release/1.2"; printf 'export const channel="stable";\n' > src/channel.js; git add -A; commit "Set release channel" )
cd "$ROOT/puppet"; git fetch -q origin
git checkout -q -B release/1.2 origin/release/1.2
printf 'export const rc = 1;\n' > src/rc.js; git add -A; commit "Cut RC1"
printf 'export const rc = 2;\n' > src/rc.js; git add -A; commit "Cut RC2"
git push -q origin release/1.2

# refresh origin refs in acme so behind/gone are visible to wtm
cd "$ROOT/acme"; git fetch -q --prune origin

echo "demo repo ready at $ROOT/acme"
