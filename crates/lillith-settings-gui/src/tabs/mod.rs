//! Tabs: the [`SettingsTab`] trait and the [`TabRegistry`] that orders
//! them. Add-ins implement `SettingsTab` and either replace a sealed slot
//! ([`TabRegistry::unseal`]) or are pushed with [`TabRegistry::register`].

pub mod about;
pub mod avatar;
pub mod bots;
pub mod bounce;
pub mod brain;
pub mod build;
pub mod companion;
pub mod icon;
pub mod import;
pub mod knobs;
pub mod sealed;

use crate::store::ConfigStore;
use lillith_core::menu::MenuPick;
use lillith_setup::detect::RuntimeReport;
use lillith_theme::palette::Rgba;
use std::path::PathBuf;

/// State every tab can read / change.
pub struct Shared {
    /// Staged edits of config.toml (everything but `[llm]`).
    pub store: ConfigStore,
    pub config_path: PathBuf,
    /// Latest runtime detection (`None` while the first probe runs).
    pub runtime: Option<RuntimeReport>,
    pub detecting: bool,
    /// Set by a tab to ask the app to re-run detection.
    pub want_redetect: bool,
    /// The animation set's `manifest.toml` (Poses), if one was found.
    pub manifest: Option<PathBuf>,
    /// Picks to hand to the host (in-process sink / stdout wire lines).
    pub picks_out: Vec<MenuPick>,
    /// Short status messages `(text, colour, until)`.
    pub toasts: Vec<(String, Rgba, f64)>,
    /// Seconds since start (animations).
    pub time: f64,
    /// Set by a tab to switch tabs (id).
    pub goto_tab: Option<String>,
    /// Set by a tab to ask the app to save everything.
    pub want_save: bool,
    /// Set when the active avatar changed: the chrome accent to switch to.
    pub accent_changed: Option<lillith_theme::Accent>,
}

impl Shared {
    pub fn toast(&mut self, text: impl Into<String>, color: Rgba) {
        self.toasts.push((text.into(), color, self.time + 4.0));
    }
}

/// One tab of the settings window.
pub trait SettingsTab {
    /// Stable id (`companion`, `brain`, `access`, `sealed-2`...), used by `--tab`.
    fn id(&self) -> &str;
    /// Label on the carved tab.
    fn title(&self) -> &str;
    /// Rune glyph on the tab (index into `lillith_theme::geom::RUNES`).
    fn rune(&self) -> usize;
    /// Sealed (empty add-in slot): drawn with a seal, dimmed.
    fn sealed(&self) -> bool {
        false
    }
    /// Draws the page.
    fn ui(&mut self, ui: &mut egui::Ui, shared: &mut Shared);
    /// Per-frame work even when not visible (poll background jobs).
    fn tick(&mut self, _ctx: &egui::Context, _shared: &mut Shared) {}
    /// Unsaved changes that live in the tab itself (not in `shared.store`).
    fn pending(&self) -> usize {
        0
    }
    /// Writes the tab's own unsaved changes. Called after `shared.store`
    /// has been saved.
    fn save(&mut self, _shared: &mut Shared) -> Result<(), String> {
        Ok(())
    }
    fn revert(&mut self, _shared: &mut Shared) {}
    /// Sub-page selection from the command line (`--step pick-model`).
    fn deep_link(&mut self, _arg: &str) {}
    /// Whether a background job is running (screenshots wait for these).
    fn busy(&self) -> bool {
        false
    }
}

/// Ordered tabs.
#[derive(Default)]
pub struct TabRegistry {
    pub tabs: Vec<Box<dyn SettingsTab>>,
}

impl TabRegistry {
    /// Companion, Motion, Chat / Brain, Avatar, Icon, Voice, Build, Bots, About,
    /// Access (`[mind.access]`, in the first add-in slot), Pose Builder
    /// (video import, in the second).
    pub fn with_defaults(shared: &Shared) -> Self {
        let mut r = Self::default();
        r.register(Box::new(companion::CompanionTab::new(shared)));
        r.register(Box::new(knobs::KnobTab::motion()));
        r.register(Box::new(brain::BrainTab::new(&shared.config_path)));
        r.register(Box::new(avatar::AvatarTab::new(&shared.config_path)));
        r.register(Box::new(icon::IconTab::new()));
        r.register(Box::new(knobs::KnobTab::voice()));
        r.register(Box::new(build::BuildTab::new(shared)));
        r.register(Box::new(bots::BotsTab::new(shared)));
        r.register(Box::new(about::AboutTab));
        for n in 1..=2 {
            r.register(Box::new(sealed::SealedTab::new(n)));
        }
        // [mind.access] takes the first add-in slot.
        r.unseal(Box::new(knobs::KnobTab::access()));
        // Pose Builder (import your own videos) takes the second.
        r.unseal(Box::new(import::ImportTab::new()));
        r
    }

    pub fn register(&mut self, tab: Box<dyn SettingsTab>) {
        self.tabs.push(tab);
    }

    /// Replaces the first sealed slot with `tab`; pushes it if none is left.
    pub fn unseal(&mut self, tab: Box<dyn SettingsTab>) {
        match self.tabs.iter().position(|t| t.sealed()) {
            Some(i) => self.tabs[i] = tab,
            None => self.tabs.push(tab),
        }
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|t| t.id() == id)
    }

    pub fn ids(&self) -> Vec<String> {
        self.tabs.iter().map(|t| t.id().to_string()).collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn shared() -> Shared {
        let p =
            std::env::temp_dir().join(format!("lillith-settings-reg-{}.toml", std::process::id()));
        Shared {
            store: ConfigStore::load(&p),
            config_path: p,
            runtime: None,
            detecting: false,
            want_redetect: false,
            manifest: None,
            picks_out: Vec::new(),
            toasts: Vec::new(),
            time: 0.0,
            goto_tab: None,
            want_save: false,
            accent_changed: None,
        }
    }

    struct Dummy;
    impl SettingsTab for Dummy {
        fn id(&self) -> &str {
            "dummy"
        }
        fn title(&self) -> &str {
            "Dummy"
        }
        fn rune(&self) -> usize {
            0
        }
        fn ui(&mut self, _: &mut egui::Ui, _: &mut Shared) {}
    }

    #[test]
    fn default_tabs_in_order_and_unseal() {
        let s = shared();
        let mut r = TabRegistry::with_defaults(&s);
        assert_eq!(
            r.ids(),
            [
                "companion",
                "motion",
                "brain",
                "avatar",
                "icon",
                "voice",
                "build",
                "bots",
                "about",
                "access",
                "import"
            ]
        );
        assert_eq!(r.tabs.iter().filter(|t| t.sealed()).count(), 0);
        // No sealed slot left: an add-in is appended.
        r.unseal(Box::new(Dummy));
        assert_eq!(r.index_of("dummy"), Some(11));
    }
}
