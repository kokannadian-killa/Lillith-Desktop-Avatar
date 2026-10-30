//! # lillith-settings-gui
//!
//! Lillith's settings / options pop-up: a resizable egui window styled as
//! an ancient demon relic × DemonOS (see `lillith-theme`), with carved tabs
//! across the top — Companion, Motion, Chat / Brain (first-run LLM setup
//! wizard on `lillith-setup`), Avatar, Icon (app icon picker), Voice, About,
//! and two sealed
//! add-in slots.
//!
//! ## Launching
//!
//! The overlay runs in another process, so on `MenuEffect::OpenSettings(path)`
//! it spawns the binary:
//!
//! ```text
//! lillith-settings [CONFIG_PATH]            # default ~/.config/lillith/config.toml
//! ```
//!
//! Everything is persisted to `config.toml` (non-destructively; other tables,
//! comments and `[llm]` written by `lillith-setup` are kept). Companion picks
//! are also printed on stdout as JSON lines (`{"menu_action":"set_mode","value":"wander"}`,
//! see [`parse_action_line`]) for a host that wants to apply them live.
//!
//! In-process (tests, a future embedded mode): [`run`] / [`run_with`].
//!
//! ## Add-ins
//!
//! Implement [`SettingsTab`] and pass it through [`run_with_tabs`]:
//! `registry.unseal(Box::new(MyTab))` replaces the first sealed slot.

mod app;
pub mod bots;
pub mod chrome;
pub mod companion;
pub mod store;
pub mod tabs;
mod task;
pub mod video_import;

pub use app::SettingsApp;
// The wire format is lillith-core's (`lillith_core::menu::wire`): the
// overlay parses exactly what this window prints.
pub use lillith_core::menu::{
    action_line, action_line_in, parse_action_line, parse_pick, pick_line, wire, MenuAction,
    MenuPick,
};
pub use tabs::{SettingsTab, Shared, TabRegistry};

use std::path::PathBuf;

/// Hook that adds / unseals tabs before the window opens.
pub type Customize = Box<dyn FnOnce(&mut TabRegistry)>;

/// Receives picks in-process (the same [`MenuPick`]s that go out on stdout).
pub type ActionSink = Box<dyn FnMut(&MenuPick) + Send>;

/// Window options.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// `None` = `$XDG_CONFIG_HOME/lillith/config.toml`.
    pub config_path: Option<PathBuf>,
    /// Initial inner size (default 960×660; minimum is 640×480).
    pub inner_size: Option<[f32; 2]>,
    /// Tab id to open (`companion`, `motion`, `brain`, `avatar`, `icon`, `voice`,
    /// `build`, `bots`, `about`, `access`, `import`).
    pub start_tab: Option<String>,
    /// Sub-page for the start tab (Brain: `runtime`, `pick-model`, `persona`, `test-save`).
    pub deep_link: Option<String>,
    /// Use the window manager's title bar instead of the relic frame's.
    pub decorated: bool,
    /// Capture the first settled frame to this PNG, then exit (docs / CI).
    pub screenshot: Option<PathBuf>,
    /// `manifest.toml` of the animation set for Companion ▸ Poses (default:
    /// `lillith_anim::atlas::default_manifest_path`, the overlay's lookup).
    pub manifest: Option<PathBuf>,
    /// Print Companion picks (and `reload_config` after every successful
    /// save) on stdout as JSON lines.
    pub print_actions: bool,
}

pub const DEFAULT_SIZE: [f32; 2] = [960.0, 660.0];
pub const MIN_SIZE: [f32; 2] = [640.0, 480.0];

/// `$XDG_CONFIG_HOME/lillith/config.toml` (same as the overlay).
pub fn default_config_path() -> PathBuf {
    lillith_core::config::Config::default_path().unwrap_or_else(|| PathBuf::from("config.toml"))
}

/// The window icon for app icon `id` (its 64 px PNG), if the icon files
/// are found.
pub fn window_icon(id: &str) -> Option<std::sync::Arc<egui::IconData>> {
    let roots = lillith_core::icons::icons_dirs();
    let png = lillith_core::icons::thumbnail_in(&roots, id, 64)?;
    let img = image::open(png).ok()?.to_rgba8();
    Some(std::sync::Arc::new(egui::IconData {
        width: img.width(),
        height: img.height(),
        rgba: img.into_raw(),
    }))
}

/// Wayland app id / X11 `WM_CLASS` of the Settings window.
pub const APP_ID: &str = "lillith-settings";

/// `true` when winit will open a Wayland window (its own rule: a non-empty
/// `WAYLAND_DISPLAY` or `WAYLAND_SOCKET`), else X11.
fn wayland_session(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    ["WAYLAND_DISPLAY", "WAYLAND_SOCKET"]
        .iter()
        .any(|k| var(k).is_some_and(|v| !v.is_empty()))
}

/// eframe options for the window (Wayland app id `lillith-settings`; the
/// window icon is the configured app icon).
pub fn native_options(opts: &Options) -> eframe::NativeOptions {
    let size = opts.inner_size.unwrap_or(DEFAULT_SIZE);
    let config = opts.config_path.clone().unwrap_or_else(default_config_path);
    let icon_id = lillith_core::config::Config::load(&config)
        .map(|c| c.icon_info().id.clone())
        .unwrap_or_else(|_| lillith_core::icons::DEFAULT_ICON.to_string());
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Lillith · Settings")
        .with_inner_size(size)
        .with_min_inner_size(MIN_SIZE)
        .with_resizable(true)
        .with_decorations(opts.decorated);
    if let Some(icon) = window_icon(&icon_id) {
        viewport = viewport.with_icon(icon);
    }
    // egui turns the app id into X11 `WM_CLASS` ("", "lillith-settings"),
    // i.e. an empty instance. Without it winit's X11 default is the
    // executable name for both ("lillith-settings", "lillith-settings"), so
    // the app id is only set for Wayland (release QA 46ed853 #10).
    if wayland_session(|k| std::env::var_os(k)) {
        viewport = viewport.with_app_id(APP_ID);
    }
    eframe::NativeOptions {
        viewport,
        centered: true,
        ..Default::default()
    }
}

/// Opens the settings window on `config_path` and blocks until it closes.
pub fn run(config_path: Option<PathBuf>) -> eframe::Result<()> {
    run_with(
        Options {
            config_path,
            ..Default::default()
        },
        None,
    )
}

/// [`run`] with options and an optional in-process action sink.
pub fn run_with(opts: Options, sink: Option<ActionSink>) -> eframe::Result<()> {
    run_with_tabs(opts, sink, |_| {})
}

/// [`run_with`] plus a hook to add / unseal tabs.
pub fn run_with_tabs(
    opts: Options,
    sink: Option<ActionSink>,
    customize: impl FnOnce(&mut TabRegistry) + 'static,
) -> eframe::Result<()> {
    let native = native_options(&opts);
    eframe::run_native(
        "lillith-settings",
        native,
        Box::new(move |cc| {
            Ok(Box::new(SettingsApp::new(
                cc,
                opts,
                sink,
                Some(Box::new(customize)),
            )))
        }),
    )
}

#[cfg(test)]
mod wm_class_tests {
    use super::*;

    #[test]
    fn app_id_only_for_wayland() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        assert!(wayland_session(env(&[("WAYLAND_DISPLAY", "wayland-0")])));
        assert!(wayland_session(env(&[("WAYLAND_SOCKET", "3")])));
        assert!(!wayland_session(env(&[("DISPLAY", ":0")])));
        assert!(!wayland_session(env(&[
            ("WAYLAND_DISPLAY", ""),
            ("DISPLAY", ":0")
        ])));
        assert_eq!(APP_ID, "lillith-settings");
    }
}
