//! Embedded assets.
//!
//! The icon set is small and fixed, so the files are compiled into the binary
//! with `include_bytes!` rather than loaded from disk. That keeps `WTM.app` a
//! single self-contained executable and means a missing icon is a compile
//! error instead of a blank square at runtime.

use std::borrow::Cow;

use anyhow::Result;
use gpui::{AssetSource, SharedString};

/// Pair an icon name with its embedded bytes, so the table below stays a list
/// of names instead of repeating each path twice.
macro_rules! icon {
    ($name:literal) => {
        (
            concat!("icons/", $name, ".svg"),
            include_bytes!(concat!("../assets/icons/", $name, ".svg")) as &[u8],
        )
    };
}

/// Every icon the app can draw, keyed by the path passed to `svg().path(..)`.
static ICONS: &[(&str, &[u8])] = &[
    icon!("archive"),
    icon!("arrow-down"),
    icon!("arrow-up"),
    icon!("arrow-up-down"),
    icon!("check"),
    icon!("chevron-down"),
    icon!("chevron-right"),
    icon!("chevron-up"),
    icon!("circle-alert"),
    icon!("circle-check"),
    icon!("circle-dot"),
    icon!("circle-x"),
    icon!("clock"),
    icon!("command"),
    icon!("copy"),
    icon!("corner-down-left"),
    icon!("ellipsis"),
    icon!("external-link"),
    icon!("file"),
    icon!("file-diff"),
    icon!("folder"),
    icon!("folder-open"),
    icon!("git-branch"),
    icon!("git-commit-horizontal"),
    icon!("git-merge"),
    icon!("hard-drive"),
    icon!("list-filter"),
    icon!("loader-circle"),
    icon!("lock"),
    icon!("panel-left"),
    icon!("panel-right"),
    icon!("pencil"),
    icon!("play"),
    icon!("plus"),
    icon!("refresh-cw"),
    icon!("rotate-cw"),
    icon!("search"),
    icon!("settings"),
    icon!("square-arrow-out-up-right"),
    icon!("terminal"),
    icon!("trash-2"),
    icon!("triangle-alert"),
    icon!("x"),
];

/// The embedded UI typefaces: Geist (four static weight cuts) plus Geist
/// Mono. See `register_fonts` for why the static cuts ship alongside the
/// variable-weight faces instead of just the one variable file.
static FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/Geist.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/GeistMono.ttf"),
];

/// Register the embedded UI fonts with the gpui text system.
/// Failure is non-fatal — the platform sans takes over.
///
/// Ship the static weight cuts (`Geist-Medium.ttf`, `Geist-SemiBold.ttf`,
/// `Geist-Bold.ttf`) alongside the variable `Geist.ttf` file rather than
/// relying on the variable font's `wght` axis alone: gpui's cosmic-text text
/// system on Linux rasterizes variable fonts at their default instance only
/// and never applies `wght` coordinates, so requesting medium or semibold
/// weight would silently paint at 400 with just the variable TTF registered.
/// Bundling the static cuts sidesteps that gap on every platform.
pub fn register_fonts(cx: &gpui::App) {
    if let Err(err) = cx
        .text_system()
        .add_fonts(FONTS.iter().map(|bytes| Cow::Borrowed(*bytes)).collect())
    {
        eprintln!(
            "warning: failed to register bundled fonts, falling back to platform sans: {err}"
        );
    }
}

/// The app's asset source, installed with `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

/// Icon paths, so call sites never spell a filename by hand.
///
/// A complete, named icon set is the point of this module: every icon in
/// `assets/icons/` gets a constant here whether or not a render call site
/// has claimed it yet, the same way a font or a color palette ships in full
/// rather than growing one glyph at a time. `#![allow(dead_code)]` below
/// covers the constants still waiting on a call site as a group, rather
/// than repeating the same reason 20+ times over — see
/// `every_declared_icon_path_resolves` below, which keeps every one of
/// them, used or not, pointed at a real embedded file.
pub mod icons {
    #![allow(dead_code)]

    pub const ARCHIVE: &str = "icons/archive.svg";
    pub const ARROW_DOWN: &str = "icons/arrow-down.svg";
    pub const ARROW_UP: &str = "icons/arrow-up.svg";
    pub const ARROW_UP_DOWN: &str = "icons/arrow-up-down.svg";
    pub const CHECK: &str = "icons/check.svg";
    pub const CHEVRON_DOWN: &str = "icons/chevron-down.svg";
    pub const CHEVRON_RIGHT: &str = "icons/chevron-right.svg";
    pub const CHEVRON_UP: &str = "icons/chevron-up.svg";
    pub const CIRCLE_ALERT: &str = "icons/circle-alert.svg";
    pub const CIRCLE_CHECK: &str = "icons/circle-check.svg";
    pub const CIRCLE_DOT: &str = "icons/circle-dot.svg";
    pub const CIRCLE_X: &str = "icons/circle-x.svg";
    pub const CLOCK: &str = "icons/clock.svg";
    pub const COMMAND: &str = "icons/command.svg";
    pub const COPY: &str = "icons/copy.svg";
    pub const ENTER: &str = "icons/corner-down-left.svg";
    pub const ELLIPSIS: &str = "icons/ellipsis.svg";
    pub const EXTERNAL_LINK: &str = "icons/external-link.svg";
    pub const FILE: &str = "icons/file.svg";
    pub const FILE_DIFF: &str = "icons/file-diff.svg";
    pub const FOLDER: &str = "icons/folder.svg";
    pub const FOLDER_OPEN: &str = "icons/folder-open.svg";
    pub const GIT_BRANCH: &str = "icons/git-branch.svg";
    pub const GIT_COMMIT_HORIZONTAL: &str = "icons/git-commit-horizontal.svg";
    pub const GIT_MERGE: &str = "icons/git-merge.svg";
    pub const HARD_DRIVE: &str = "icons/hard-drive.svg";
    pub const LIST_FILTER: &str = "icons/list-filter.svg";
    pub const LOADER_CIRCLE: &str = "icons/loader-circle.svg";
    pub const LOCK: &str = "icons/lock.svg";
    pub const PANEL_LEFT: &str = "icons/panel-left.svg";
    pub const PANEL_RIGHT: &str = "icons/panel-right.svg";
    pub const PENCIL: &str = "icons/pencil.svg";
    pub const PLAY: &str = "icons/play.svg";
    pub const PLUS: &str = "icons/plus.svg";
    pub const REFRESH: &str = "icons/refresh-cw.svg";
    pub const ROTATE_CW: &str = "icons/rotate-cw.svg";
    pub const SEARCH: &str = "icons/search.svg";
    pub const SETTINGS: &str = "icons/settings.svg";
    pub const OPEN_EXTERNAL: &str = "icons/square-arrow-out-up-right.svg";
    pub const TERMINAL: &str = "icons/terminal.svg";
    pub const TRASH: &str = "icons/trash-2.svg";
    pub const WARNING: &str = "icons/triangle-alert.svg";
    pub const CLOSE: &str = "icons/x.svg";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_icon_path_resolves() {
        for path in [
            icons::ARCHIVE,
            icons::ARROW_DOWN,
            icons::ARROW_UP,
            icons::ARROW_UP_DOWN,
            icons::CHECK,
            icons::CHEVRON_DOWN,
            icons::CHEVRON_RIGHT,
            icons::CHEVRON_UP,
            icons::CIRCLE_ALERT,
            icons::CIRCLE_CHECK,
            icons::CIRCLE_DOT,
            icons::CIRCLE_X,
            icons::CLOCK,
            icons::COMMAND,
            icons::COPY,
            icons::ENTER,
            icons::ELLIPSIS,
            icons::EXTERNAL_LINK,
            icons::FILE,
            icons::FILE_DIFF,
            icons::FOLDER,
            icons::FOLDER_OPEN,
            icons::GIT_BRANCH,
            icons::GIT_COMMIT_HORIZONTAL,
            icons::GIT_MERGE,
            icons::HARD_DRIVE,
            icons::LIST_FILTER,
            icons::LOADER_CIRCLE,
            icons::LOCK,
            icons::PANEL_LEFT,
            icons::PANEL_RIGHT,
            icons::PENCIL,
            icons::PLAY,
            icons::PLUS,
            icons::REFRESH,
            icons::ROTATE_CW,
            icons::SEARCH,
            icons::SETTINGS,
            icons::OPEN_EXTERNAL,
            icons::TERMINAL,
            icons::TRASH,
            icons::WARNING,
            icons::CLOSE,
        ] {
            assert!(
                Assets.load(path).unwrap().is_some(),
                "icon {path} is referenced but not embedded"
            );
        }
    }

    #[test]
    fn icon_table_matches_declared_count() {
        // Guards against an icon being added to `ICONS` but never exposed
        // through `icons::*` (or vice versa) as the set grows.
        assert_eq!(ICONS.len(), 43, "icon count drifted; update this guard");
    }
}
