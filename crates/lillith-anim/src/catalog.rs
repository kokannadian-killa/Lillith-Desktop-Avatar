//! Pose catalogue for the right-click menu: the loaded manifest's clips
//! grouped into show-and-tell categories, and the "Show off" sequence.

use crate::atlas::Manifest;

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

/// `(category, clips)` for every non-empty category, clips sorted.
pub fn grouped(m: &Manifest) -> Vec<(&'static str, Vec<String>)> {
    let mut groups: Vec<Vec<String>> = vec![Vec::new(); CATEGORIES.len()];
    for name in m.clips.keys() {
        if is_pose(name, m) {
            groups[category_of(name)].push(name.clone());
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
/// Flirty / Allure (looping idle_flirt clips are skipped).
pub fn show_off(m: &Manifest, seed: u64) -> Vec<String> {
    let mut pool: Vec<String> = m
        .clips
        .iter()
        .filter(|(n, c)| {
            is_pose(n, m) && category_of(n) == 0 && c.mode == crate::atlas::LoopMode::Once
        })
        .map(|(n, _)| n.clone())
        .collect();
    pool.sort();
    let mut rng = lillith_core::rng::Rng::new(seed | 1);
    // Fisher-Yates.
    for i in (1..pool.len()).rev() {
        let j = rng.index(i + 1);
        pool.swap(i, j);
    }
    let n = 4 + rng.index(3);
    pool.truncate(n);
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
}
