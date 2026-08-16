# scripts/

## `bundle-mac.sh`

Builds `wtm-gui` and packages it into `WTM.app`, the macOS application bundle
that the `wtm` CLI looks for at `/Applications/WTM.app` and
`~/Applications/WTM.app` (see `src/commands/app.rs`). Run this whenever you
need a real `.app` to install, double-click, `open -a` by name, or hand to
someone else — a bare `cargo build -p wtm-gui` only produces a binary that
doesn't show up in the Dock, Spotlight, or `open -a`.

```
scripts/bundle-mac.sh --debug --open      # fast local loop while iterating
scripts/bundle-mac.sh                     # release build → target/bundle/WTM.app
scripts/bundle-mac.sh --sign "Developer ID Application: Jane Doe (TEAMID)"
```

See `scripts/bundle-mac.sh --help` for the full flag list (output directory,
skipping the cargo build, codesigning identity). macOS only — it uses
`sips`/`iconutil`/`actool`/`codesign`, none of which exist elsewhere.

The bundle it produces is ad-hoc signed by default, which is enough to launch
locally but is not notarized and will not pass Gatekeeper on another Mac; use
`--sign` with a real Developer ID identity before distributing it.

## `package-linux.sh`

Builds `wtm-gui` and packages it into a self-contained Linux tarball,
`wtm-gui-<version>-<arch>-linux.tar.xz`, laid out so it can be extracted
anywhere and installed with the `install.sh` bundled inside it:

```
wtm-gui-<version>-<arch>-linux/
  bin/wtm-gui
  share/applications/dev.wtm.app.desktop
  share/icons/hicolor/<size>x<size>/apps/dev.wtm.app.png   (16 through 512)
  install.sh          # copies the above into ~/.local or /usr/local and
                       # refreshes the desktop/icon caches
  README.md           # what this is, runtime deps, how to install
```

```
scripts/package-linux.sh --debug            # fast local loop
scripts/package-linux.sh                    # release build → the tarball above
scripts/package-linux.sh --skip-build       # reuse an existing target/<profile>/wtm-gui
```

See `scripts/package-linux.sh --help` for the full flag list. **This script
builds on Linux, not on this repo's usual macOS dev machine** — gpui's Linux
backend doesn't cross-compile from a Mac, so run it on a Linux box or inside
the `wtm-linux-build` container:

```
docker run --rm -v "$PWD":/src -w /src -v wtm-target:/src/target \
  wtm-linux-build scripts/package-linux.sh
```

`--skip-build` is the only way to invoke it on macOS, and only to assemble a
tarball from a binary that was already cross-built elsewhere (e.g. copied
out of that container); the script verifies the reused binary is actually an
ELF file before packaging it, precisely to catch a stray native macOS build
sitting in `target/` instead.

The `.desktop` file and hicolor icon set it packages live under
`crates/wtm-gui/resources/linux/`, generated once from
`assets/icon-src/wtm-icon-1024.png` via `sips -Z <size>` and committed as
real files (there is no Linux box in this repo's normal dev loop to
regenerate them from, unlike the mac bundle's icon step).

### `.deb` via cargo-deb

There is no wrapper script for the `.deb` — `crates/wtm-gui/Cargo.toml`'s
`[package.metadata.deb]` section carries everything `cargo-deb` needs, so
building one is just:

```
cargo install cargo-deb   # once
cargo deb -p wtm-gui      # → target/debian/wtm-gui_<version>-1_<arch>.deb
```

`depends` combines cargo-deb's `$auto` (which resolves the binary's directly
linked libraries — `libc6`, `libxcb1`, `libxkbcommon0`,
`libxkbcommon-x11-0`, `zlib1g` — to their Debian packages by running the
equivalent of `ldd` + `dpkg -S` against the built binary) with two
hand-added packages, `libvulkan1` and `libwayland-client0`, that `$auto`
cannot see: gpui's Linux backend `dlopen`s both at startup instead of
linking them, so they leave no trace in the ELF's dynamic section for
`$auto` to find, but the app cannot open a window without `libvulkan1` and
can't run under Wayland without `libwayland-client0`.

### Runtime dependencies (both artifacts)

`libc6`, `libgcc-s1`, `zlib1g`, `libxcb1`, `libxkbcommon0`,
`libxkbcommon-x11-0`, `libvulkan1`, `libwayland-client0` — see the tarball's
bundled `README.md` or the `.deb`'s `Depends` field for the full story.
These are all present on essentially any desktop Linux install; a minimal
server/container base image is the likeliest place to be missing
`libvulkan1` or `libwayland-client0` specifically.

**Neither artifact has been run on real Linux desktop hardware yet** — only
built, installed, and inspected inside the headless `wtm-linux-build`
container (`dpkg -i`, `dpkg -c`, `desktop-file-validate`, extracting the
tarball and running `install.sh`). Treat both as unverified beyond "the
files land in the right place" until someone runs the actual app on a
Linux desktop.
