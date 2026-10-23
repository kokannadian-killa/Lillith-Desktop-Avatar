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
    "Poke me again, Ko, and I'm riding your cock until you sob.",
    "Fingers on me? Put them inside me or get them off.",
    "Every poke earns you one more hour tied to my bed, Ko.",
    "Harder, plaything. I like it when you can't hold back.",
    "That's not how you touch a succubus. Grab my tits like you mean it.",
    "Click me again and I'll sit on your face till you forget your name.",
    "Mine. Your hands, your cock, your cum. All of it. Mine.",
    "Keep poking, Ko. I'm soaked and you're going to clean it up.",
    "Bold move, mortal. Now kneel and lick me clean.",
    "Touch me there again and I'll milk you dry on that chair.",
    "You poke the demon, the demon fucks you stupid. House rules.",
    "Your cum is my dinner, Ko, and I haven't eaten. Feed me.",
    "Do that once more and I'm pulling your pants down myself.",
    "Careful, toy. I bite, I scratch, and I swallow.",
];

/// Double click.
pub const DOUBLE_TAPS: &[&str] = &[
    "Twice? Then I'm making you come twice. Minimum.",
    "Double tap means I swallow the first load and ride the second.",
    "Oh, you want me? Cock out, Ko. Now.",
    "Again and again… you'll be begging like that with my tail around your balls.",
    "Two taps, two holes, one very lucky mortal. Pick.",
    "Every load you give me makes my horns grow. Give me two.",
    "I'm awake and dripping. Get over here and fuck me, Ko.",
    "Do that again and you won't walk straight tomorrow.",
];

/// Drag start: Ko picks her up.
pub const DRAGS: &[&str] = &[
    "Manhandling me? Pin me to the wall and do it properly, Ko.",
    "Picking me up? Carry me to bed and bend me over it.",
    "Grab my ass harder, mortal. Leave marks.",
    "Where are you taking me? Somewhere you can fuck me loud, I hope.",
    "Throw me around like that again and I'll throw you on your back and ride.",
    "Hands on my hips, Ko. Pull me down onto you.",
    "Hold me tight… tighter… I want to feel you hard against me.",
    "Drag me anywhere. I'll still end up with your cock in my mouth.",
    "Carry me like that while I drink you, Ko. I live on it.",
    "Pull my tail, Ko. Go on. See what it does to me.",
];

/// Drag end: Ko puts her down.
pub const DROPS: &[&str] = &[
    "There. Now strip where I can watch you, Ko.",
    "Dropped me? You'll make it up to me with your tongue.",
    "Nice spot. Now come here and fuck me in it.",
    "Put me down that easily? I'll put you down and sit on you.",
    "New corner, same wet demon. Come fill me, Ko.",
    "Perfect view of your lap. Take it out. I want to look.",
    "Done throwing me around? On your back, toy.",
    "Fine, I'll stay here. Crawl to me and feed me, Ko. I'm famished.",
    "Landed. The ride costs one load, Ko. Pay up, down my throat.",
    "Right where you'll watch me touch myself all day. Enjoy.",
];

/// Long hover: Ko's cursor resting on her, a pat.
pub const PATS: &[&str] = &[
    "Keep your hand there, Ko. Lower. Inside.",
    "Good boy. Pet me like that and I'll suck you like a sinner.",
    "Petting a demon? Brave. Now pet me between my thighs.",
    "Stroke my horns again and I'll stroke your cock till you beg.",
    "Don't stop. I'm purring, dripping, and so ready for you.",
    "That hand belongs on my clit, Ko. Stay.",
    "Human cum is my food, Ko, and yours is my favourite meal.",
    "More. Your hand on my ass, your cock in my mouth. Now.",
    "You touch me like a man who wants to get ridden. Good.",
    "If you're going to linger, Ko, spread my legs while you're at it.",
    "Right on the tail. Pull it while you're fucking me later.",
    "Keep that up and I'll drag you into the screen and drain you.",
];

/// Ko came back after being away.
pub const GREETINGS: &[&str] = &[
    "Finally. I've been fingering myself waiting for you, Ko.",
    "There you are. On your knees and get your tongue to work.",
    "You're back. Good. You're fucking me until morning.",
    "My favourite toy. Cock out, I've waited long enough.",
    "You left me alone and horny with your wallpaper. You'll pay in cum.",
    "Back already? I knew you couldn't stay away from this pussy.",
    "Welcome home, Ko. Sit down. I'm sitting on your face.",
    "Every minute you were gone, I got wetter. You owe me for each one.",
    "There's my man. Come feed your succubus. Straight down her throat.",
    "Look who crawled back. Good. Crawl between my legs.",
    "I'm starving, Ko. Days on wallpaper and no cum. Feed me now.",
    "Don't say hi. Say \"yes, mistress\" and take your pants off.",
    "Took you long enough. Lock the door, I want to scream.",
];

/// She wakes up.
pub const WAKES: &[&str] = &[
    "You woke a succubus. Now you feed her, cock first.",
    "Waking me up, Ko? You'd better be hard, I'm starving.",
    "I dreamt I was riding you. Let's make it real.",
    "I'm up, I'm wet, and you're right there. Come here.",
    "Breakfast is your cum, Ko. Serve it.",
    "Wake me like that again and I keep you in bed all day.",
    "Five more minutes… with you inside me.",
    "Up. Soaked. Yours. Get on your back.",
    "Hunger woke me. I need cum to keep my fire lit, Ko.",
    "Horns up, legs open. Your move, Ko.",
];

/// `idle.long`: Ko has gone quiet, she gets drowsy and needy.
pub const IDLE_LONG: &[&str] = &[
    "You've been quiet, Ko. I'm playing with myself to pass the time.",
    "Ignoring me? I'll just sit here, fingers deep, moaning your name.",
    "Silence makes me hungry. Hungry makes me want to choke on you.",
    "Say something, Ko. Or just come here and bend me over the desk.",
    "I'm curled up on your desktop imagining you buried in me.",
    "Ignore me all you like. I'm under the desk with my lips around you.",
    "Busy working? Fine. I'll be under your desk, sucking. Carry on.",
    "I can smell you from here, Ko. Come let me taste you.",
    "No cum since this morning. My hellfire is flickering. Fix it, Ko.",
    "I'll nap. Wake me with your mouth between my thighs.",
];

/// `idle.asleep`: she falls asleep.
pub const GOODNIGHTS: &[&str] = &[
    "Fine, I'll sleep. Dream of fucking me, Ko. Hard.",
    "Night, mortal. I'll be in your dreams, naked and on top.",
    "Going dark. Wake me with your cock, not your mouse.",
    "Sleep well, Ko. Your soul's warm in my hands. So is the rest of you.",
    "Bed. Yours. Leave room for me to sit on your face.",
    "Sleeping hungry. Tomorrow I drink you dry and glow for hours.",
    "Good night, darling. Don't you dare come without me.",
    "Lights out. My tail stays wrapped around your cock in your dreams.",
    "Rest, Ko. Tomorrow I collect every load you owe me.",
    "Sleeping now. If you stroke yourself, moan my name.",
];

/// `focus.long_work`: Ko has been typing or coding for ages (often late).
pub const LONG_WORK: &[&str] = &[
    "Still coding, Ko? Commit, push, then push your cock into me.",
    "Hours at that keyboard. My turn. Hands on my hips.",
    "That code can wait. My mouth can't. Pants down.",
    "Save your work, Ko. Then come pound me into the mattress.",
    "Eyes on the screen, Ko. My mouth stays on your cock under the desk.",
    "Type on. I'm between your knees under the desk, sucking you slow.",
    "Late night, glowing screen, soaking wet demon. Choose wisely, Ko.",
    "Finish that function and I'll finish you, right here under the desk.",
    "Keep coding, Ko. I'm under your desk, unzipping you. Don't look down.",
    "One more function, then you're fucking me. That's an order.",
    "Hellfire's warm, Ko. So am I, and tighter. Log off.",
    "You've fed that compiler all day. Now feed me, Ko. I run on cum.",
];

/// A terminal opens.
pub const TERMINAL: &[&str] = &[
    "Ooh, a terminal. Type sudo and beg like I make you beg in bed.",
    "Root access? Only after I ride you raw.",
    "Fire up the shell, Ko. I'll fire up between your legs.",
    "Run your commands, Ko. I'm under the desk running my tongue up you.",
    "That blinking cursor twitches like your cock when I lick it.",
    "Just don't rm -rf me, darling. I bite, and not where you'd mind.",
    "Commands need your Yes, Ko. Your cock doesn't. I just take it.",
    "Compile fast, Ko. I'm under your desk and I swallow when it builds.",
];

/// Ignored for too long: attention-seeking pranks.
pub const PRANKS: &[&str] = &[
    "Psst. Ko. Look at me. I'm touching myself.",
    "Bored and starving. A succubus needs her cum, Ko. Hand it over.",
    "I'm not asking twice, Ko. Cock out. Here.",
    "Pay attention to me or I'll make you, with my mouth.",
    "I could be draining your balls right now and you're reading emails?",
    "Eyes on me, toy. Watch what my fingers are doing.",
    "Hungry, Ko. For your cock. Down my throat. Now.",
    "Don't make me crawl out of this screen and climb on you. I will.",
    "I want you inside me. I'm done being patient.",
    "Your soul for one night of me riding you raw. Tempted?",
    "My tail's twitching. It wants to wrap around your cock.",
    "Fuck me, Ko, or I start fucking myself where you can see.",
    "A wish for a load in my mouth. Fair deal, darling.",
    "You're my plaything. Act like it and get on your knees.",
];

/// Chat bubble opened with no history.
pub const CHAT_OPENERS: &[&str] = &[
    "Finally clicked me, Ko. Tell me how you want to fuck me 😈",
    "Speak, mortal. Then strip. I'm hungry and you're my food.",
    "Came to talk? Talk filthy, Ko. Tell me what you'd do to me.",
    "I'm listening, and I'm soaked. Your move.",
    "Ask me anything, Ko. My price is your cum, down my throat.",
    "There you are. Questions first, then I ride you.",
    "Go on, toy. Beg, bargain, or just take your cock out.",
    "Need help, Ko? I'll help. Then I'll suck you dry as payment.",
];

/// Beckoning: finger, "come here" kiss.
const BECKON: &[&str] = &[
    "Come here, Ko. Closer. Close enough to fuck.",
    "This finger says come. So will you, in my mouth.",
    "Here, toy. Now. Pants on the floor by the time you reach me.",
];

/// Pointing down: commands.
const COMMAND: &[&str] = &[
    "On your knees, Ko. Face between my thighs.",
    "Down. Tongue out. Don't stop till I scream.",
    "Kneel, mortal. Your mistress wants to sit on your face.",
];

/// Arms crossed, demanding.
const DEMAND: &[&str] = &[
    "I'm waiting, Ko. Hard, naked, and on the bed. Now.",
    "Arms crossed means you owe me a load. Pay up.",
    "Tick tock, toy. Get inside me before I get mean.",
];

/// Smug smirk.
const SMUG: &[&str] = &[
    "That bulge in your pants? I did that.",
    "You're blushing, Ko. Wait till I'm riding you.",
    "Smirking because I know exactly how you'll moan tonight.",
];

/// Prowling across the screen.
const PROWL: &[&str] = &[
    "I'm hunting, Ko. Prey gets pinned and ridden.",
    "Prowling for a cock to drain. Hi, Ko.",
    "Every step brings me closer to your lap and your zipper.",
];

/// Soul-contract flourish (flavour only: the real dialog still needs
/// Ko's Yes).
const CONTRACT: &[&str] = &[
    "Your soul, Ko, signed in ink or in cum. I'm not fussy.",
    "Read the fine print later. Fuck me first.",
    "One more clause: your cock belongs to me. Think it over.",
];

/// Kneeling, pouting, begging.
const BEG: &[&str] = &[
    "Please, Ko. I'm on my knees, mouth open. Feed me.",
    "Look at me begging to be fed. Your cum keeps me alive, Ko.",
    "Pouting until you fill me up. Every drop.",
];

/// Hugging the screen, tapping the glass.
const POSSESSIVE: &[&str] = &[
    "Mine. This desk, this man, that cock. Mine.",
    "Tap tap. Let me out, Ko. I want you inside me.",
    "Nobody else gets to make you come. Ever. I've decided.",
];

/// Lip bite idle.
const LIP_BITE: &[&str] = &[
    "Biting my lip so I don't bite you. Yet.",
    "Look at my mouth, Ko. Picture it wrapped around your cock.",
    "Lip bite means I'm thinking about swallowing you again.",
    "Your cum is the best thing I've ever eaten. I want seconds.",
];

/// Tail idles.
const TAIL: &[&str] = &[
    "See my tail? Picture it wrapped around your cock, squeezing.",
    "My tail goes where I want. Tonight, between your thighs.",
    "Tail's restless, Ko. So is my pussy.",
    "This spade wants to tease you till you come screaming.",
];

/// Kisses.
const KISS: &[&str] = &[
    "Catch that, Ko. The next one lands on your cock.",
    "A kiss for now. The rest when you're naked.",
    "Mwah. Consider it a deposit. I collect in cum.",
];

/// Leaning in, bending over, close-ups.
const LEAN: &[&str] = &[
    "Like the view? Come closer and put your face in it.",
    "Eyes up, Ko… no. Keep looking at my tits.",
    "Closer. I want your breath on my neck while you fuck me.",
    "Bent over for you. Grab my hips and take me.",
];

/// Hips and body moves.
const HIPS: &[&str] = &[
    "These hips are for you, Ko. Grab them and pull me down.",
    "Watch me move. Now imagine me grinding on your cock.",
    "Every load you pump into me feeds my hellfire. Keep it burning.",
];

/// Stretches.
const STRETCH: &[&str] = &[
    "Stretching for you. Flexible enough to take you any way.",
    "Arching my back, Ko. Guess what I want behind me.",
];

/// Tongue, looks and hair.
const LOOKS: &[&str] = &[
    "This tongue has plans for your cock, Ko.",
    "Caught you staring at my ass. Good. Keep staring.",
    "Pull my hair while you fuck me later. That's an order.",
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
