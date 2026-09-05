#!/usr/bin/env bash
#
# bundle-mac.sh — assemble WTM.app, the macOS application bundle for wtm-gui.
#
# The `wtm` CLI (see src/commands/app.rs) looks for the app at
# /Applications/WTM.app, ~/Applications/WTM.app, $WTM_APP, or a `wtm-gui`
# binary next to itself / on $PATH. This script produces the first kind of
# artifact: a real .app bundle that `open -a WTM.app` and Spotlight/the Dock
# can find by name and icon, not just a bare executable.
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: scripts/bundle-mac.sh [options]

Build wtm-gui and assemble it into WTM.app, a macOS application bundle.

Options:
  --release       Build the release profile (default).
  --debug         Build the debug profile instead. Much faster; use this
                   while iterating on the bundle itself.
  --output <dir>  Directory the bundle is written into, as <dir>/WTM.app.
                   Relative paths are resolved against the repo root.
                   Default: target/bundle
  --sign <id>     Codesign with this identity (as accepted by `codesign
                   --sign`), e.g. a "Developer ID Application: ..." identity
                   from `security find-identity -v -p codesigning`. Without
                   this flag the bundle is ad-hoc signed instead (see the
                   comment above the codesign call for what that does and
                   does not buy you).
  --skip-build    Reuse the existing target/<profile>/wtm-gui binary instead
                   of invoking cargo. The binary must already exist.
  --open          Launch the bundle with `open` once it is assembled.
  -h, --help      Show this help and exit.

Examples:
  scripts/bundle-mac.sh --debug --open        # fast local loop
  scripts/bundle-mac.sh --sign "Developer ID Application: Jane Doe (TEAMID)"
EOF
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

profile=release
output_dir=target/bundle
sign_identity=""
skip_build=false
do_open=false

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
	--sign)
		[ $# -ge 2 ] || {
			echo "error: --sign requires an identity argument" >&2
			exit 1
		}
		sign_identity=$2
		shift 2
		;;
	--skip-build)
		skip_build=true
		shift
		;;
	--open)
		do_open=true
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

# App bundles are a macOS-specific concept (Info.plist, .icns, codesign, the
# `open` command all only make sense there); fail loudly rather than half
# producing a directory tree that looks like a bundle but isn't one.
if [ "$(uname -s)" != "Darwin" ]; then
	echo "error: bundle-mac.sh only runs on macOS (got $(uname -s))" >&2
	exit 1
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"

case "$output_dir" in
/*) : ;; # already absolute
*) output_dir="$repo_root/$output_dir" ;;
esac

bundle="$output_dir/WTM.app"
contents="$bundle/Contents"

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

# Resolve the target dir the way cargo does, since CI/container builds
# routinely set CARGO_TARGET_DIR. A relative value is only resolvable when
# this script itself invokes cargo from $repo_root; with --skip-build it
# can't be, so that combination is rejected below rather than guessed at.
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

# ---------------------------------------------------------------------------
# Version — read from Cargo.toml rather than hardcoding it, so the bundle
# can never silently ship a stale version string once the crate is bumped.
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

# ---------------------------------------------------------------------------
# Assemble the bundle layout
# ---------------------------------------------------------------------------

echo "==> assembling $bundle"
rm -rf "$bundle"
mkdir -p "$contents/MacOS" "$contents/Resources"

cp "$binary" "$contents/MacOS/wtm-gui"
chmod +x "$contents/MacOS/wtm-gui"

sed \
	-e "s/__VERSION__/$version/g" \
	-e "s/__YEAR__/$(date +%Y)/g" \
	"$repo_root/crates/wtm-gui/resources/Info.plist" \
	>"$contents/Info.plist"

# PkgInfo is a legacy-but-still-checked-by-some-tools 8-byte redundant
# encoding of CFBundlePackageType + CFBundleSignature. printf (not echo) so
# no trailing newline sneaks into the 8 bytes.
printf 'APPL????' >"$contents/PkgInfo"

# ---------------------------------------------------------------------------
# Icon
# ---------------------------------------------------------------------------

icns_out="$contents/Resources/wtm.icns"
icon_composer_src="$repo_root/assets/wtm.icon"
icon_png_src="$repo_root/assets/icon-src/wtm-icon-1024.png"

# Xcode 26's actool can compile an Icon Composer `.icon` bundle (our
# assets/wtm.icon, which carries the gradient background, glyph layer and
# shadow/translucency the flattened PNG doesn't have) directly to a
# ready-to-use .icns via `--app-icon <name> --include-all-app-icons`. That
# flag combination only exists on actool >= 26; older Xcodes, or a machine
# with only the Command Line Tools and no Xcode.app, don't have it, so this
# is attempted first and silently falls back to the sips/iconutil path on
# any failure.
try_actool_icns() {
	command -v actool >/dev/null 2>&1 || return 1
	[ -d "$icon_composer_src" ] || return 1

	local major
	major="$(actool --version 2>/dev/null | plutil -p - 2>/dev/null |
		sed -n 's/.*"short-bundle-version" => "\([0-9]*\).*/\1/p')"
	[ -n "$major" ] && [ "$major" -ge 26 ] || return 1

	local work
	work="$(mktemp -d)"
	trap 'rm -rf "$work"' RETURN

	# actool infers the icon's name from the .icon directory name, and
	# writes its .icns output as "<name>.icns" — copy under a fixed name
	# so the rest of this function doesn't have to guess it back out.
	cp -R "$icon_composer_src" "$work/Icon.icon"
	mkdir -p "$work/out"

	actool "$work/Icon.icon" \
		--compile "$work/out" \
		--output-format human-readable-text \
		--notices --warnings \
		--output-partial-info-plist "$work/out/info.plist" \
		--app-icon Icon \
		--include-all-app-icons \
		--enable-on-demand-resources NO \
		--development-region en \
		--target-device mac \
		--minimum-deployment-target 26.0 \
		--platform macosx \
		>/dev/null || return 1

	[ -f "$work/out/Icon.icns" ] || return 1
	cp "$work/out/Icon.icns" "$icns_out"
}

# Plain sips + iconutil, which ships with every macOS install that has the
# Command Line Tools. Builds a classic 10-image .iconset from the flattened
# 1024px master. The sizes are macOS's fixed, non-negotiable menu: iconutil
# rejects an .iconset that is missing any of the 1x/2x pairs from 16pt up to
# 512pt, and the "@2x" entries are what give Retina displays a crisp icon
# instead of an upscaled blurry one.
build_icns_with_sips() {
	[ -f "$icon_png_src" ] || {
		echo "error: no icon source found ($icon_composer_src or $icon_png_src)" >&2
		exit 1
	}

	local work iconset
	work="$(mktemp -d)"
	trap 'rm -rf "$work"' RETURN
	iconset="$work/wtm.iconset"
	mkdir -p "$iconset"

	local size name
	for entry in \
		"16 icon_16x16" \
		"32 icon_16x16@2x" \
		"32 icon_32x32" \
		"64 icon_32x32@2x" \
		"128 icon_128x128" \
		"256 icon_128x128@2x" \
		"256 icon_256x256" \
		"512 icon_256x256@2x" \
		"512 icon_512x512" \
		"1024 icon_512x512@2x"; do
		size="${entry%% *}"
		name="${entry#* }"
		sips -z "$size" "$size" "$icon_png_src" --out "$iconset/$name.png" >/dev/null
	done

	iconutil -c icns "$iconset" -o "$icns_out"
}

echo "==> generating wtm.icns"
if try_actool_icns; then
	echo "    via actool (Icon Composer source)"
else
	echo "    via sips/iconutil (flattened PNG fallback)"
	build_icns_with_sips
fi

# ---------------------------------------------------------------------------
# Codesign
# ---------------------------------------------------------------------------

if [ -n "$sign_identity" ]; then
	echo "==> codesigning with $sign_identity"
	codesign --force --deep --sign "$sign_identity" "$bundle"
else
	# Ad-hoc signing ("-" as the identity) satisfies the *local* Gatekeeper
	# check that refuses to run an unsigned Mach-O at all — without it,
	# launching the freshly built bundle on this same machine can already
	# fail. It is NOT a substitute for a real Developer ID signature plus
	# notarization: an ad-hoc signature has no certificate chain, so any
	# *other* machine (or this one, once the bundle has been through
	# AirDrop/a browser download and picked up a quarantine xattr) will
	# still have Gatekeeper block it. Use --sign with a real identity, then
	# notarize, before distributing the bundle to anyone else.
	echo "==> ad-hoc codesigning (no --sign identity given)"
	codesign --force --deep --sign - "$bundle"
fi
# --deep walks into nested bundles/frameworks and signs each one. This app
# does not embed any today, but gpui's dependency surface (Metal, video
# decoding) makes it plausible a future build vendors a .framework, and a
# top-level-only signature over an unsigned nested binary is invalid anyway
# — so signing deep is the safe default rather than something to special-case
# later.

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------

echo "==> built $bundle"
echo "    install: cp -R \"$bundle\" /Applications/"

if [ "$do_open" = true ]; then
	open "$bundle"
fi
