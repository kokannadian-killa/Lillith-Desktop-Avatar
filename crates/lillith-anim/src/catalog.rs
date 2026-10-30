//! Pose catalogue for the right-click menu: the loaded manifest's clips
//! grouped into show-and-tell categories, and the "Show off" sequence.

use crate::atlas::Manifest;
use lillith_core::seasons::Outfit;
use std::collections::BTreeSet;

/// Manifest tag of a seasonal-outfit clip (`assets/seasons.toml`,
/// docs/SEASONS.md). Such clips are allure poses that only show while
/// their outfit is worn.
pub const SEASONAL_TAG: &str = "seasonal";

/// Menu categories, in display order.
pub const CATEGORIES: [&str; 6] = [
    "Flirty / Allure",
    "Idle habits",
    "Gestures",
    "Dance & fun",
    "Events / reactions",
    "Talk",
];

const ALLURE: &[&str] = &[
    "hip_pop_wink",
    "blow_kiss",
    "lean_tease",
    "hair_flip",
    "tail_tease",
    "over_shoulder_look",
    "hands_behind_head_pose",
    "finger_wag_tease",
    "hair_twirl",
    "cheeky_tongue_wink",
    "stretch_arch",
    "heart_hands",
    "shoulder_shimmy",
    "hip_sway",
    "lean_forward",
    "blink_wink",
    "strut_right",
    "strut_left",
    // Bold set (generated_v2, approved): MP4-sourced.
    "body_roll_slow",
    "arch_back_over_shoulder",
    "kneel_hands_on_thighs",
    "wall_lean_invisible",
    "blow_kiss_hip_pop",
    "hair_lift_stretch",
    "slow_turn_spin_tease",
    "bend_forward_wink",
    // 15 s variants: same moves with a ~10 s living peak hold.
    "body_roll_slow_long",
    "arch_back_over_shoulder_long",
    "kneel_hands_on_thighs_long",
    "wall_lean_invisible_long",
    "blow_kiss_hip_pop_long",
    "hair_lift_stretch_long",
    "slow_turn_spin_tease_long",
    "bend_forward_wink_long",
    // Dominant-persona set (tag `dom`): expression/gaze/gesture aimed at the viewer.
    "dom_beckon_finger",
    "dom_point_down",
    "dom_arms_crossed_demand",
    "smug_smirk",
    "hungry_lip_lick",
    "predator_lean_in",
    "tail_wrap_pull",
    "kiss_then_come_here",
    "soul_contract_sign",
    "kneel_pout_beg",
    "hug_screen_possessive",
    "seduce_beckon_finger",
    "seduce_lip_bite_glance",
    "seduce_lean_to_screen_whisper",
    "seduce_blow_kiss_screen",
    "seduce_tap_glass",
    // Bold allure set (batch 17): hip/body motion + gaze/gesture, canonical outfit.
    "hip_sway_slow",
    "come_hither_crawl",
    "taskbar_lounge_lie",
    "kneel_sit_back_beckon",
    "dom_throne_lean",
    // Bold allure set (batch 18): gaze, gesture, hair/tail play, canonical outfit.
    "leg_cross_slow_sit",
    "hair_twirl_lip_bite",
    "tail_heart_wink",
    "trace_heart_on_glass",
    "dom_chin_tilt_summon",
    // Bold allure set (batch 19): lean/smoulder, glass play, dom order, canonical outfit.
    "lean_doorframe_smolder",
    "kiss_mark_on_glass",
    "dom_snap_kneel_order",
    "finger_walk_on_glass",
    // Bold allure set (batch 20): body roll, seated chin-rest wink, dom whisper, hip-pop beckon.
    "body_roll_to_viewer",
    "chin_rest_slow_wink",
    "dom_screen_edge_whisper",
    "hip_pop_beckon",
];
const FUN: &[&str] = &[
    "dance",
    "sway_dance",
    "twirl",
    "tiptoe_bounce",
    "bounce_giggle",
    "victory_jump",
    "tada_celebrate",
    "peace_sign_pose",
    "tail_wag",
    "spell_orb_cast",
    "beam_cast",
];
const IDLE: &[&str] = &["idle", "sit_swing", "sleep", "wake"];

/// Clips that are not poses: plumbing (blends, drag, locomotion) and the
/// baked-jiggle siblings (drawn automatically in jiggle mode).
pub fn is_pose(name: &str, m: &Manifest) -> bool {
    let Some(c) = m.clips.get(name) else {
        return false;
    };
    c.jiggle_of.is_none()
        && !name.ends_with("_jiggle")
        && !name.starts_with("blend_")
        && !name.starts_with("walk_")
        && !name.starts_with("run_")
        && !name.starts_with("stalk_prowl_")
        && name != "drag_held"
}

/// Base loops the pose cycle never picks (she returns to them).
const NOT_IDLE_PICKS: &[&str] = &[
    "idle",
    "idle_a",
    "idle_b",
    "sleep",
    "wake",
    "sit_swing",
    "perch_sit",
];

/// The loneliness picker's pool: every pose with its manifest
/// [`severity`](crate::atlas::Clip::severity). System-interaction clips
/// (`system = true`, `event_*`, talk loops) are kept but flagged so they are
/// never picked; base loops and plumbing are left out.
pub fn pose_tags(m: &Manifest) -> Vec<lillith_core::loneliness::PoseTag> {
    m.clips
        .iter()
        .filter(|(name, _)| is_pose(name, m) && !NOT_IDLE_PICKS.contains(&name.as_str()))
        .map(|(name, c)| {
            let system = c.system || matches!(category_of(name), 4 | 5);
            lillith_core::loneliness::PoseTag::new(name.clone(), c.severity, system)
        })
        .collect()
}

/// Category index (into [`CATEGORIES`]) for a clip name.
pub fn category_of(name: &str) -> usize {
    if name.starts_with("talk") {
        5
    } else if name.starts_with("event_") {
        4
    } else if name.starts_with("idle_flirt") || ALLURE.contains(&name) {
        0
    } else if FUN.contains(&name) {
        3
    } else if IDLE.contains(&name) {
        1
    } else {
        2
    }
}

/// A seasonal-outfit clip: tagged [`SEASONAL_TAG`] (or `season:<id>` /
/// `outfit:<id>`).
pub fn is_seasonal(name: &str, m: &Manifest) -> bool {
    m.clips.get(name).is_some_and(|c| {
        c.tags
            .iter()
            .any(|t| t == SEASONAL_TAG || t.starts_with("season:") || t.starts_with("outfit:"))
    })
}

/// [`category_of`], with the manifest's tags: seasonal clips are
/// Flirty / Allure (they would land in Gestures by name).
pub fn category_in(name: &str, m: &Manifest) -> usize {
    if is_seasonal(name, m) {
        0
    } else {
        category_of(name)
    }
}

/// A clip may show / play now: any base clip, or a seasonal clip of the
/// outfit worn (`active`; `None` = base outfit, no seasonal clip at all).
pub fn seasonal_visible(name: &str, m: &Manifest, active: Option<&Outfit>) -> bool {
    if !is_seasonal(name, m) {
        return true;
    }
    match (active, m.clips.get(name)) {
        (Some(o), Some(c)) => o.owns_clip(name, &c.tags),
        _ => false,
    }
}

/// The outfit worn on `date` for `setting` (`seasons.active_outfit`),
/// counting only outfits that have clips in `m` (one without clips yet
/// means her base outfit).
pub fn worn_outfit<'a>(
    seasons: &'a lillith_core::seasons::Seasons,
    m: &Manifest,
    date: lillith_core::seasons::MonthDay,
    setting: &str,
) -> Option<&'a Outfit> {
    seasons
        .active_outfit(date, setting)
        .filter(|o| o.available(|c| m.clips.contains_key(c)))
}

/// Seasonal clips of outfits that are not worn now (kept out of random
/// picks; they stay in the manifest).
pub fn hidden_seasonal(m: &Manifest, active: Option<&Outfit>) -> BTreeSet<String> {
    m.clips
        .keys()
        .filter(|n| !seasonal_visible(n, m, active))
        .cloned()
        .collect()
}

/// `(category, clips)` for every non-empty category, clips sorted; base
/// outfit only (no seasonal clips, see [`grouped_for`]).
pub fn grouped(m: &Manifest) -> Vec<(&'static str, Vec<String>)> {
    grouped_for(m, None)
}

/// [`grouped`] with the `active` outfit's seasonal clips in Flirty /
/// Allure.
pub fn grouped_for(m: &Manifest, active: Option<&Outfit>) -> Vec<(&'static str, Vec<String>)> {
    let mut groups: Vec<Vec<String>> = vec![Vec::new(); CATEGORIES.len()];
    for name in m.clips.keys() {
        if is_pose(name, m) && seasonal_visible(name, m, active) {
            groups[category_in(name, m)].push(name.clone());
        }
    }
    CATEGORIES
        .iter()
        .zip(groups)
        .filter(|(_, g)| !g.is_empty())
        .map(|(c, mut g)| {
            g.sort();
            (*c, g)
        })
        .collect()
}

/// A random "Show off" sequence: 4–6 distinct once-clips from
/// Flirty / Allure (looping idle_flirt clips are skipped). Base outfit
/// only; see [`show_off_for`].
pub fn show_off(m: &Manifest, seed: u64) -> Vec<String> {
    show_off_for(m, seed, None)
}

/// [`show_off`] while `active` is worn: its seasonal clips join the pool
/// (they play once each, whatever their loop mode) and at least one of
/// them is in the sequence. Other outfits' clips never are.
pub fn show_off_for(m: &Manifest, seed: u64, active: Option<&Outfit>) -> Vec<String> {
    let mut pool: Vec<String> = m
        .clips
        .iter()
        .filter(|(n, c)| {
            is_pose(n, m)
                && !is_seasonal(n, m)
                && category_of(n) == 0
                && c.mode == crate::atlas::LoopMode::Once
        })
        .map(|(n, _)| n.clone())
        .collect();
    let mut seasonal: Vec<String> = m
        .clips
        .keys()
        .filter(|n| is_pose(n, m) && is_seasonal(n, m) && seasonal_visible(n, m, active))
        .cloned()
        .collect();
    pool.extend(seasonal.iter().cloned());
    pool.sort();
    seasonal.sort();
    let mut rng = lillith_core::rng::Rng::new(seed | 1);
    // Fisher-Yates.
    for i in (1..pool.len()).rev() {
        let j = rng.index(i + 1);
        pool.swap(i, j);
    }
    let n = 4 + rng.index(3);
    pool.truncate(n);
    if !seasonal.is_empty() && !pool.iter().any(|c| seasonal.contains(c)) {
        let pick = seasonal[rng.index(seasonal.len())].clone();
        let at = rng.index(pool.len().max(1));
        match pool.get_mut(at) {
            Some(slot) => *slot = pick,
            None => pool.push(pick),
        }
    }
    pool
}

/// Friendly label: `idle_flirt_hip_lean` → "Hip lean", `event_wifi` → "Wifi".
pub fn label(name: &str) -> String {
    let s = name
        .trim_start_matches("idle_flirt_")
        .trim_start_matches("event_");
    let s = s.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn categories() {
        assert_eq!(category_of("idle_flirt_lip_bite"), 0);
        assert_eq!(category_of("blow_kiss"), 0);
        assert_eq!(category_of("body_roll_slow"), 0);
        assert_eq!(category_of("bend_forward_wink"), 0);
        assert_eq!(category_of("dom_beckon_finger"), 0);
        assert_eq!(category_of("dom_throne_lean"), 0);
        assert_eq!(category_of("hip_sway_slow"), 0);
        assert_eq!(category_of("dom_chin_tilt_summon"), 0);
        assert_eq!(category_of("leg_cross_slow_sit"), 0);
        assert_eq!(category_of("dom_snap_kneel_order"), 0);
        assert_eq!(category_of("kiss_mark_on_glass"), 0);
        assert_eq!(category_of("chin_rest_slow_wink"), 0);
        assert_eq!(category_of("dom_screen_edge_whisper"), 0);
        assert_eq!(category_of("body_roll_slow_long"), 0);
        assert_eq!(category_of("arch_back_over_shoulder_long"), 0);
        assert_eq!(category_of("bend_forward_wink_long"), 0);
        assert_eq!(category_of("slow_turn_spin_tease_long"), 0);
        assert_eq!(category_of("hair_lift_stretch_long"), 0);
        assert_eq!(category_of("blow_kiss_hip_pop_long"), 0);
        assert_eq!(category_of("wall_lean_invisible_long"), 0);
        assert_eq!(category_of("kneel_hands_on_thighs_long"), 0);
        assert_eq!(category_of("idle"), 1);
        assert_eq!(category_of("wave"), 2);
        assert_eq!(category_of("twirl"), 3);
        assert_eq!(category_of("event_wifi_flirty"), 4);
        assert_eq!(category_of("talk_loop_flirty"), 5);
        assert_eq!(label("idle_flirt_hip_lean"), "Hip lean");
        assert_eq!(label("event_battery_low"), "Battery low");
    }

    #[test]
    fn pose_tags_carry_severity_and_flag_system_clips() {
        let m = Manifest::from_toml_str(
            r#"
            [clips.idle]
            page = "i.png"
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            [clips.walk_left]
            page = "w.png"
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            [clips.wave]
            page = "w.png"
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            [clips.cleavage_beckon]
            page = "c.png"
            severity = 3
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            [clips.soul_contract_sign]
            page = "s.png"
            system = true
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            [clips.event_wifi]
            page = "e.png"
            frames = [ { x = 0, y = 0, w = 4, h = 4 } ]
            "#,
        )
        .unwrap();
        let tags = pose_tags(&m);
        let get = |n: &str| tags.iter().find(|t| t.name == n).cloned();
        assert!(get("idle").is_none() && get("walk_left").is_none());
        assert_eq!(get("wave").unwrap().severity, 0);
        assert_eq!(get("cleavage_beckon").unwrap().severity, 3);
        assert!(get("soul_contract_sign").unwrap().system);
        assert!(get("event_wifi").unwrap().system);
        assert!(!get("wave").unwrap().system);
    }

    /// The shipped manifest is tagged: every tier has clips and the
    /// system-interaction clips are flagged.
    #[test]
    fn shipped_manifest_is_severity_tagged() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/generated/manifest.toml");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return;
        };
        let m = Manifest::from_toml_str(&text).unwrap();
        let tags = pose_tags(&m);
        for sev in 0..=3u8 {
            assert!(
                tags.iter().any(|t| t.severity == sev && !t.system),
                "no severity {sev} poses"
            );
        }
        for n in [
            "soul_contract_sign",
            "pose_hacking",
            "pose_wait_palm",
            "event_wifi",
        ] {
            assert!(m.clips[n].system || n.starts_with("event_"), "{n}");
            assert_eq!(m.clips[n].severity, 0, "{n}");
        }
        assert_eq!(m.clips["wave"].severity, 0);
        assert_eq!(m.clips["blow_kiss"].severity, 1);
        assert_eq!(m.clips["dom_beckon_finger"].severity, 2);
        assert_eq!(m.clips["cleavage_beckon"].severity, 3);
    }

    fn seasonal_manifest() -> Manifest {
        let mut text = String::new();
        let mut clip = |name: &str, mode: &str, tags: &str| {
            text += &format!(
                "[clips.{name}]\npage = \"{name}.png\"\nmode = \"{mode}\"\ntags = [{tags}]\nframes = [{{ x = 0, y = 0, w = 400, h = 400, anchor = [200, 392] }}]\n"
            );
        };
        for n in [
            "blow_kiss",
            "hair_flip",
            "hip_sway",
            "heart_hands",
            "lean_tease",
            "wave",
        ] {
            clip(n, "once", "\"allure\"");
        }
        for n in ["halloween_hat_tip_wink", "halloween_cape_swirl_arch"] {
            clip(
                n,
                "loop",
                "\"allure\", \"seasonal\", \"season:halloween\", \"outfit:halloween\"",
            );
        }
        clip(
            "xmas_bow_wink",
            "loop",
            "\"seasonal\", \"season:xmas\", \"outfit:xmas\"",
        );
        Manifest::from_toml_str(&text).expect("test manifest")
    }

    #[test]
    fn seasonal_clips_are_allure_and_follow_the_outfit() {
        let m = seasonal_manifest();
        let seasons = lillith_core::seasons::Seasons::bundled();
        let witch = seasons.outfit("halloween");
        assert!(witch.is_some());
        // By name alone they would be Gestures; the tag makes them Flirty.
        assert_eq!(category_of("halloween_hat_tip_wink"), 2);
        assert_eq!(category_in("halloween_hat_tip_wink", &m), 0);
        assert_eq!(category_in("wave", &m), 2);
        assert!(is_seasonal("xmas_bow_wink", &m) && !is_seasonal("blow_kiss", &m));
        // Base outfit: no seasonal clip in the pages; all base clips stay.
        let base = grouped(&m);
        let all: Vec<&String> = base.iter().flat_map(|(_, c)| c).collect();
        assert!(all.iter().all(|c| !is_seasonal(c, &m)), "{all:?}");
        assert_eq!(all.len(), 6);
        // Witch worn: her clips in Flirty, never Christmas ones.
        let worn = grouped_for(&m, witch);
        assert_eq!(worn[0].0, CATEGORIES[0]);
        assert!(worn[0].1.contains(&"halloween_hat_tip_wink".to_string()));
        assert!(worn[0].1.contains(&"halloween_cape_swirl_arch".to_string()));
        assert!(worn
            .iter()
            .all(|(_, c)| !c.contains(&"xmas_bow_wink".to_string())));
        assert_eq!(worn.iter().map(|(_, c)| c.len()).sum::<usize>(), 8);
        // Worn: by date / setting, and only with clips in the set.
        let md = |m, d| lillith_core::seasons::MonthDay::new(m, d).unwrap();
        let id = |o: Option<&Outfit>| o.map(|o| o.id.clone());
        assert_eq!(
            id(worn_outfit(&seasons, &m, md(10, 31), "auto")),
            Some("halloween".into())
        );
        assert_eq!(
            id(worn_outfit(&seasons, &m, md(7, 4), "halloween")),
            Some("halloween".into())
        );
        assert_eq!(id(worn_outfit(&seasons, &m, md(10, 31), "off")), None);
        // Christmas lists no clips in seasons.toml yet: base outfit.
        assert_eq!(id(worn_outfit(&seasons, &m, md(12, 10), "auto")), None);
        // Hidden sets.
        assert_eq!(hidden_seasonal(&m, None).len(), 3);
        assert_eq!(
            hidden_seasonal(&m, witch).into_iter().collect::<Vec<_>>(),
            ["xmas_bow_wink"]
        );
    }

    #[test]
    fn show_off_only_uses_the_worn_outfit() {
        let m = seasonal_manifest();
        let seasons = lillith_core::seasons::Seasons::bundled();
        let witch = seasons.outfit("halloween");
        for seed in 0..200 {
            let base = show_off(&m, seed);
            assert!(base.iter().all(|c| !is_seasonal(c, &m)), "{base:?}");
            let s = show_off_for(&m, seed, witch);
            assert!((4..=6).contains(&s.len()), "{s:?}");
            assert!(s.iter().any(|c| c.starts_with("halloween_")), "{s:?}");
            assert!(!s.contains(&"xmas_bow_wink".to_string()), "{s:?}");
            let mut d = s.clone();
            d.sort();
            d.dedup();
            assert_eq!(d.len(), s.len(), "distinct: {s:?}");
        }
    }
}
