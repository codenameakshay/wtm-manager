#!/usr/bin/env bash
# Install wtm (fast Git worktree manager) and, optionally, its shell
# integration. See ../reference.md for the full command reference.
#
# Usage: install.sh [--yes]
#   --yes   Skip the shell-integration confirmation prompt and install it
#           non-interactively (still idempotent: skipped if already present).

set -euo pipefail

ASSUME_YES=0
for arg in "$@"; do
  case "$arg" in
    --yes | -y)
      ASSUME_YES=1
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

# --- Step 1: git is required ------------------------------------------------

if ! command -v git >/dev/null 2>&1; then
  echo "error: git is required but was not found on PATH." >&2
  echo "Install Git first: https://git-scm.com/downloads" >&2
  exit 1
fi

# --- Step 2: skip install if wtm is already present -------------------------

if command -v wtm >/dev/null 2>&1; then
  echo "wtm is already installed: $(wtm --version)"
else
  # --- Step 3: install via the best available method ------------------------

  if command -v cargo >/dev/null 2>&1; then
    echo "Installing wtm from the GitHub repository via cargo..."
    cargo install --git https://github.com/codenameakshay/wtm-manager --locked
  else
    echo "Installing wtm via the prebuilt-binary installer..."
    curl --proto '=https' --tlsv1.2 -LsSf \
      https://github.com/codenameakshay/wtm-manager/releases/latest/download/wtm-installer.sh | sh
  fi

  # --- Step 4: verify ---------------------------------------------------------

  if command -v wtm >/dev/null 2>&1; then
    echo "Installed: $(wtm --version)"
  else
    echo "wtm was installed but is not on PATH yet." >&2
    echo "If installed via cargo, add ~/.cargo/bin to PATH and retry:" >&2
    # shellcheck disable=SC2016 # literal text shown to the user, not expanded here
    echo '  export PATH="$HOME/.cargo/bin:$PATH"' >&2
    exit 1
  fi
fi

# --- Step 5: shell integration (with confirmation) --------------------------

shell_name="$(basename "${SHELL:-}")"
rc_file=""
init_shell=""
case "$shell_name" in
  zsh)
    rc_file="$HOME/.zshrc"
    init_shell="zsh"
    ;;
  bash)
    rc_file="$HOME/.bashrc"
    init_shell="bash"
    ;;
  *)
    echo "Unrecognized \$SHELL ('${SHELL:-unset}'); skipping shell integration."
    echo "To set it up manually, add one of these to your shell rc file:"
    # shellcheck disable=SC2016 # literal text shown to the user, not expanded here
    echo '  eval "$(command wtm init zsh)"   # zsh'
    # shellcheck disable=SC2016 # literal text shown to the user, not expanded here
    echo '  eval "$(command wtm init bash)"  # bash'
    exit 0
    ;;
esac

integration_line="eval \"\$(command wtm init $init_shell)\""

if [ -f "$rc_file" ] && grep -q 'wtm init' "$rc_file"; then
  echo "Shell integration already present in $rc_file; leaving it as-is."
  exit 0
fi

do_install=0
if [ "$ASSUME_YES" -eq 1 ]; then
  do_install=1
elif [ -t 0 ]; then
  printf 'Add wtm shell integration to %s? [y/N] ' "$rc_file"
  read -r reply
  case "$reply" in
    y | Y | yes | YES) do_install=1 ;;
    *) do_install=0 ;;
  esac
else
  echo "Non-interactive shell and --yes not given; skipping shell integration."
  echo "To add it later, append this line to $rc_file:"
  echo "  $integration_line"
  exit 0
fi

if [ "$do_install" -eq 1 ]; then
  {
    echo ""
    echo "# wtm shell integration (cd-on-switch, completions)"
    echo "$integration_line"
  } >>"$rc_file"
  echo "Added shell integration to $rc_file:"
  echo "  $integration_line"
  echo "Open a new shell (or 'source $rc_file') for it to take effect."
else
  echo "Skipped shell integration. Add it later with:"
  echo "  $integration_line"
fi
