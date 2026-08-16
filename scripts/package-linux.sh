#!/usr/bin/env bash
#
# package-linux.sh — assemble a self-contained tarball of wtm-gui for Linux.
#
# This mirrors bundle-mac.sh's job for macOS: turn a plain `cargo build`
# output into something a user can actually install — here, a
# wtm-gui-<version>-<arch>-linux.tar.xz containing the binary, its
# .desktop entry, its hicolor icons, and an install.sh that puts all three
# where the desktop/icon caches expect them. `cargo build -p wtm-gui` alone
# produces a bare binary with no icon, no launcher entry, and no indication
# of which files belong where.
#
# For a .deb instead, see `cargo deb -p wtm-gui` — the metadata behind it
# lives in crates/wtm-gui/Cargo.toml's [package.metadata.deb]; this script
# does not build one.
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: scripts/package-linux.sh [options]

Build wtm-gui and package it into a Linux tarball:
  target/package-linux/wtm-gui-<version>-<arch>-linux.tar.xz

Options:
  --release       Build the release profile (default).
  --debug         Build the debug profile instead. Much faster; use this
                   while iterating on the packaging itself.
  --output <dir>  Directory the tarball is written into. Relative paths are
                   resolved against the repo root. Default: target/package-linux
  --skip-build    Reuse the existing target/<profile>/wtm-gui binary instead
                   of invoking cargo. The binary must already exist and be a
                   Linux ELF build.
  -h, --help      Show this help and exit.

Examples:
  # Inside the wtm-linux-build container, or on a Linux machine directly:
  scripts/package-linux.sh --debug            # fast local loop
  scripts/package-linux.sh                    # release tarball

  # Assembling on a Mac from a binary already cross-built in the container:
  scripts/package-linux.sh --skip-build
EOF
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

profile=release
output_dir=target/package-linux
skip_build=false

while [ $# -gt 0 ]; do
	case "$1" in
	--release)
		profile=release
		shift
		;;
	--debug)
		profile=debug
		shift
		;;
	--output)
		[ $# -ge 2 ] || {
			echo "error: --output requires a directory argument" >&2
			exit 1
		}
		output_dir=$2
		shift 2
		;;
	--skip-build)
		skip_build=true
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		echo "error: unknown argument: $1" >&2
		usage >&2
		exit 1
		;;
	esac
done

# This script produces a Linux ELF binary and a Linux-specific tarball layout
# (FHS-style bin/, share/applications, share/icons/hicolor). It runs on the
# Linux build machine or inside the wtm-linux-build container — not on a
# developer's Mac, which cannot produce that binary at all (gpui's Linux
# backend doesn't cross-compile from here). --skip-build is the one
# legitimate exception: it lets the tarball be assembled on a Mac from a
# binary that was already cross-built elsewhere (e.g. copied out of the
# container). The ELF magic-byte check further down still catches an
# accidentally-native macOS binary either way, so this isn't the only guard.
if [ "$(uname -s)" != "Linux" ] && [ "$skip_build" = false ]; then
	echo "error: package-linux.sh builds on Linux only (got $(uname -s))" >&2
	cat >&2 <<'EOF'
       Run it inside the wtm-linux-build container, e.g.:
         docker run --rm -v "$PWD":/src -w /src -v wtm-target:/target \
           -e CARGO_TARGET_DIR=/target wtm-linux-build scripts/package-linux.sh
       Or, if a Linux binary was already built elsewhere, pass
       --skip-build to assemble the tarball here from that binary.
EOF
	exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

case "$output_dir" in
/*) : ;; # already absolute
*) output_dir="$repo_root/$output_dir" ;;
esac

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

# cargo writes build artifacts under $CARGO_TARGET_DIR when it's set, not
# under <repo>/target — and container/CI builds routinely set it (see the
# -e CARGO_TARGET_DIR=/target example in the usage text above). Hardcoding
# "$repo_root/target" here would make the script look in the wrong place
# whenever a build ran that way: cargo would succeed, and this script would
# then either fail to find the binary or, worse, pick up a stale one left
# over from an unrelated build, despite the real binary sitting right there
# under $CARGO_TARGET_DIR. So resolve the target directory the same way
# cargo does — do not "simplify" this back to a hardcoded path.
#
# Per Cargo's docs, a relative CARGO_TARGET_DIR is resolved against the
# current working directory cargo runs in, not the repo root. When this
# script performs the build itself it always invokes cargo from
# "$repo_root" (see the `cd "$repo_root" &&` below), so a relative value
# resolves against $repo_root here too. With --skip-build, though, cargo
# never runs in this script at all — the binary was produced by some other
# invocation whose working directory we have no way to know, so a relative
# CARGO_TARGET_DIR can't be resolved correctly here. Rather than guess (and
# risk silently assembling a tarball around the wrong binary), fail loudly
# and ask for an absolute path in that case.
if [ -n "${CARGO_TARGET_DIR:-}" ]; then
	case "$CARGO_TARGET_DIR" in
	/*) target_dir="$CARGO_TARGET_DIR" ;; # already absolute
	*)
		if [ "$skip_build" = true ]; then
			echo "error: CARGO_TARGET_DIR=$CARGO_TARGET_DIR is relative, and --skip-build" >&2
			echo "       means this script never invokes cargo itself, so it can't know" >&2
			echo "       what directory that path was resolved against when the binary was" >&2
			echo "       actually built." >&2
			echo "       Set CARGO_TARGET_DIR to an absolute path, or unset it if the binary" >&2
			echo "       is under $repo_root/target." >&2
			exit 1
		fi
		target_dir="$repo_root/$CARGO_TARGET_DIR"
		;;
	esac
else
	target_dir="$repo_root/target"
fi

binary="$target_dir/$profile/wtm-gui"

if [ "$skip_build" = false ]; then
	build_args=(build -p wtm-gui)
	[ "$profile" = release ] && build_args+=(--release)
	echo "==> cargo ${build_args[*]}"
	(cd "$repo_root" && cargo "${build_args[@]}")
fi

if [ ! -x "$binary" ]; then
	echo "error: $binary does not exist or is not executable" >&2
	echo "       (run without --skip-build, or build it manually first)" >&2
	exit 1
fi

# Confirm the binary is actually an ELF file (magic bytes 0x7fELF) rather
# than, say, a macOS Mach-O left in target/ from a native
# `cargo build -p wtm-gui` on this machine. Matters most with --skip-build
# on a non-Linux host, where nothing else here would catch a wrong binary —
# the resulting tarball would look correct until someone tried to run it.
magic="$(od -An -tx1 -N4 "$binary" | tr -d ' \n')"
if [ "$magic" != "7f454c46" ]; then
	echo "error: $binary is not an ELF binary (magic 0x$magic, want 0x7f454c46)" >&2
	echo "       it was not built for Linux — rebuild inside wtm-linux-build" >&2
	exit 1
fi

# ---------------------------------------------------------------------------
# Version — read from Cargo.toml rather than hardcoding it, matching
# bundle-mac.sh, so the tarball can never silently ship a stale version.
# ---------------------------------------------------------------------------

cargo_toml="$repo_root/crates/wtm-gui/Cargo.toml"

version="$(awk '
	/^\[package\]/ { in_pkg = 1; next }
	/^\[/          { in_pkg = 0 }
	in_pkg && /^version[[:space:]]*=/ {
		match($0, /"[^"]*"/)
		print substr($0, RSTART + 1, RLENGTH - 2)
		exit
	}
' "$cargo_toml")"

if [ -z "$version" ]; then
	echo "error: could not find version = \"...\" in [package] of $cargo_toml" >&2
	exit 1
fi

echo "==> version $version (from $cargo_toml)"

arch="$(uname -m)"
pkg_name="wtm-gui-$version-$arch-linux"

echo "==> assembling $pkg_name"

# ---------------------------------------------------------------------------
# Assemble the tarball layout in a scratch directory
# ---------------------------------------------------------------------------

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
root="$work/$pkg_name"

mkdir -p "$root/bin" "$root/share/applications"
cp "$binary" "$root/bin/wtm-gui"
chmod +x "$root/bin/wtm-gui"

linux_resources="$repo_root/crates/wtm-gui/resources/linux"

cp "$linux_resources/dev.wtm.app.desktop" "$root/share/applications/dev.wtm.app.desktop"

icons_src="$linux_resources/icons/hicolor"
for size_dir in "$icons_src"/*/apps; do
	size="$(basename "$(dirname "$size_dir")")"
	dest="$root/share/icons/hicolor/$size/apps"
	mkdir -p "$dest"
	cp "$size_dir/dev.wtm.app.png" "$dest/dev.wtm.app.png"
done

install -m 755 "$linux_resources/install.sh.template" "$root/install.sh"

sed \
	-e "s/__VERSION__/$version/g" \
	"$linux_resources/README.md.template" \
	>"$root/README.md"

# ---------------------------------------------------------------------------
# Compress
#
# .tar.xz over .tar.gz: xz's LZMA2 gets a meaningfully smaller archive than
# gzip for this binary (measured on a release build: ~4.2MB vs ~6.4MB, about
# a third smaller), and xz/unxz ship by default on every mainstream desktop
# distro this app targets. There's no minimal/embedded-system audience here
# that would make gzip's wider-but-irrelevant compatibility worth the bigger
# download.
# ---------------------------------------------------------------------------

mkdir -p "$output_dir"
tarball="$output_dir/$pkg_name.tar.xz"
rm -f "$tarball"

echo "==> writing $tarball"
tar -C "$work" -cJf "$tarball" "$pkg_name"

echo "==> built $tarball"
echo "    install: tar -xJf \"$tarball\" && ./$pkg_name/install.sh"
