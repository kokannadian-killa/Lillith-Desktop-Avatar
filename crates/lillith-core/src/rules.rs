//! Data-driven event → reaction rules (`assets/rules.toml`, PLAN.md §3.2).

use crate::event::Event;
use crate::mood::Mood;
use serde::Deserialize;
use std::collections::BTreeMap;

/// Pose names that exist in the curated art (`assets/source/poses/NN_<name>.*`).
pub const KNOWN_POSES: &[&str] = &[
    "browser_history",
    "connection_lost",
    "calendar",
    "crop",
    "dual_tail",
    "bluetooth",
    "bookmark",
    "screenshot",
    "minimize",
    "hacking",
    "scheduler",
    "waiting",
    "battery_low",
    "code_inspect",
    "unknown_back_view_tail",
    "recycle",
    "loading",
    "hover_sit_on_window",
    "brightness",
    "multi_select",
    "printing",
    "cloud_sync",
    "vpn",
    "drag_drop",
    "running",
    "clipboard",
    "volume",
    "shortcut_menu",
    "knife",
    "htop",
    "hotspot",
    "firewall",
    "dismiss_notif",
    "cleaning",
    "sleep",
    "magnify",
    "start_menu",
    "diagnostics",
    "private",
    "busy",
    "wifi",
    "unknown_peek_over_laptop",
    "sitting",
    "error",
    "compress",
    "peeking",
    "help",
    "unknown_net_files_terminal",
    "drive_eject",
    "peeking_folder",
    "tools",
    "alert",
];

/// Event reaction clips that exist in the canonical generated set
/// (`assets/generated/manifest.json`, clips named `event_<name>`). Mirrors
/// the manifest the same way [`KNOWN_POSES`] mirrors the pose art, so a typo
/// in `[rule_clips]` fails `cargo test`.
pub const KNOWN_EVENT_CLIPS: &[&str] = &[
    "event_approval_needed",
    "event_archiving",
    "event_battery_low",
    "event_bluetooth",
    "event_brightness",
    "event_browser_bookmark",
    "event_browser_history",
    "event_calendar",
    "event_clipboard",
    "event_clipboard_cut",
    "event_cloud_sync",
    "event_cpu_busy",
    "event_diagnostics",
    "event_error",
    "event_firewall_block",
    "event_help",
    "event_hotspot_on",
    "event_incognito",
    "event_input_click_double",
    "event_input_click_right",
    "event_input_hover_long",
    "event_loading",
    "event_net_lost",
    "event_notification",
    "event_printing",
    "event_ram_high",
    "event_recycle_bin",
    "event_screenshot",
    "event_search_opened",
    "event_start_menu_opened",
    "event_usb_added",
    "event_usb_eject",
    "event_volume",
    "event_vpn_up",
    "event_wifi",
    "event_window_close",
    "event_window_minimize",
    "event_window_open",
];

/// Regular (non-event) animations in the canonical generated set that rules
/// may also point at via `[rule_clips]`.
pub const KNOWN_BASE_CLIPS: &[&str] = &[
    "beam_cast",
    "blink_wink",
    "dance",
    "drag_held",
    "facepalm",
    "hip_sway",
    "idle",
    "knife_tap",
    "lean_forward",
    "nod",
    "point",
    "run_left",
    "run_right",
    "salute",
    "shoulder_roll",
    "shrug",
    "sit_swing",
    "sleep",
    "spell_orb_cast",
    "tada_celebrate",
    "tail_wag",
    "think",
    "wake",
    "walk_left",
    "walk_right",
    "wave",
];

/// `true` if `clip` is a known clip of the generated set (event or regular).
pub fn is_known_clip(clip: &str) -> bool {
    KNOWN_EVENT_CLIPS.contains(&clip) || KNOWN_BASE_CLIPS.contains(&clip)
}

/// Errors loading the rule table.
#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("invalid rules.toml: {0}")]
    Parse(#[from] toml::de::Error),
}

/// One `[[rule]]` row.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Rule {
    pub event: String,
    pub pose: String,
    #[serde(default)]
    pub then: Option<String>,
    #[serde(default)]
    pub gesture: Option<String>,
    #[serde(default)]
    pub line: Option<String>,
    #[serde(default)]
    pub mood: Option<Mood>,
    #[serde(default)]
    pub sfx: Option<String>,
    #[serde(default)]
    pub priority: u8,
    #[serde(default)]
    pub cooldown_s: f32,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl Rule {
    /// The bubble line for `event`, with `{summary}` replaced by its detail.
    pub fn line_for(&self, event: &Event) -> Option<String> {
        let line = self.line.as_ref()?;
        Some(line.replace("{summary}", event.detail.as_deref().unwrap_or("")))
    }
}

/// A key binding: either a pose or a named action.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum KeyBinding {
    Pose { pose: String },
    Action { action: String },
}

/// The whole rules file.
#[derive(Debug, Clone, Deserialize)]
pub struct RuleSet {
    #[serde(default)]
    pub thresholds: BTreeMap<String, f64>,
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
    #[serde(default)]
    pub keys: BTreeMap<String, KeyBinding>,
    /// Rule event name -> clip (`[rule_clips]`, formerly `[event_clips]`):
    /// an `event_<name>` reaction or any regular animation. Optional; rules
    /// without an entry fall back to their `pose`.
    #[serde(default, alias = "event_clips")]
    pub rule_clips: BTreeMap<String, String>,
}

impl RuleSet {
    /// Parses `rules.toml` content.
    pub fn from_toml_str(s: &str) -> Result<Self, RulesError> {
        Ok(toml::from_str(s)?)
    }

    /// The bundled default rules (`assets/rules.toml`).
    pub fn bundled() -> Self {
        Self::from_toml_str(include_str!("../../../assets/rules.toml"))
            .expect("bundled assets/rules.toml is valid (checked by tests)")
    }

    /// First enabled rule for an event name.
    pub fn find(&self, event: &str) -> Option<&Rule> {
        self.rules.iter().find(|r| r.enabled && r.event == event)
    }

    /// Clip (event reaction or regular animation) for a fired rule event,
    /// if one is mapped.
    pub fn clip_for(&self, event: &str) -> Option<&str> {
        self.rule_clips.get(event).map(String::as_str)
    }

    /// Same as [`RuleSet::clip_for`] (kept for existing callers).
    pub fn event_clip(&self, event: &str) -> Option<&str> {
        self.clip_for(event)
    }

    /// Enabled rules that have no clip mapped.
    pub fn rules_without_clip(&self) -> impl Iterator<Item = &Rule> {
        self.rules
            .iter()
            .filter(|r| r.enabled && !self.rule_clips.contains_key(&r.event))
    }

    /// Clips from `known` that no rule maps to.
    pub fn clips_without_rule<'a>(&self, known: &[&'a str]) -> Vec<&'a str> {
        known
            .iter()
            .copied()
            .filter(|c| !self.rule_clips.values().any(|v| v == c))
            .collect()
    }

    /// A numeric threshold by key.
    pub fn threshold(&self, key: &str) -> Option<f64> {
        self.thresholds.get(key).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn bundled_rules_parse() {
        let rs = RuleSet::bundled();
        assert!(rs.rules.len() >= 50, "only {} rules", rs.rules.len());
        let cpu = rs.find("cpu.high").expect("cpu.high rule");
        assert_eq!(cpu.pose, "busy");
        assert_eq!(cpu.then.as_deref(), Some("alert"));
        assert_eq!(cpu.mood, Some(Mood::Alert));
        assert_eq!(rs.threshold("battery_low_percent"), Some(15.0));
    }

    #[test]
    fn every_pose_exists_in_art() {
        let known: HashSet<&str> = KNOWN_POSES.iter().copied().collect();
        let rs = RuleSet::bundled();
        for r in &rs.rules {
            assert!(
                known.contains(r.pose.as_str()),
                "{}: unknown pose {}",
                r.event,
                r.pose
            );
            if let Some(t) = &r.then {
                assert!(
                    known.contains(t.as_str()),
                    "{}: unknown then-pose {t}",
                    r.event
                );
            }
        }
        for (k, b) in &rs.keys {
            if let KeyBinding::Pose { pose } = b {
                assert!(
                    known.contains(pose.as_str()),
                    "key {k}: unknown pose {pose}"
                );
            }
        }
    }

    #[test]
    fn event_names_are_unique() {
        let rs = RuleSet::bundled();
        let mut seen = HashSet::new();
        for r in &rs.rules {
            assert!(seen.insert(r.event.as_str()), "duplicate rule {}", r.event);
        }
    }

    /// (rule event, clip) pairs that must resolve.
    const MAPPED: &[(&str, &str)] = &[
        ("cpu.high", "event_cpu_busy"),
        ("system.status", "event_diagnostics"),
        ("battery.low", "event_battery_low"),
        ("brightness.changed", "event_brightness"),
        ("wifi.changed", "event_wifi"),
        ("net.restored", "event_wifi"),
        ("bt.paired", "event_bluetooth"),
        ("firewall.blocked", "event_firewall_block"),
        ("volume.changed", "event_volume"),
        ("usb.removed", "event_usb_eject"),
        ("print.job", "event_printing"),
        ("archive.created", "event_archiving"),
        ("cloud.sync", "event_cloud_sync"),
        ("trash.added", "event_recycle_bin"),
        ("screenshot.taken", "event_screenshot"),
        ("clipboard.copied", "event_clipboard"),
        ("notify.received", "event_notification"),
        ("calendar.reminder", "event_calendar"),
        ("window.opened.app", "event_window_open"),
        ("window.closed", "event_window_close"),
        ("window.minimized", "event_window_minimize"),
        ("window.private", "event_incognito"),
        ("app.loading", "event_loading"),
        ("journal.error", "event_error"),
        ("app.crash", "event_error"),
        ("help.requested", "event_help"),
        ("input.click.middle", "event_help"),
        // Regular animations.
        ("idle.asleep", "sleep"),
        ("input.drag", "drag_held"),
        ("input.click.left", "knife_tap"),
        ("llm.thinking", "think"),
        ("llm.done", "tada_celebrate"),
        ("idle.long", "sit_swing"),
        ("input.hover", "sit_swing"),
        ("focus.long_work", "shoulder_roll"),
        ("user.returned", "wake"),
        ("input.secret", "tail_wag"),
        // Low-confidence stand-ins.
        ("update.available", "event_notification"),
        ("timer.fired", "event_calendar"),
        ("window.opened.folder", "event_window_open"),
        ("window.opened.browser", "event_window_open"),
        ("window.opened.terminal", "event_window_open"),
        ("window.opened.media", "event_window_open"),
        // Dedicated event clips (batch 3).
        ("ram.high", "event_ram_high"),
        ("net.lost", "event_net_lost"),
        ("vpn.up", "event_vpn_up"),
        ("hotspot.on", "event_hotspot_on"),
        ("usb.added", "event_usb_added"),
        ("clipboard.cut", "event_clipboard_cut"),
        ("start_menu.opened", "event_start_menu_opened"),
        ("search.opened", "event_search_opened"),
        ("browser.bookmark", "event_browser_bookmark"),
        ("browser.history", "event_browser_history"),
        ("approval.needed", "event_approval_needed"),
        ("input.click.double", "event_input_click_double"),
        ("input.click.right", "event_input_click_right"),
        ("input.hover_long", "event_input_hover_long"),
    ];

    /// True art gaps: rules deliberately left without a clip (see
    /// docs/EVENT_MAPPING.md). Empty since batch 3; keep the list so a new
    /// rule without art has to be acknowledged here.
    const UNMAPPED: &[&str] = &[];

    #[test]
    fn mapped_events_resolve_to_clips() {
        let rs = RuleSet::bundled();
        for (ev, clip) in MAPPED {
            assert_eq!(rs.event_clip(ev), Some(*clip), "{ev}");
        }
        assert_eq!(
            rs.rule_clips.len(),
            MAPPED.len(),
            "unexpected extra mapping"
        );
    }

    #[test]
    fn unmapped_events_have_no_clip() {
        let rs = RuleSet::bundled();
        for ev in UNMAPPED {
            assert!(rs.find(ev).is_some(), "{ev} is not a rule");
            assert_eq!(rs.event_clip(ev), None, "{ev}");
        }
        let gaps: Vec<&str> = rs.rules_without_clip().map(|r| r.event.as_str()).collect();
        assert_eq!(gaps, UNMAPPED);
        assert_eq!(rs.event_clip("no.such.event"), None);
        assert_eq!(MAPPED.len() + UNMAPPED.len(), rs.rules.len());
    }

    #[test]
    fn event_clip_table_is_consistent() {
        let rs = RuleSet::bundled();
        for (ev, clip) in &rs.rule_clips {
            assert!(rs.find(ev).is_some(), "[rule_clips] key {ev} is not a rule");
            assert!(
                is_known_clip(clip),
                "[rule_clips] {ev}: unknown clip {clip}"
            );
        }
        // Every generated event clip is reachable from some rule.
        assert!(rs.clips_without_rule(KNOWN_EVENT_CLIPS).is_empty());
    }

    #[test]
    fn rule_clips_table_is_optional() {
        let rs =
            RuleSet::from_toml_str("[[rule]]\nevent = \"cpu.high\"\npose = \"busy\"\n").unwrap();
        assert!(rs.rule_clips.is_empty());
        assert_eq!(rs.event_clip("cpu.high"), None);
        assert_eq!(rs.rules_without_clip().count(), 1);
        assert_eq!(
            rs.clips_without_rule(&["event_cpu_busy"]),
            ["event_cpu_busy"]
        );
    }

    #[test]
    fn counts_and_aliases() {
        let rs = RuleSet::bundled();
        assert_eq!(rs.rules.len(), 57);
        assert_eq!(MAPPED.len(), 57);
        assert_eq!(UNMAPPED.len(), 0);
        assert_eq!(rs.rules_without_clip().count(), 0);
        assert_eq!(KNOWN_EVENT_CLIPS.len(), 38);
        // Dedicated art replaced the semantically wrong reuses.
        assert_eq!(rs.clip_for("ram.high"), Some("event_ram_high"));
        assert_eq!(rs.clip_for("usb.added"), Some("event_usb_added"));
        // window.closed unchanged pending a tone decision.
        assert_eq!(rs.clip_for("window.closed"), Some("event_window_close"));
        // Old table name still parses.
        let old = RuleSet::from_toml_str(
            "[[rule]]\nevent = \"cpu.high\"\npose = \"busy\"\n[event_clips]\n\"cpu.high\" = \"event_cpu_busy\"\n",
        )
        .unwrap();
        assert_eq!(old.event_clip("cpu.high"), Some("event_cpu_busy"));
        assert!(is_known_clip("sleep") && is_known_clip("event_wifi"));
        assert!(is_known_clip("event_ram_high"));
        assert!(!is_known_clip("event_no_such_clip"));
    }

    #[test]
    fn summary_substitution() {
        let rs = RuleSet::bundled();
        let rule = rs.find("notify.received").unwrap();
        let ev = Event::with_detail("notify.received", "New mail");
        assert_eq!(rule.line_for(&ev).as_deref(), Some("New mail"));
    }
}
