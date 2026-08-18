# Third-party notices

wtm is distributed under its own MIT OR Apache-2.0 terms (see `LICENSE-MIT`
and `LICENSE-APACHE`). It bundles the following third-party assets, each
under its own license.

## Geist / Geist Mono

- **Component:** `Geist.ttf`, `Geist-Medium.ttf`, `Geist-SemiBold.ttf`,
  `Geist-Bold.ttf`, `GeistMono.ttf`
- **Source:** https://github.com/vercel/geist-font
- **Copyright:** © 2023 Vercel, Inc.
- **License:** SIL Open Font License, Version 1.1
- **Bundled at:** `crates/wtm-gui/assets/fonts/`
- **License text:** `crates/wtm-gui/assets/fonts/OFL.txt`

Static weight cuts (Medium/SemiBold/Bold) ship alongside the variable Geist
file because gpui's cosmic-text rasterizer on Linux renders variable fonts at
their default instance only and never applies `wght` axis coordinates — so
medium/semibold text would otherwise silently paint at weight 400.

## Lucide icons

- **Component:** SVG icons under `crates/wtm-gui/assets/icons/`
- **Source:** https://lucide.dev
- **Copyright:** © Lucide Icons and Contributors
- **License:** ISC License
- **License text:** `crates/wtm-gui/assets/icons/NOTICE.md`

## Everything else

All other code and assets in this repository are wtm's own and remain under
the dual MIT OR Apache-2.0 license at the repository root.
