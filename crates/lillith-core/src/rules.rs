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

    #[test]
    fn summary_substitution() {
        let rs = RuleSet::bundled();
        let rule = rs.find("notify.received").unwrap();
        let ev = Event::with_detail("notify.received", "New mail");
        assert_eq!(rule.line_for(&ev).as_deref(), Some("New mail"));
    }
}
