//! `wtm-gui` — the wtm desktop app.
//!
//! This binary is what lives inside `WTM.app`. It is deliberately separate
//! from the `wtm` CLI binary: the CLI stays small and starts instantly, while
//! everything gpui pulls in (Metal, CoreText, a font stack) is confined to
//! this crate.
//!
//! Launch context: an optional path argument names the repository to open.
//! Without one — the Dock/Spotlight case — the app falls back to the current
//! directory's repository, then to whatever `prefs.last_repo` remembers.
//!
//! This file also owns the keyboard-shortcut table (`key_bindings!` below)
//! and the startup wiring for persisted preferences: loading them before the
//! window opens, restoring the window frame and appearance they name, and
//! saving them back on the "meaningful change" triggers named in
//! `crate::app::WtmApp` (sidebar/detail-panel toggle, repo switch) plus
//! window close, handled here via `on_window_should_close`.

mod app;
mod assets;
mod context_menu;
mod data;
mod detail_panel;
mod dialogs;
mod diff_view;
mod file_browser;
mod motion;
mod palette;
mod prefs;
mod run_panel;
mod settings;
mod text_input;
mod theme;
mod ui;
mod watcher;
mod window_frame;
mod worktree_list;

use std::path::PathBuf;

use gpui::prelude::*;
use gpui::{
    actions, point, px, size, App, Application, Bounds, KeyBinding, Menu, MenuItem, Pixels,
    SystemMenuType, TitlebarOptions, WindowBackgroundAppearance, WindowBounds, WindowOptions,
};
// Only the client-side-decoration request below (`request_decorations`) uses
// this; on macOS there is no such request to make, so the import would be
// dead there.
#[cfg(not(target_os = "macos"))]
use gpui::WindowDecorations;

use app::WtmApp;
use assets::Assets;
use prefs::WindowFrame;

actions!(wtm, [Quit]);

/// One macro invocation per keyboard shortcut this app registers: the
/// keystroke (gpui syntax), the action it triggers, the key context (`None`
/// for a window-global binding), a display glyph, and a human label.
///
/// Each entry is written exactly once. The macro expands it into both
/// `registered_key_bindings` — the real `Vec<KeyBinding>` `cx.bind_keys`
/// installs below — and `REGISTERED_BINDINGS`, the metadata the settings
/// sheet's "Keyboard Shortcuts" list reads (see `crate::settings`). Because
/// both come from the same list, the display can never silently drift from
/// what is actually bound the way two hand-maintained lists could.
macro_rules! key_bindings {
    ($( $keys:literal, $action:expr, $ctx:expr, $display:literal, $label:literal );* $(;)?) => {
        /// The real bindings, built fresh each call (gpui `Action`s are
        /// cheap unit structs) rather than cached, since `cx.bind_keys` only
        /// ever needs this once at startup.
        pub(crate) fn registered_key_bindings() -> Vec<KeyBinding> {
            vec![$( KeyBinding::new($keys, $action, $ctx) ),*]
        }

        /// Display mirror of `registered_key_bindings`; see the macro doc
        /// above for why these cannot drift apart.
        pub(crate) const REGISTERED_BINDINGS: &[settings::ShortcutMeta] = &[
            $(
                settings::ShortcutMeta {
                    keystroke: $keys,
                    context: $ctx,
                    display: $display,
                    label: $label,
                }
            ),*
        ];
    };
}

key_bindings! {
    "cmd-q", Quit, None, "⌘Q", "Quit";
    "cmd-r", app::Reload, Some("WtmApp"), "⌘R", "Reload";
    "cmd-shift-f", app::FetchRemote, Some("WtmApp"), "⌘⇧F", "Fetch";
    "enter", app::OpenSelected, Some("WtmApp"), "⏎", "Open in Editor";
    "down", app::SelectNext, Some("WtmApp"), "↓", "Select Next";
    "up", app::SelectPrev, Some("WtmApp"), "↑", "Select Previous";
    "cmd-b", app::ToggleSidebar, Some("WtmApp"), "⌘B", "Toggle Sidebar";
    "cmd-n", app::NewWorktree, Some("WtmApp"), "⌘N", "New Worktree";
    "cmd-backspace", app::RemoveSelected, Some("WtmApp"), "⌘⌫", "Remove Worktree";
    "delete", app::RemoveSelected, Some("WtmApp"), "⌦", "Remove Worktree";
    "cmd-shift-p", app::PruneRepo, Some("WtmApp"), "⌘⇧P", "Prune Worktrees";
    "cmd-c", app::CopyPath, Some("WtmApp"), "⌘C", "Copy Path";
    "cmd-shift-t", app::OpenInTerminal, Some("WtmApp"), "⌘⇧T", "Open in Terminal";
    "cmd-shift-r", app::RevealInFinder, Some("WtmApp"), "⌘⇧R", "Reveal in Finder";
    "escape", app::CloseDialog, Some("WtmApp"), "⎋", "Close Dialog or Menu";
    "cmd-i", app::ToggleDetailPanel, Some("WtmApp"), "⌘I", "Toggle Detail Panel";
    "cmd-1", app::ShowDetailsTab, Some("WtmApp"), "⌘1", "Detail Panel: Details Tab";
    "cmd-2", app::ShowFilesTab, Some("WtmApp"), "⌘2", "Detail Panel: Files Tab";
    "cmd-3", app::ShowChangesTab, Some("WtmApp"), "⌘3", "Detail Panel: Changes Tab";
    "cmd-,", app::OpenSettings, Some("WtmApp"), "⌘,", "Settings";
    "cmd-k", app::OpenPalette, Some("WtmApp"), "⌘K", "Command Palette";
    "cmd-f", app::FocusFilter, Some("WtmApp"), "⌘F", "Filter Worktrees";
    "cmd-shift-o", app::AddRepository, Some("WtmApp"), "⌘⇧O", "Add Repository";
    "cmd-e", app::RunCommand, Some("WtmApp"), "⌘E", "Run Command";
}

/// The default window size and position, used when no saved frame exists or
/// the saved one no longer lands on a connected display.
const DEFAULT_WINDOW_SIZE: (f32, f32) = (1180.0, 760.0);

/// The titlebar `WindowOptions` asks for, per platform.
///
/// macOS: unchanged from before Linux support existed — a transparent
/// native titlebar with the traffic lights inset into the app's own
/// title-bar strip (see `app::chrome::render_titlebar`).
#[cfg(target_os = "macos")]
fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some("wtm".into()),
        appears_transparent: true,
        traffic_light_position: Some(point(px(16.0), px(17.0))),
    }
}

/// Linux: `appears_transparent` and `traffic_light_position` are both
/// documented as macOS-only concepts in gpui's own `TitlebarOptions` (the
/// former's doc comment points here, at `WindowOptions::window_decorations`,
/// for what to use on Linux instead — see `request_decorations` below).
/// `title` still does something real here though: X11's backend uses it to
/// set `WM_NAME` (the taskbar/alt-tab label) regardless of who ends up
/// drawing the frame around the window.
#[cfg(not(target_os = "macos"))]
fn titlebar_options() -> TitlebarOptions {
    TitlebarOptions {
        title: Some("wtm".into()),
        appears_transparent: false,
        traffic_light_position: None,
    }
}

/// The window background `WindowOptions` asks for, per platform.
///
/// macOS: unchanged — vibrancy behind the (mostly-opaque) sidebar tint, as
/// `theme::Theme::sidebar`'s own doc explains.
#[cfg(target_os = "macos")]
const WINDOW_BACKGROUND: WindowBackgroundAppearance = WindowBackgroundAppearance::Blurred;

/// Linux has no vibrancy equivalent, so `Blurred` is not an option — but
/// the choice between the two backgrounds gpui *does* support here is not
/// as simple as picking `Opaque` and moving on:
///
/// `window_frame::wrap` draws rounded corners under client-side
/// decorations, with a shadow cast into the margin outside them. An opaque
/// window background is a full, literal rectangle at the platform level —
/// window managers do not know or care that our own content stops short of
/// the corners with rounded edges, so the four corners outside the rounded
/// rect (and the whole shadow margin) would still be *opaque*, painted in
/// whatever the platform's default backing color is. That reads as a solid
/// square block sitting behind the rounded card, not a floating window with
/// a soft shadow. Only `Transparent` lets those pixels genuinely composite
/// with the desktop, which is what makes the rounded corners and the shadow
/// look like a window rather than a screenshot of one.
///
/// This costs nothing when the compositor instead draws server-side
/// decorations (`Decorations::Server`): `window_frame::wrap` is a no-op in
/// that case, our own content fills the window edge-to-edge exactly as it
/// would under `Opaque`, and a transparent background with 100%-opaque
/// content on top is visually identical to an opaque one. The one real
/// tradeoff is compositor dependence — on an X11 session with no compositor
/// running at all, a `Transparent` window can render incorrectly (this is a
/// limitation shared by every app that draws rounded, shadowed client-side
/// decorations, not specific to wtm); compositor-less X11 is rare enough
/// today that this is judged worth it for the common case.
#[cfg(not(target_os = "macos"))]
const WINDOW_BACKGROUND: WindowBackgroundAppearance = WindowBackgroundAppearance::Transparent;

/// Convert a persisted [`WindowFrame`] into gpui's `Bounds<Pixels>`.
fn window_frame_bounds(frame: &WindowFrame) -> Bounds<Pixels> {
    Bounds {
        origin: point(px(frame.x), px(frame.y)),
        size: size(px(frame.width), px(frame.height)),
    }
}

/// Whether `frame` overlaps at least one display in `display_bounds`. A
/// window frame saved while on a monitor that has since been unplugged (or
/// on a display arrangement that no longer exists) would otherwise reopen
/// entirely off-screen — a position no mouse or keyboard shortcut can reach
/// back from — so a frame with zero overlap on every currently connected
/// display is rejected in favor of the centered default.
///
/// Pure and independent of gpui's real `PlatformDisplay` (nothing outside a
/// running `App` can construct one), so this is testable directly against
/// synthetic display bounds.
fn frame_is_reachable(frame: &WindowFrame, display_bounds: &[Bounds<Pixels>]) -> bool {
    if frame.width <= 0.0 || frame.height <= 0.0 {
        return false;
    }
    let frame_bounds = window_frame_bounds(frame);
    display_bounds.iter().any(|d| d.intersects(&frame_bounds))
}

/// Snapshot `view`'s live preferences, stamp in `window`'s current frame, and
/// persist. Shared by both quit paths registered in `main` (window-close and
/// app-quit) so the frame-capture logic exists exactly once.
fn save_prefs_with_window_frame(view: &gpui::Entity<WtmApp>, window: &gpui::Window, cx: &App) {
    let mut prefs = view.read(cx).prefs_snapshot();
    let bounds = window.window_bounds().get_bounds();
    prefs.window = Some(WindowFrame {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
    });
    if let Err(e) = prefs::save(&prefs) {
        eprintln!("wtm: could not save preferences: {e}");
    }
}

fn main() {
    let requested_repo = std::env::args().nth(1).map(PathBuf::from);
    let prefs = prefs::load();

    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            // Register the bundled Geist/Geist Mono faces before anything
            // paints. Failure is non-fatal (see `assets::register_fonts`) —
            // this is best-effort, not a gate on startup.
            assets::register_fonts(cx);
            theme::init(cx);
            // `theme::init` already resolved the OS appearance; a forced
            // Light/Dark preference overrides that immediately by handing
            // `theme::refresh` the appearance it would produce that palette
            // for, regardless of what the OS is actually running — see
            // `WtmApp::set_appearance` for the same trick used at runtime.
            match prefs.appearance {
                prefs::Appearance::System => {}
                prefs::Appearance::Light => theme::refresh(gpui::WindowAppearance::Light, cx),
                prefs::Appearance::Dark => theme::refresh(gpui::WindowAppearance::Dark, cx),
            }
            // Same "apply the persisted preference at startup" treatment as
            // appearance above — see `WtmApp::set_reduce_motion` for the
            // runtime toggle that keeps this in sync after launch.
            motion::set_reduced(cx, prefs.reduce_motion);
            cx.activate(true);

            cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
            cx.bind_keys(registered_key_bindings());
            cx.set_menus(vec![
                Menu {
                    name: "wtm".into(),
                    items: vec![
                        MenuItem::os_submenu("Services", SystemMenuType::Services),
                        MenuItem::separator(),
                        MenuItem::action("Quit wtm", Quit),
                    ],
                },
                Menu {
                    name: "View".into(),
                    items: vec![
                        MenuItem::action("Reload", app::Reload),
                        MenuItem::action("Toggle Sidebar", app::ToggleSidebar),
                    ],
                },
            ]);

            // Resolve which repository to show before opening the window so
            // the first paint already has something in it: the CLI argument,
            // then the cwd's repository, then — only when neither applies —
            // whatever repository was open last. `WtmApp::new` below takes
            // this the rest of the way: it lists this repository's
            // worktrees synchronously too, so the rows are already in
            // `initial`'s repository by the time the window's first frame
            // is painted — see `WtmApp::seed_initial_rows` for why that
            // matters.
            let initial = match &requested_repo {
                Some(path) => match data::open_repo(path) {
                    Ok(repo) => Some(repo),
                    Err(e) => {
                        eprintln!("wtm: {e}");
                        None
                    }
                },
                None => data::open_repo_from_cwd().or_else(|| {
                    prefs
                        .last_repo
                        .as_deref()
                        .and_then(|path| data::open_repo(path).ok())
                }),
            };

            let display_bounds: Vec<Bounds<Pixels>> =
                cx.displays().iter().map(|d| d.bounds()).collect();
            let default_bounds = Bounds::centered(
                None,
                size(px(DEFAULT_WINDOW_SIZE.0), px(DEFAULT_WINDOW_SIZE.1)),
                cx,
            );
            let bounds = prefs
                .window
                .as_ref()
                .filter(|frame| frame_is_reachable(frame, &display_bounds))
                .map(window_frame_bounds)
                .unwrap_or(default_bounds);

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                // The app draws its own title bar so the sidebar can run to
                // the top of the window; macOS still owns the traffic lights,
                // which are inset to sit inside that strip. On Linux, this
                // titlebar has no OS-drawn buttons at all — see
                // `titlebar_options` and `app::chrome::render_titlebar`.
                titlebar: Some(titlebar_options()),
                window_background: WINDOW_BACKGROUND,
                window_min_size: Some(size(px(820.), px(520.))),
                app_id: Some("dev.wtm.app".to_string()),
                ..Default::default()
            };

            cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| WtmApp::new(initial, prefs.clone(), window, cx));

                // Ask the compositor to let the app draw its own frame
                // (a title bar with no OS buttons, and — when granted —
                // `window_frame::wrap`'s rounded corners) instead of a
                // server-side one, so Linux gets the same custom-chrome
                // look macOS already has rather than a second, native title
                // bar sitting above this one. A no-op on macOS (there is no
                // Linux compositor to ask), and not guaranteed even here: a
                // window manager without client-decoration support silently
                // keeps `Decorations::Server` regardless of this request —
                // `window_frame::wrap` and `render_titlebar` both read the
                // real answer back from `window.window_decorations()` every
                // frame rather than assuming this call was honored.
                #[cfg(not(target_os = "macos"))]
                window.request_decorations(WindowDecorations::Client);

                // Ask the platform to bring this window to the foreground
                // and make it key. This matters beyond the obvious UX
                // reason: gpui's macOS backend only (re)starts the
                // `CVDisplayLink` that drives repaints when Cocoa reports a
                // window occlusion/visibility change (or a screen change) —
                // `cx.notify()`/`window.refresh()` only mark state dirty,
                // they never themselves ask the platform for a frame. A
                // window that never becomes key can be left showing nothing
                // but the one frame painted synchronously during window
                // setup, forever, until something (a click) changes its
                // occlusion state. `activate_window` is the correct thing to
                // ask for here, but it is not a guaranteed fix on its own:
                // in at least one observed automated session the OS refused
                // to hand over focus at all, so this call did nothing. The
                // real mitigation for that failure mode is making sure the
                // *synchronous* first frame already has real content — see
                // `WtmApp::seed_initial_rows`.
                window.activate_window();

                // Keep the palette in step with the system appearance while
                // the app is open — but only when the user hasn't forced a
                // Light/Dark preference; overriding unconditionally here
                // would silently discard that preference the next time the
                // OS appearance changes.
                window
                    .observe_window_appearance({
                        let view = view.clone();
                        move |window, cx| {
                            if view.read(cx).follows_system_appearance() {
                                theme::refresh(window.appearance(), cx);
                            }
                        }
                    })
                    .detach();

                // Persist preferences on close: sidebar/detail-panel
                // visibility, appearance, and last-opened repo are already
                // saved as they change (see `WtmApp::save_prefs`'s call
                // sites), so this only needs to add the one thing that's
                // meaningless to save more often than once — the window
                // frame at the moment it's about to disappear.
                //
                // Two hooks, not one: `on_window_should_close` fires for an
                // OS-level close request on this window (the traffic-light
                // button), but ⌘Q and the "Quit wtm" menu item both invoke
                // `App::quit` directly (see `cx.on_action(|_: &Quit, ...)`
                // above), which tears the app down through `App::shutdown`
                // *without* ever asking any window whether it should close.
                // Missing `on_app_quit` here would mean the window frame is
                // never saved on the quit gesture most users actually reach
                // for.
                window.on_window_should_close(cx, {
                    let view = view.clone();
                    move |window, cx| {
                        save_prefs_with_window_frame(&view, window, cx);
                        true
                    }
                });

                cx.on_app_quit({
                    let view = view.clone();
                    move |cx| {
                        // `shutdown` runs quit observers before it clears
                        // `cx.windows()`, so the window this app opened is
                        // still there to read bounds from.
                        if let Some(handle) = cx.windows().first().copied() {
                            let _ = handle.update(cx, |_root, window, cx| {
                                save_prefs_with_window_frame(&view, window, cx);
                            });
                        }
                        std::future::ready(())
                    }
                })
                .detach();

                view
            })
            .expect("failed to open the wtm window");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(x), px(y)),
            size: size(px(w), px(h)),
        }
    }

    fn frame(x: f32, y: f32, w: f32, h: f32) -> WindowFrame {
        WindowFrame {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn frame_fully_on_a_display_is_reachable() {
        let displays = [display(0.0, 0.0, 1920.0, 1080.0)];
        assert!(frame_is_reachable(
            &frame(100.0, 100.0, 800.0, 600.0),
            &displays
        ));
    }

    #[test]
    fn frame_off_every_display_is_rejected() {
        // A frame from a since-unplugged second monitor to the right of a
        // single remaining 1920-wide display.
        let displays = [display(0.0, 0.0, 1920.0, 1080.0)];
        assert!(!frame_is_reachable(
            &frame(2500.0, 100.0, 800.0, 600.0),
            &displays
        ));
    }

    #[test]
    fn frame_partially_overlapping_a_display_is_reachable() {
        // A sliver of titlebar overlap is enough to drag the window back.
        let displays = [display(0.0, 0.0, 1920.0, 1080.0)];
        assert!(frame_is_reachable(
            &frame(1900.0, 100.0, 800.0, 600.0),
            &displays
        ));
    }

    #[test]
    fn frame_reachable_on_a_secondary_display() {
        let displays = [
            display(0.0, 0.0, 1920.0, 1080.0),
            display(1920.0, 0.0, 1920.0, 1080.0),
        ];
        assert!(frame_is_reachable(
            &frame(2200.0, 200.0, 800.0, 600.0),
            &displays
        ));
    }

    #[test]
    fn zero_sized_frame_is_rejected_even_if_positioned_on_a_display() {
        let displays = [display(0.0, 0.0, 1920.0, 1080.0)];
        assert!(!frame_is_reachable(
            &frame(100.0, 100.0, 0.0, 0.0),
            &displays
        ));
    }

    #[test]
    fn no_displays_means_nothing_is_reachable() {
        assert!(!frame_is_reachable(&frame(0.0, 0.0, 800.0, 600.0), &[]));
    }
}
