//! Seasonal outfits (`assets/seasons.toml`, see `docs/SEASONS.md`): date
//! windows, outfit-menu entries and their clip lists, plus the selection
//! rule behind the `seasonal_outfit` config key.
//!
//! Seasonal outfits are add-only extras: her base outfit and its clips never
//! change. An *active* outfit only adds its own clips to the allure pools;
//! with `off` (or no season today) no seasonal clip plays.
//!
//! ```text
//! active_outfit =
//!   setting == "off"          -> none
//!   setting == <outfit id>    -> that outfit (manual always beats the date)
//!   setting == "auto"         -> outfit of the highest-priority [[season]]
//!                                whose window contains today, else none
//! ```
//!
//! Pure data and date math; the only I/O is [`Seasons::load`] and
//! [`today`] (the local calendar date).

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// `seasonal_outfit = "auto"`: pick by date (the default).
pub const AUTO: &str = "auto";
/// `seasonal_outfit = "off"`: her base outfit only, never a seasonal clip.
pub const OFF: &str = "off";

/// The bundled `assets/seasons.toml` (used when no file sits next to the
/// animation set, e.g. a hand-packed release without it).
pub const BUNDLED: &str = include_str!("../../../assets/seasons.toml");

/// A local calendar day without a year (`"MM-DD"` in seasons.toml).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonthDay {
    pub month: u8,
    pub day: u8,
}

impl MonthDay {
    /// `None` unless `month` is 1..=12 and `day` fits the month (Feb 29 is
    /// allowed: windows have no year).
    pub fn new(month: u8, day: u8) -> Option<Self> {
        const DAYS: [u8; 12] = [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let max = *DAYS.get(usize::from(month).checked_sub(1)?)?;
        (1..=max).contains(&day).then_some(Self { month, day })
    }

    /// Parses `"MM-DD"` (e.g. `"12-31"`; `"1-2"` is accepted too).
    pub fn parse(s: &str) -> Option<Self> {
        let (m, d) = s.trim().split_once('-')?;
        let all_digits =
            |t: &str| !t.is_empty() && t.len() <= 2 && t.bytes().all(|b| b.is_ascii_digit());
        if !all_digits(m) || !all_digits(d) {
            return None;
        }
        Self::new(m.parse().ok()?, d.parse().ok()?)
    }
}

impl std::fmt::Display for MonthDay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:02}-{:02}", self.month, self.day)
    }
}

impl<'de> Deserialize<'de> for MonthDay {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        MonthDay::parse(&s).ok_or_else(|| {
            serde::de::Error::custom(format!("bad month-day {s:?} (want \"MM-DD\")"))
        })
    }
}

/// `[[season]]`: an inclusive local-date window and the outfit it wears.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Season {
    pub id: String,
    #[serde(default)]
    pub name: String,
    pub start: MonthDay,
    /// Inclusive; `end < start` wraps over New Year (Dec 31 → Jan 2).
    pub end: MonthDay,
    /// Highest wins where windows overlap (newyear beats xmas on Dec 31).
    #[serde(default)]
    pub priority: i32,
    /// `[[outfit]].id` worn in this window.
    pub outfit: String,
}

impl Season {
    /// `date` is inside the window (inclusive, wrapping when `end < start`).
    pub fn contains(&self, date: MonthDay) -> bool {
        if self.start <= self.end {
            self.start <= date && date <= self.end
        } else {
            date >= self.start || date <= self.end
        }
    }
}

/// `[[outfit.secret]]`: reserved per-clip hide/unlock (nothing uses it yet).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct OutfitSecret {
    pub clip: String,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub unlock: Option<String>,
}

/// `[[outfit]]`: one selectable outfit.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Outfit {
    pub id: String,
    /// Menu label.
    #[serde(default)]
    pub name: String,
    /// `[[season]].id` it belongs to (its clips carry `season:<id>`).
    #[serde(default)]
    pub season: String,
    /// 128×128 RGBA PNG, path relative to the repo / install root.
    #[serde(default)]
    pub thumbnail: String,
    /// Manifest clip names (missing ones are skipped).
    #[serde(default)]
    pub clips: Vec<String>,
    /// Reserved: kept out of the menu and out of auto mode until `unlock`.
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub unlock: Option<String>,
    #[serde(default)]
    pub secret: Vec<OutfitSecret>,
}

impl Outfit {
    /// Menu label (`name`, else the id).
    pub fn label(&self) -> &str {
        if self.name.trim().is_empty() {
            &self.id
        } else {
            &self.name
        }
    }

    /// At least one of its clips is in the loaded animation set.
    pub fn available(&self, has_clip: impl Fn(&str) -> bool) -> bool {
        self.clips.iter().any(|c| has_clip(c))
    }

    /// Clip `name` (with `tags`) belongs to this outfit: listed in `clips`,
    /// or tagged `outfit:<id>` / `season:<season>`.
    pub fn owns_clip(&self, name: &str, tags: &[String]) -> bool {
        self.clips.iter().any(|c| c == name)
            || tags.iter().any(|t| {
                t.strip_prefix("outfit:") == Some(self.id.as_str())
                    || (!self.season.is_empty()
                        && t.strip_prefix("season:") == Some(self.season.as_str()))
            })
    }
}

/// The parsed `seasons.toml`.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
pub struct Seasons {
    #[serde(default)]
    pub schema: u32,
    #[serde(default, rename = "season")]
    pub seasons: Vec<Season>,
    #[serde(default, rename = "outfit")]
    pub outfits: Vec<Outfit>,
}

/// seasons.toml load errors.
#[derive(Debug, thiserror::Error)]
pub enum SeasonsError {
    #[error("seasons.toml io: {0}")]
    Io(#[from] std::io::Error),
    #[error("seasons.toml parse: {0}")]
    Parse(#[from] toml::de::Error),
}

/// The `seasonal_outfit` setting, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutfitSetting {
    Auto,
    Off,
    Manual(String),
}

impl OutfitSetting {
    /// `"auto"` (or empty) / `"off"` / anything else = an outfit id.
    /// Case-insensitive for the two keywords; surrounding space ignored.
    pub fn parse(s: &str) -> Self {
        let t = s.trim();
        if t.is_empty() || t.eq_ignore_ascii_case(AUTO) {
            OutfitSetting::Auto
        } else if t.eq_ignore_ascii_case(OFF) {
            OutfitSetting::Off
        } else {
            OutfitSetting::Manual(t.to_string())
        }
    }

    /// The config value (`auto` / `off` / the id).
    pub fn key(&self) -> &str {
        match self {
            OutfitSetting::Auto => AUTO,
            OutfitSetting::Off => OFF,
            OutfitSetting::Manual(id) => id,
        }
    }
}

impl Seasons {
    pub fn from_toml_str(s: &str) -> Result<Self, SeasonsError> {
        Ok(toml::from_str(s)?)
    }

    /// The bundled copy of `assets/seasons.toml`.
    pub fn bundled() -> Self {
        Self::from_toml_str(BUNDLED).unwrap_or_else(|e| {
            tracing::warn!("bundled seasons.toml: {e}");
            Self::default()
        })
    }

    /// Reads `path`.
    pub fn load(path: &Path) -> Result<Self, SeasonsError> {
        Self::from_toml_str(&std::fs::read_to_string(path)?)
    }

    /// `path` if it is there and parses; a missing file means the bundled
    /// copy; a bad one logs a warning and means no seasonal outfits at all
    /// (her base outfit only).
    pub fn load_or_bundled(path: Option<&Path>) -> Self {
        let Some(path) = path else {
            return Self::bundled();
        };
        match Self::load(path) {
            Ok(s) => s,
            Err(SeasonsError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::debug!("{}: not found; using the bundled seasons", path.display());
                Self::bundled()
            }
            Err(e) => {
                tracing::warn!("{}: {e}; seasonal outfits off", path.display());
                Self::default()
            }
        }
    }

    /// `seasons.toml` for an animation set: `assets/seasons.toml` next to
    /// `assets/generated/manifest.toml` (the manifest's grandparent dir).
    pub fn path_for_manifest(manifest: &Path) -> Option<PathBuf> {
        Some(manifest.parent()?.parent()?.join("seasons.toml"))
    }

    pub fn outfit(&self, id: &str) -> Option<&Outfit> {
        self.outfits.iter().find(|o| o.id == id)
    }

    /// Outfits the menu lists (not `hidden`), in file order.
    pub fn listed(&self) -> impl Iterator<Item = &Outfit> {
        self.outfits.iter().filter(|o| !o.hidden)
    }

    /// The season whose window contains `date` (highest `priority`; the
    /// first in file order on a tie).
    pub fn season_on(&self, date: MonthDay) -> Option<&Season> {
        let mut best: Option<&Season> = None;
        for s in self.seasons.iter().filter(|s| s.contains(date)) {
            if best.is_none_or(|b| s.priority > b.priority) {
                best = Some(s);
            }
        }
        best
    }

    /// The outfit to wear on `date` for `setting` (see the module docs).
    /// Unknown ids (a manual pick or a season's `outfit`) and hidden outfits
    /// fall back to none, with a warning.
    pub fn active_outfit(&self, date: MonthDay, setting: &str) -> Option<&Outfit> {
        match OutfitSetting::parse(setting) {
            OutfitSetting::Off => None,
            OutfitSetting::Manual(id) => match self.outfit(&id) {
                Some(o) if !o.hidden => Some(o),
                Some(_) => {
                    tracing::warn!("seasonal_outfit {id:?} is locked; base outfit");
                    None
                }
                None => {
                    tracing::warn!("seasonal_outfit {id:?}: no such outfit; base outfit");
                    None
                }
            },
            OutfitSetting::Auto => {
                let season = self.season_on(date)?;
                match self.outfit(&season.outfit) {
                    Some(o) if !o.hidden => Some(o),
                    Some(_) => None,
                    None => {
                        tracing::warn!(
                            "season {:?} wears unknown outfit {:?}; base outfit",
                            season.id,
                            season.outfit
                        );
                        None
                    }
                }
            }
        }
    }
}

/// Today's local calendar date (falls back to UTC if the local time can't
/// be read).
pub fn today() -> MonthDay {
    local_today().unwrap_or_else(|| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        month_day_of_unix(secs)
    })
}

/// The UTC month-day of a unix time (civil-from-days, Howard Hinnant).
pub fn month_day_of_unix(secs: i64) -> MonthDay {
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u8;
    MonthDay { month, day }
}

#[cfg(unix)]
fn local_today() -> Option<MonthDay> {
    // SAFETY: `time(NULL)` and `localtime_r` with valid pointers to locals.
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return None;
        }
        MonthDay::new((tm.tm_mon + 1) as u8, tm.tm_mday as u8)
    }
}

#[cfg(windows)]
fn local_today() -> Option<MonthDay> {
    // SAFETY: GetLocalTime only writes the SYSTEMTIME we pass.
    let st = unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime() };
    MonthDay::new(st.wMonth as u8, st.wDay as u8)
}

#[cfg(not(any(unix, windows)))]
fn local_today() -> Option<MonthDay> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(m: u8, d: u8) -> MonthDay {
        MonthDay::new(m, d).unwrap()
    }

    fn id(o: Option<&Outfit>) -> Option<&str> {
        o.map(|o| o.id.as_str())
    }

    #[test]
    fn bundled_file_parses_with_six_seasons_and_outfits() {
        let s = Seasons::bundled();
        assert_eq!(s.schema, 1);
        assert_eq!(s.seasons.len(), 6);
        assert_eq!(s.outfits.len(), 6);
        // Every season wears a known outfit.
        for season in &s.seasons {
            assert!(s.outfit(&season.outfit).is_some(), "{season:?}");
        }
        let h = s.outfit("halloween").unwrap();
        assert_eq!(h.clips.len(), 4);
        assert!(h.thumbnail.ends_with("outfits/halloween.png"));
        assert_eq!(h.label(), "Micro Witch (Halloween)");
    }

    #[test]
    fn doc_dates() {
        // docs/SEASONS.md: Dec 31 (newyear wins), Jan 2 (newyear),
        // Oct 31 (halloween), Nov 3 (none), a manual id in July.
        let s = Seasons::bundled();
        assert_eq!(id(s.active_outfit(md(12, 31), AUTO)), Some("newyear"));
        assert_eq!(id(s.active_outfit(md(1, 2), AUTO)), Some("newyear"));
        assert_eq!(id(s.active_outfit(md(10, 31), AUTO)), Some("halloween"));
        assert_eq!(id(s.active_outfit(md(11, 3), AUTO)), None);
        assert_eq!(
            id(s.active_outfit(md(7, 15), "halloween")),
            Some("halloween")
        );
        assert_eq!(id(s.active_outfit(md(7, 15), AUTO)), Some("summer"));
    }

    #[test]
    fn window_edges_are_inclusive() {
        let s = Seasons::bundled();
        assert_eq!(id(s.active_outfit(md(9, 30), AUTO)), None);
        assert_eq!(id(s.active_outfit(md(10, 1), AUTO)), Some("halloween"));
        assert_eq!(id(s.active_outfit(md(11, 2), AUTO)), Some("halloween"));
        assert_eq!(id(s.active_outfit(md(12, 1), AUTO)), Some("xmas"));
        assert_eq!(id(s.active_outfit(md(12, 30), AUTO)), Some("xmas"));
        assert_eq!(id(s.active_outfit(md(1, 3), AUTO)), None);
        assert_eq!(id(s.active_outfit(md(2, 6), AUTO)), None);
        assert_eq!(id(s.active_outfit(md(2, 7), AUTO)), Some("valentines"));
        assert_eq!(id(s.active_outfit(md(2, 15), AUTO)), Some("valentines"));
        assert_eq!(id(s.active_outfit(md(3, 19), AUTO)), None);
        assert_eq!(id(s.active_outfit(md(3, 20), AUTO)), Some("spring"));
        assert_eq!(id(s.active_outfit(md(8, 31), AUTO)), Some("summer"));
        assert_eq!(id(s.active_outfit(md(9, 1), AUTO)), None);
    }

    #[test]
    fn windows_wrap_the_year_end() {
        let s = Seasons::from_toml_str(
            r#"
[[season]]
id = "winter"
start = "12-20"
end = "01-05"
outfit = "w"
[[outfit]]
id = "w"
"#,
        )
        .unwrap();
        for (m, d, on) in [
            (12, 19, false),
            (12, 20, true),
            (12, 31, true),
            (1, 1, true),
            (1, 5, true),
            (1, 6, false),
            (6, 1, false),
        ] {
            assert_eq!(
                s.active_outfit(md(m, d), AUTO).is_some(),
                on,
                "{m:02}-{d:02}"
            );
        }
    }

    #[test]
    fn off_manual_and_unknown_ids() {
        let s = Seasons::bundled();
        assert_eq!(id(s.active_outfit(md(10, 31), OFF)), None);
        assert_eq!(id(s.active_outfit(md(10, 31), " OFF ")), None);
        // Manual beats the date, in and out of its window.
        assert_eq!(id(s.active_outfit(md(10, 31), "xmas")), Some("xmas"));
        assert_eq!(
            id(s.active_outfit(md(4, 1), "halloween")),
            Some("halloween")
        );
        // Empty = auto.
        assert_eq!(id(s.active_outfit(md(10, 31), "")), Some("halloween"));
        // Unknown manual id: none (warning), never a panic or the date's.
        assert_eq!(id(s.active_outfit(md(10, 31), "pirate")), None);
        // A season pointing at an unknown outfit: none.
        let bad = Seasons::from_toml_str(
            "[[season]]\nid = \"x\"\nstart = \"01-01\"\nend = \"12-31\"\noutfit = \"ghost\"\n",
        )
        .unwrap();
        assert_eq!(id(bad.active_outfit(md(5, 5), AUTO)), None);
        assert_eq!(OutfitSetting::parse("Auto"), OutfitSetting::Auto);
        assert_eq!(OutfitSetting::parse("x").key(), "x");
    }

    #[test]
    fn hidden_outfits_stay_out_of_menu_and_auto() {
        let s = Seasons::from_toml_str(
            r#"
[[season]]
id = "s"
start = "01-01"
end = "12-31"
outfit = "secret"
[[outfit]]
id = "secret"
hidden = true
unlock = "secret:oops_retie"
[[outfit]]
id = "open"
clips = ["a"]
[[outfit.secret]]
clip = "a"
hidden = true
"#,
        )
        .unwrap();
        assert_eq!(id(s.active_outfit(md(5, 5), AUTO)), None);
        assert_eq!(id(s.active_outfit(md(5, 5), "secret")), None);
        let listed: Vec<&str> = s.listed().map(|o| o.id.as_str()).collect();
        assert_eq!(listed, ["open"]);
        assert_eq!(s.outfit("open").unwrap().secret.len(), 1);
    }

    #[test]
    fn priority_breaks_overlaps_then_file_order() {
        let s = Seasons::from_toml_str(
            r#"
[[season]]
id = "a"
start = "06-01"
end = "06-30"
outfit = "a"
[[season]]
id = "b"
start = "06-10"
end = "06-12"
outfit = "b"
priority = 3
[[season]]
id = "c"
start = "06-01"
end = "06-30"
outfit = "c"
[[outfit]]
id = "a"
[[outfit]]
id = "b"
[[outfit]]
id = "c"
"#,
        )
        .unwrap();
        assert_eq!(id(s.active_outfit(md(6, 11), AUTO)), Some("b"));
        assert_eq!(id(s.active_outfit(md(6, 20), AUTO)), Some("a"));
    }

    #[test]
    fn missing_and_bad_files() {
        let dir = std::env::temp_dir().join(format!("lillith-seasons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Missing: an error from `load`, the bundled set from `load_or_bundled`.
        let missing = dir.join("nope.toml");
        assert!(matches!(Seasons::load(&missing), Err(SeasonsError::Io(_))));
        assert_eq!(Seasons::load_or_bundled(Some(&missing)), Seasons::bundled());
        // Bad toml / bad dates: an error, and no seasonal outfits at all.
        for text in [
            "[[season]\nid = 1",
            "[[season]]\nid = \"x\"\nstart = \"13-01\"\nend = \"01-01\"\noutfit = \"x\"\n",
            "[[season]]\nid = \"x\"\nstart = \"02-30\"\nend = \"01-01\"\noutfit = \"x\"\n",
            "[[season]]\nid = \"x\"\nstart = \"Oct 1\"\nend = \"01-01\"\noutfit = \"x\"\n",
        ] {
            let p = dir.join("bad.toml");
            std::fs::write(&p, text).unwrap();
            assert!(
                matches!(Seasons::load(&p), Err(SeasonsError::Parse(_))),
                "{text}"
            );
            let s = Seasons::load_or_bundled(Some(&p));
            assert_eq!(s, Seasons::default());
            assert_eq!(s.active_outfit(md(10, 31), AUTO), None);
            assert_eq!(s.active_outfit(md(10, 31), "halloween"), None);
        }
        // An empty file is valid: no seasons.
        let p = dir.join("empty.toml");
        std::fs::write(&p, "").unwrap();
        assert_eq!(Seasons::load(&p).unwrap(), Seasons::default());
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            Seasons::path_for_manifest(Path::new("/x/assets/generated/manifest.toml")),
            Some(PathBuf::from("/x/assets/seasons.toml"))
        );
    }

    #[test]
    fn month_days() {
        assert_eq!(MonthDay::parse("12-31"), Some(md(12, 31)));
        assert_eq!(MonthDay::parse("1-2"), Some(md(1, 2)));
        assert_eq!(MonthDay::parse("02-29"), Some(md(2, 29)));
        for bad in [
            "00-01", "01-00", "04-31", "1231", "12-", "-1", "a-b", "+1-01", "123-1",
        ] {
            assert_eq!(MonthDay::parse(bad), None, "{bad}");
        }
        assert_eq!(md(3, 4).to_string(), "03-04");
        assert_eq!(month_day_of_unix(0), md(1, 1));
        // 2026-10-24T12:00:00Z
        assert_eq!(month_day_of_unix(1_792_843_200), md(10, 24));
        // 2024-02-29T00:00:00Z
        assert_eq!(month_day_of_unix(1_709_164_800), md(2, 29));
        let t = today();
        assert!(MonthDay::new(t.month, t.day).is_some());
    }

    #[test]
    fn outfit_owns_its_clips() {
        let s = Seasons::bundled();
        let h = s.outfit("halloween").unwrap();
        let tags = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(h.owns_clip("halloween_hat_tip_wink", &[]));
        assert!(h.owns_clip("x", &tags(&["seasonal", "outfit:halloween"])));
        assert!(h.owns_clip("x", &tags(&["seasonal", "season:halloween"])));
        assert!(!h.owns_clip("xmas_a", &tags(&["seasonal", "outfit:xmas", "season:xmas"])));
        assert!(h.available(|c| c == "halloween_cape_swirl_arch"));
        assert!(!s.outfit("xmas").unwrap().available(|_| true));
    }
}
