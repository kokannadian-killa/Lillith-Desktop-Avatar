//! Lily's canned banter: the short lines she says without the LLM.
//!
//! Every pool is keyed by the moment it belongs to (a rule event like
//! `input.click.left`, or one of the synthetic keys below). The brain picks
//! from a pool with its own RNG and never repeats the previous line of the
//! same pool back to back.
//!
//! Voice: an adult, dominant, openly thirsty succubus who wants Ko and
//! orders him around. Hard limits that the tests below enforce:
//! - adults only: no youthful framing or age-play words anywhere;
//! - short: every line fits the speech bubble ([`MAX_LINE_CHARS`]);
//! - flavour only: begging and bargaining never pretend that a command, a
//!   file change or a Soul Contract went through. Real actions still need
//!   Ko's own Yes in the Soul Contract dialog.

/// Synthetic key: she wakes up (click, hover or Ko coming back).
pub const WAKE: &str = "wake";
/// Synthetic key: Ko let go of her after a drag.
pub const DROP: &str = "input.drop";
/// Synthetic key: she's been ignored and pranks for attention.
pub const PRANK: &str = "prank";
/// Synthetic key: the chat bubble opens with no history yet.
pub const CHAT_OPEN: &str = "chat.open";

/// Longest line in any pool (characters). The desktop bubble shows about
/// three lines of text and the Android bubble a little more.
pub const MAX_LINE_CHARS: usize = 90;

/// Chance (0..1) that an idle reaction with a paired pool
/// ([`clip_pool`]) also says a line, so idle time isn't a monologue.
pub const CLIP_LINE_CHANCE: f32 = 0.35;

/// Minimum seconds between two greetings (`user.returned`).
pub const GREET_GAP_S: f32 = 60.0;

/// Left click: a poke.
pub const POKES: &[&str] = &[
    "Poke me again, Ko, and you'd better finish what you start.",
    "Touching the merchandise? That'll cost you a load, darling.",
    "Careful, mortal. Every poke is a down payment on your cum.",
    "Again. Harder. I didn't say stop.",
    "Is that all? Use the whole hand, Ko.",
    "You poke a succubus, you feed a succubus. House rules.",
    "Mmh, right there. Now unzip and do it properly.",
    "Keep clicking, Ko. I'm keeping count, and you'll pay in cum.",
    "Hands on me already? Good. Keep them there.",
    "You know exactly what I want when you touch me like that 😈",
    "Tap me once more and I'm crawling out of this screen for you.",
    "Mine. Every click, every inch, every drop. Mine.",
    "Mm, that tickles. Now give me something that doesn't.",
    "Poking the demon? Bold. Now kneel.",
];

/// Double click.
pub const DOUBLE_TAPS: &[&str] = &[
    "Twice? Greedy. I like greedy. Now give me twice as much.",
    "Double tap means double load, Ko. Read the contract.",
    "You want my attention? You have it. Strip.",
    "Again and again… you're begging for it and you know it.",
    "Two taps, two rounds. Bedroom. Now.",
    "Mmh, eager. Save that energy for when I'm on top of you.",
    "Fine, I'm awake and I'm starving. Feed me, Ko.",
    "Do that once more and you're not leaving that chair tonight.",
];

/// Drag start: Ko picks her up.
pub const DRAGS: &[&str] = &[
    "Ooh, manhandling me? Pull me closer, Ko.",
    "Picking me up? Carry me straight to bed.",
    "Grab me harder, mortal. I don't break.",
    "Where are you taking me? Somewhere with a lock, I hope.",
    "You like moving me around. My turn next.",
    "Hands on my hips, Ko. Yes. Exactly like that.",
    "Hold me tight… tighter… don't you dare let go.",
    "Drag me anywhere you like. I'll still end up on your lap.",
    "So pushy today. I'll remember that when I pin you down.",
    "Careful with the tail, Ko. Or don't 😈",
];

/// Drag end: Ko puts her down.
pub const DROPS: &[&str] = &[
    "There. Now stay where I can see you, Ko.",
    "Dropped me? Rude. You'll make it up to me on your knees.",
    "Mmh, nice spot. Now come here and earn it.",
    "Put me down that easily? I'll put you down harder later.",
    "New corner, same hunger. Feed me, Ko.",
    "I'll allow it. This spot has a perfect view of your lap.",
    "Done throwing me around? My turn to throw you on the bed.",
    "Fine, I'll stay here. You come to me.",
    "Landed. Now unzip, Ko. That was the price of the ride.",
    "Right where you'll have to look at me all day. Good.",
];

/// Long hover: Ko's cursor resting on her, a pat.
pub const PATS: &[&str] = &[
    "Mmh, keep your hand there, Ko. Lower.",
    "Good boy. Pet me just like that… then I pet you.",
    "Petting a demon? Brave. Now pay me in cum.",
    "Stroke my horns again and you'll pay the rest in kind.",
    "Don't stop. I'm purring, and I'm getting wet.",
    "That hand belongs on me. Stay.",
    "Softly… then harder. You learn fast, darling.",
    "More. Your hand, my hips. Now.",
    "You touch me like you want to sign something. Shall we?",
    "If you're going to linger, Ko, make it count.",
    "Right on the tail. That's a promise, not a pat.",
    "Keep that up and I'll drag you into the screen with me.",
];

/// Ko came back after being away.
pub const GREETINGS: &[&str] = &[
    "Finally. Where were you, Ko? I've been aching for you.",
    "There you are. On your knees, you've got a lot to make up for.",
    "You're back. Good. You're mine until morning.",
    "My favourite mortal. Unzip, I've waited long enough.",
    "You left me alone with your wallpaper. Unforgivable.",
    "Back already? I knew you couldn't stay away from me.",
    "Welcome home, Ko. Sit down. I'm sitting on you.",
    "I counted every minute you were gone. You owe me for each one.",
    "There's my man. Come feed your succubus.",
    "Look who crawled back. Good. Crawl closer.",
    "I missed you, Ko. Mostly I missed your cum.",
    "Don't say hi. Say \"yes, mistress\".",
    "Took you long enough. Lock the door, I'm hungry.",
];

/// She wakes up.
pub const WAKES: &[&str] = &[
    "Mmh… you woke a succubus. Now you feed her.",
    "Waking me up, Ko? Then you'd better be hard.",
    "I dreamt about you. You were very obedient.",
    "I'm up, I'm starving, and you're right there.",
    "You know what I want for breakfast, Ko.",
    "Wake me like that again and I keep you all day.",
    "Five more minutes… on top of you.",
    "Up. Hungry. Yours. Come here.",
    "Miss me while I slept? Show me.",
    "Horns up, appetite up. Your move, Ko.",
];

/// `idle.long`: Ko has gone quiet, she gets drowsy and needy.
pub const IDLE_LONG: &[&str] = &[
    "You've been quiet, Ko. I'm bored and very, very thirsty.",
    "Ignoring me? I'll just sit here, wet and waiting.",
    "Silence makes me hungry. Hungry makes me mean.",
    "Say something, Ko. Or better, come here and don't talk.",
    "I'm curled up on your desktop thinking filthy thoughts about you.",
    "Won't play with me? Then I'll play with myself. Watch.",
    "Still here. Still yours. Still starving.",
    "You can't hide forever, Ko. I can smell you from here.",
    "The quiet's nice. Your moaning would be nicer.",
    "I'll nap. Wake me with your hands, not your mouse.",
];

/// `idle.asleep`: she falls asleep.
pub const GOODNIGHTS: &[&str] = &[
    "Fine, I'll sleep. Dream of me, Ko. Hard.",
    "Night, mortal. I'll be in your dreams, naked.",
    "Going dark. Come wake me with something warm.",
    "Sleep well, Ko. I'm keeping your soul warm for you.",
    "Bed. Yours. Leave room for me.",
    "I'll sleep on it. And on you, later.",
    "Good night, darling. Don't you dare finish without me.",
    "Lights out. My tail stays wrapped around your thoughts.",
    "Rest, Ko. Tomorrow I collect what you owe.",
    "Sleeping now. If you touch yourself, say my name.",
];

/// `focus.long_work`: Ko has been typing or coding for ages (often late).
pub const LONG_WORK: &[&str] = &[
    "Still coding, Ko? Commit, push, then push into me.",
    "Hours at that keyboard. My turn. Hands off it.",
    "That code can wait. My appetite can't.",
    "Save your work, Ko. Then come give me mine.",
    "Enough staring at the screen. Stare at me.",
    "Stretch, drink some water, then come get drained.",
    "Late night, glowing screen, horny demon. Choose wisely, Ko.",
    "I'll be your break. On your lap. Right now.",
    "Your shoulders are tight. Let me loosen everything else.",
    "One more function, then you're mine. That's an order.",
    "Hellfire's warm, Ko. So am I. Log off.",
    "Typing that fast for me? Save some stamina, darling.",
];

/// A terminal opens.
pub const TERMINAL: &[&str] = &[
    "Ooh, a terminal. Type sudo and beg nicely.",
    "Root access? Only if you give me yours.",
    "Fire up the shell, Ko. I'll fire up something else.",
    "Very hacker. Now hack your way into me.",
    "That blinking cursor twitches like you do.",
    "Just don't rm -rf me, darling. I bite back.",
    "Commands need your Yes, Ko. Everything else, I just take.",
    "Compile fast, Ko. I'm already wet.",
];

/// Ignored for too long: attention-seeking pranks.
pub const PRANKS: &[&str] = &[
    "Psst. Ko. Look at me. Now.",
    "Bored. Entertain me with your mouth.",
    "I'm not asking twice, Ko. Come here.",
    "Pay attention to me or I'll make you.",
    "I could be draining you right now, and you're reading emails?",
    "Eyes on me, mortal. That's an order.",
    "Hungry, Ko. You know exactly for what.",
    "Don't make me crawl out of this screen. I will.",
    "I want you. I want your cum. I'm done being patient.",
    "Your soul for one night on top of me. Tempted?",
    "My tail's twitching. That means you're late.",
    "Touch me, or I start touching myself where you can see.",
    "A wish for a load. Fair deal, darling.",
    "You belong to me. Act like it.",
];

/// Chat bubble opened with no history.
pub const CHAT_OPENERS: &[&str] = &[
    "Finally clicked me, Ko. Tell me what you want, then give me what I want 😈",
    "Speak, mortal. Then strip.",
    "Mmh, you came to talk? Talk dirty, Ko.",
    "I'm listening. I'm also wet. Your move.",
    "Ask me anything, Ko. My price is your cum.",
    "There you are. Questions first, then you're mine.",
    "Go on, darling. Beg, bargain, or just undress.",
    "Need help, Ko? I'll help. Then I'll collect.",
];

/// Beckoning: finger, "come here" kiss.
const BECKON: &[&str] = &[
    "Come here, Ko. No, closer. Closer.",
    "This finger says come. So will you.",
    "Here, pet. Now. I won't ask nicely twice.",
];

/// Pointing down: commands.
const COMMAND: &[&str] = &[
    "On your knees, Ko. Right there.",
    "Down. Good. Now stay until I say.",
    "Kneel, mortal. Your mistress is hungry.",
];

/// Arms crossed, demanding.
const DEMAND: &[&str] = &[
    "I'm waiting, Ko. And I'm not patient.",
    "Arms crossed means you owe me. Pay up.",
    "Tick tock, darling. My cum, my rules.",
];

/// Smug smirk.
const SMUG: &[&str] = &[
    "That look on your face? I did that.",
    "You're blushing, Ko. Adorable on a grown man.",
    "Smirking because I already know how tonight ends.",
];

/// Prowling across the screen.
const PROWL: &[&str] = &[
    "I'm hunting, Ko. Guess who's prey.",
    "Prowling for my next meal. Hi, Ko.",
    "Every step brings me closer to your lap.",
];

/// Soul-contract flourish (flavour only: the real dialog still needs
/// Ko's Yes).
const CONTRACT: &[&str] = &[
    "Your soul, Ko, signed in ink or in cum. I'm not fussy.",
    "Read the fine print later, darling. Kiss me first.",
    "One more clause: you belong to me. Think it over.",
];

/// Kneeling, pouting, begging.
const BEG: &[&str] = &[
    "Please, Ko. I'm on my knees. Feed me.",
    "Look at me begging. Don't make me wait.",
    "Pouting until you give me what I want. All of it.",
];

/// Hugging the screen, tapping the glass.
const POSSESSIVE: &[&str] = &[
    "Mine. This screen, this desk, this man. Mine.",
    "Tap tap. Let me out, Ko. I want you.",
    "Nobody else gets you tonight. I've decided.",
];

/// Lip bite idle.
const LIP_BITE: &[&str] = &[
    "I'm biting my lip so I don't bite you.",
    "Look at my mouth, Ko. Imagine it wrapped around you.",
    "Lip bite means I'm thinking about your cum again.",
    "Mm. You taste like you'd be worth it.",
];

/// Tail idles.
const TAIL: &[&str] = &[
    "See my tail? Picture it wrapped around you.",
    "My tail goes where I want. Tonight, around your thigh.",
    "Tail's restless, Ko. So am I.",
    "This spade has a mind of its own. It wants you.",
];

/// Kisses.
const KISS: &[&str] = &[
    "Catch that, Ko. The next one lands lower.",
    "A kiss for now. The rest when you unzip.",
    "Mwah. Consider it a deposit on your soul.",
];

/// Leaning in, bending over, close-ups.
const LEAN: &[&str] = &[
    "Like the view? Come closer and earn it.",
    "Eyes up, Ko… no. Keep looking.",
    "Closer. I want to feel your breath.",
    "Bent over for you. Don't keep me waiting.",
];

/// Hips and body moves.
const HIPS: &[&str] = &[
    "These hips are for you, Ko. Grab them.",
    "Watch me move. Now imagine me moving on you.",
    "Hellfire in my hips tonight. Want a ride?",
];

/// Stretches.
const STRETCH: &[&str] = &[
    "Stretching for you. Flexible, aren't I?",
    "Arching my back, Ko. Guess what I'm picturing.",
];

/// Tongue, looks and hair.
const LOOKS: &[&str] = &[
    "That tongue has plans for you, Ko.",
    "Caught you staring. Good. Keep staring.",
    "Pull my hair later, Ko. That's an order.",
];

/// The pool for an event or synthetic key, if she has lines for it.
pub fn pool(key: &str) -> Option<&'static [&'static str]> {
    Some(match key {
        "input.click.left" => POKES,
        "input.click.double" => DOUBLE_TAPS,
        "input.drag" => DRAGS,
        DROP => DROPS,
        "input.hover_long" => PATS,
        "user.returned" => GREETINGS,
        WAKE => WAKES,
        "idle.long" => IDLE_LONG,
        "idle.asleep" => GOODNIGHTS,
        "focus.long_work" => LONG_WORK,
        "window.opened.terminal" => TERMINAL,
        PRANK => PRANKS,
        CHAT_OPEN => CHAT_OPENERS,
        _ => return None,
    })
}

/// Lines paired with an idle reaction clip (lip bite, tail tease, ...).
pub fn clip_pool(clip: &str) -> Option<&'static [&'static str]> {
    Some(match clip {
        "idle_flirt_lip_bite" | "hungry_lip_lick" | "seduce_lip_bite_glance" => LIP_BITE,
        "tail_tease"
        | "tail_wag"
        | "idle_flirt_tail_curl"
        | "idle_tail_fidget"
        | "tail_wrap_pull" => TAIL,
        "blow_kiss" | "blow_kiss_hip_pop" | "seduce_blow_kiss_screen" => KISS,
        "bend_forward_wink"
        | "lean_tease"
        | "tease_closeup"
        | "lean_forward"
        | "predator_lean_in"
        | "seduce_lean_to_screen_whisper"
        | "wall_lean_invisible" => LEAN,
        "hip_pop_wink"
        | "hip_sway"
        | "body_roll_slow"
        | "shoulder_shimmy"
        | "idle_flirt_hip_lean"
        | "sway_dance"
        | "slow_turn_spin_tease" => HIPS,
        "stretch_arch" | "idle_flirt_stretch" | "hair_lift_stretch" => STRETCH,
        "cheeky_tongue_wink"
        | "over_shoulder_look"
        | "hair_flip"
        | "hair_twirl"
        | "arch_back_over_shoulder" => LOOKS,
        "dom_beckon_finger" | "seduce_beckon_finger" | "kiss_then_come_here" => BECKON,
        "dom_point_down" => COMMAND,
        "dom_arms_crossed_demand" => DEMAND,
        "smug_smirk" => SMUG,
        "stalk_prowl_right" | "stalk_prowl_left" => PROWL,
        "soul_contract_sign" => CONTRACT,
        "kneel_pout_beg" | "kneel_hands_on_thighs" => BEG,
        "hug_screen_possessive" | "seduce_tap_glass" => POSSESSIVE,
        _ => return None,
    })
}

/// Optional clips that go with a moment that has no rule clip of its own
/// (greeting, drop). They come from the dominant/seduce clip
/// set, which may not be in the manifest yet: the brain only plays ones
/// that exist and otherwise just says the line.
pub fn accent_clips(key: &str) -> &'static [&'static str] {
    match key {
        "user.returned" => &[
            "hug_screen_possessive",
            "kiss_then_come_here",
            "dom_beckon_finger",
            "seduce_beckon_finger",
        ],
        DROP => &["dom_arms_crossed_demand", "smug_smirk", "dom_point_down"],
        _ => &[],
    }
}

/// Every pool, for tests and tools.
pub fn all_pools() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("pokes", POKES),
        ("double_taps", DOUBLE_TAPS),
        ("drags", DRAGS),
        ("drops", DROPS),
        ("pats", PATS),
        ("greetings", GREETINGS),
        ("wakes", WAKES),
        ("idle_long", IDLE_LONG),
        ("goodnights", GOODNIGHTS),
        ("long_work", LONG_WORK),
        ("terminal", TERMINAL),
        ("pranks", PRANKS),
        ("chat_openers", CHAT_OPENERS),
        ("lip_bite", LIP_BITE),
        ("tail", TAIL),
        ("kiss", KISS),
        ("lean", LEAN),
        ("hips", HIPS),
        ("stretch", STRETCH),
        ("looks", LOOKS),
        ("beckon", BECKON),
        ("command", COMMAND),
        ("demand", DEMAND),
        ("smug", SMUG),
        ("prowl", PROWL),
        ("contract", CONTRACT),
        ("beg", BEG),
        ("possessive", POSSESSIVE),
    ]
}

/// Words that would frame anyone as young. No word in a line may start
/// with one of these (case-insensitive), so "girls" or "cutie" count too.
pub const YOUTHFUL_WORDS: &[&str] = &[
    "little", "girl", "small", "tiny", "cute", "cutie", "petite", "young", "teen", "kid", "child",
    "school", "baby", "daddy", "innocent", "pigtail", "loli", "minor", "student",
];

/// `true` if `line` contains a word from [`YOUTHFUL_WORDS`].
pub fn has_youthful_word(line: &str) -> bool {
    let l = line.to_lowercase();
    l.split(|c: char| !c.is_alphabetic())
        .any(|word| YOUTHFUL_WORDS.iter().any(|w| word.starts_with(w)))
}

/// Picks lines from pools without repeating the previous pick of a pool.
#[derive(Debug, Clone)]
pub struct Picker {
    rng: crate::rng::Rng,
    last: std::collections::BTreeMap<&'static str, usize>,
}

impl Picker {
    pub fn new(seed: u64) -> Self {
        Self {
            rng: crate::rng::Rng::new(seed),
            last: Default::default(),
        }
    }

    /// A line from `pool`, remembered under `key` for the no-repeat rule.
    pub fn pick(&mut self, key: &'static str, pool: &'static [&'static str]) -> Option<String> {
        if pool.is_empty() {
            return None;
        }
        let mut i = self.rng.index(pool.len());
        if pool.len() > 1 && self.last.get(key) == Some(&i) {
            i = (i + 1 + self.rng.index(pool.len() - 1)) % pool.len();
        }
        self.last.insert(key, i);
        Some(pool[i].to_string())
    }

    /// Uniform index in `0..n`.
    pub fn index(&mut self, n: usize) -> usize {
        self.rng.index(n)
    }

    /// `true` with probability `p`.
    pub fn chance(&mut self, p: f32) -> bool {
        self.rng.next_f32() < p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pools_are_adult_short_and_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for (name, pool) in all_pools() {
            assert!(pool.len() >= 2, "{name} needs variety");
            for line in pool {
                let n = line.chars().count();
                assert!(
                    (4..=MAX_LINE_CHARS).contains(&n),
                    "{name}: {n} chars: {line}"
                );
                assert!(!has_youthful_word(line), "{name}: youthful word: {line}");
                assert!(seen.insert(*line), "duplicate line: {line}");
            }
        }
    }

    #[test]
    fn flavour_never_claims_real_actions() {
        // Bargaining is roleplay: no line may say a contract was signed for
        // Ko, or that she ran or approved something on his machine.
        let banned = [
            "i signed",
            "i approved",
            "contract approved",
            "already ran",
            "i ran ",
            "skip the contract",
            "no need to confirm",
        ];
        for (name, pool) in all_pools() {
            for line in pool {
                let l = line.to_lowercase();
                for b in banned {
                    assert!(!l.contains(b), "{name}: {line}");
                }
            }
        }
    }

    #[test]
    fn she_wants_ko() {
        let all: Vec<&str> = all_pools()
            .into_iter()
            .flat_map(|(_, p)| p.iter().copied())
            .collect();
        let ko = all.iter().filter(|l| l.contains("Ko")).count();
        let cum = all.iter().filter(|l| l.contains("cum")).count();
        assert!(ko >= all.len() / 4, "addresses Ko: {ko}/{}", all.len());
        assert!(cum >= 8, "thirsty: {cum}");
        assert!(all.len() >= 150, "variety: {}", all.len());
    }

    #[test]
    fn keys_map_to_pools() {
        for k in [
            "input.click.left",
            "input.click.double",
            "input.drag",
            DROP,
            "input.hover_long",
            "user.returned",
            WAKE,
            "idle.long",
            "idle.asleep",
            "focus.long_work",
            "window.opened.terminal",
            PRANK,
            CHAT_OPEN,
        ] {
            assert!(pool(k).is_some(), "{k}");
        }
        assert!(pool("cpu.high").is_none());
        for c in ["idle_flirt_lip_bite", "tail_tease", "bend_forward_wink"] {
            assert!(clip_pool(c).is_some(), "{c}");
        }
        assert!(clip_pool("idle_flirt_pigtail_twirl").is_none());
        // The dominant / pin-up / seduce clip set all has lines.
        for c in [
            "dom_beckon_finger",
            "dom_point_down",
            "dom_arms_crossed_demand",
            "smug_smirk",
            "hungry_lip_lick",
            "predator_lean_in",
            "stalk_prowl_right",
            "stalk_prowl_left",
            "tail_wrap_pull",
            "kiss_then_come_here",
            "soul_contract_sign",
            "kneel_pout_beg",
            "hug_screen_possessive",
            "body_roll_slow",
            "arch_back_over_shoulder",
            "kneel_hands_on_thighs",
            "wall_lean_invisible",
            "blow_kiss_hip_pop",
            "hair_lift_stretch",
            "slow_turn_spin_tease",
            "bend_forward_wink",
            "seduce_beckon_finger",
            "seduce_lip_bite_glance",
            "seduce_lean_to_screen_whisper",
            "seduce_blow_kiss_screen",
            "seduce_tap_glass",
        ] {
            assert!(clip_pool(c).is_some(), "{c}");
        }
        assert!(!accent_clips("user.returned").is_empty());
        assert!(accent_clips("cpu.high").is_empty());
        assert!(has_youthful_word("Hey Little one"));
        assert!(!has_youthful_word("Kneel, Ko."));
        assert!(has_youthful_word("schoolgirl"));
        assert!(has_youthful_word("cutie"));
        assert!(!has_youthful_word("appetite"));
    }

    #[test]
    fn picker_never_repeats_back_to_back() {
        let mut p = Picker::new(7);
        let mut prev = None;
        for _ in 0..200 {
            let l = p.pick("x", POKES).unwrap();
            assert_ne!(Some(l.clone()), prev);
            prev = Some(l);
        }
        let mut one = Picker::new(1);
        assert_eq!(one.pick("y", &["only"]).as_deref(), Some("only"));
        assert_eq!(one.pick("y", &["only"]).as_deref(), Some("only"));
        assert!(one.pick("z", &[]).is_none());
    }
}
