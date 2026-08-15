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
