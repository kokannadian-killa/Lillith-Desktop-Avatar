//! # lillith-tray
//!
//! StatusNotifierItem tray via `ksni` (replaces `pystray`, PLAN.md M6).
//! Menu: Show/Hide, Mode, Random, Mute, Talk, Status, Voice, Settings,
//! QuantumSec, Quit. Tooltip shows live QuantumSec proton status when set.

use std::sync::{Arc, RwLock};

/// Tray menu actions, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowHide,
    Mode,
    RandomReaction,
    HackingMode,
    Mute,
    Talk,
    Status,
    VoiceListen,
    Settings,
    /// Open / focus QuantumSec (DemonOS shield).
    QuantumSec,
    Quit,
}

impl TrayAction {
    pub const ALL: &'static [TrayAction] = &[
        TrayAction::ShowHide,
        TrayAction::Mode,
        TrayAction::RandomReaction,
        TrayAction::HackingMode,
        TrayAction::Mute,
        TrayAction::Talk,
        TrayAction::Status,
        TrayAction::VoiceListen,
        TrayAction::Settings,
        TrayAction::QuantumSec,
        TrayAction::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TrayAction::ShowHide => "Show / Hide",
            TrayAction::Mode => "Mode",
            TrayAction::RandomReaction => "Random Reaction",
            TrayAction::HackingMode => "Hacking Mode",
            TrayAction::Mute => "Mute",
            TrayAction::Talk => "Talk to Lillith",
            TrayAction::Status => "Status",
            TrayAction::VoiceListen => "Voice Listen",
            TrayAction::Settings => "Settings",
            TrayAction::QuantumSec => "QuantumSec",
            TrayAction::Quit => "Quit",
        }
    }
}

/// Shared QuantumSec one-liner for the tray tooltip (light; updated by the app).
#[derive(Debug, Clone, Default)]
pub struct QuantumSecTooltip(Arc<RwLock<String>>);

impl QuantumSecTooltip {
    /// New empty tooltip store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the tooltip body (e.g. from `status.json` summary).
    pub fn set(&self, text: impl Into<String>) {
        if let Ok(mut g) = self.0.write() {
            *g = text.into();
        }
    }

    /// Current tooltip body.
    pub fn get(&self) -> String {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }
}

/// The tray item.
#[derive(Debug, Default)]
pub struct LillithTray {
    /// Optional live QuantumSec status line for hover tooltip.
    pub qsec: QuantumSecTooltip,
}

impl LillithTray {
    /// Tray with a shared QuantumSec tooltip handle.
    pub fn with_qsec(qsec: QuantumSecTooltip) -> Self {
        Self { qsec }
    }
}

impl ksni::Tray for LillithTray {
    fn id(&self) -> String {
        "lillith".into()
    }

    /// The theme icon the installers put in hicolor; the Icon picker
    /// rewrites it, so the tray follows the chosen icon.
    fn icon_name(&self) -> String {
        "lillith".into()
    }
    fn title(&self) -> String {
        "Lillith · DemonOS".into()
    }

    fn tool_tip(&self) -> ksni::ToolTip {
        let body = self.qsec.get();
        let description = if body.is_empty() {
            "PhantomCore / QuantumSec — local protons idle".into()
        } else {
            body
        };
        ksni::ToolTip {
            title: "Lillith QuantumSec".into(),
            description,
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        TrayAction::ALL
            .iter()
            .map(|a| {
                ksni::menu::StandardItem {
                    label: a.label().into(),
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantumsec_is_in_menu_and_tooltip_roundtrips() {
        assert!(TrayAction::ALL.contains(&TrayAction::QuantumSec));
        let tip = QuantumSecTooltip::new();
        tip.set("STATUS NOMINAL");
        assert_eq!(tip.get(), "STATUS NOMINAL");
        let tray = LillithTray::with_qsec(tip);
        let tt = ksni::Tray::tool_tip(&tray);
        assert!(tt.description.contains("NOMINAL"));
        assert_eq!(ksni::Tray::title(&tray), "Lillith · DemonOS");
    }
}
