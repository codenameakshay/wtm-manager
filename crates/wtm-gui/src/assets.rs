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
    icon!("check"),
    icon!("copy"),
    icon!("corner-down-left"),
    icon!("folder"),
    icon!("git-branch"),
    icon!("lock"),
    icon!("panel-left"),
    icon!("plus"),
    icon!("refresh-cw"),
    icon!("search"),
    icon!("settings"),
    icon!("square-arrow-out-up-right"),
    icon!("trash-2"),
    icon!("triangle-alert"),
    icon!("x"),
];

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
pub mod icons {
    pub const CHECK: &str = "icons/check.svg";
    pub const COPY: &str = "icons/copy.svg";
    pub const ENTER: &str = "icons/corner-down-left.svg";
    pub const FOLDER: &str = "icons/folder.svg";
    pub const GIT_BRANCH: &str = "icons/git-branch.svg";
    pub const LOCK: &str = "icons/lock.svg";
    pub const PANEL_LEFT: &str = "icons/panel-left.svg";
    pub const PLUS: &str = "icons/plus.svg";
    pub const REFRESH: &str = "icons/refresh-cw.svg";
    pub const SEARCH: &str = "icons/search.svg";
    pub const SETTINGS: &str = "icons/settings.svg";
    pub const OPEN_EXTERNAL: &str = "icons/square-arrow-out-up-right.svg";
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
            icons::CHECK,
            icons::COPY,
            icons::ENTER,
            icons::FOLDER,
            icons::GIT_BRANCH,
            icons::LOCK,
            icons::PANEL_LEFT,
            icons::PLUS,
            icons::REFRESH,
            icons::SEARCH,
            icons::SETTINGS,
            icons::OPEN_EXTERNAL,
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
}
