//! # lillith-tray
//!
//! StatusNotifierItem tray via `ksni` (replaces `pystray`, PLAN.md M6).
//! Menu: Show/Hide, Mode, Random, Mute, Talk, Status, Voice, Settings, Quit.
//! TODO(M6): wire menu activations to core commands.

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
            TrayAction::Quit => "Quit",
        }
    }
}

/// The tray item.
#[derive(Debug, Default)]
pub struct LillithTray;

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
        "Lillith".into()
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
