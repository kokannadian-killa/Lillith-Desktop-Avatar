//! Habit / pattern mining over the event log.
//!
//! Events are bucketed by local time of day (3-hour buckets) and day class
//! (weekday vs weekend). For "now", every subject seen in the same bucket
//! (adjacent buckets count half) is ranked by its share of that bucket,
//! damped for small samples. One pass over a bounded event list; no model.

/// Seconds in a day.
pub const DAY_S: i64 = 86_400;

/// Local (hour 0..24, weekday 0 = Monday .. 6 = Sunday) for a unix time
/// plus the local UTC offset in seconds.
pub fn local_hour_weekday(unix_s: f64, utc_offset_s: i32) -> (u32, u32) {
    let t = unix_s.floor() as i64 + utc_offset_s as i64;
    let days = t.div_euclid(DAY_S);
    let secs = t.rem_euclid(DAY_S);
    // 1970-01-01 was a Thursday (= 3 with Monday = 0).
    let weekday = (days + 3).rem_euclid(7) as u32;
    ((secs / 3600) as u32, weekday)
}

/// 3-hour bucket 0..8.
pub fn bucket(hour: u32) -> u32 {
    (hour % 24) / 3
}

/// Whether a weekday (0 = Monday) is Saturday/Sunday.
pub fn is_weekend(weekday: u32) -> bool {
    weekday >= 5
}

/// Human name for a bucket ("late night", "morning", ...).
pub fn bucket_name(b: u32) -> &'static str {
    match b {
        0 => "late night",
        1 => "early morning",
        2 => "morning",
        3 => "late morning",
        4 => "afternoon",
        5 => "late afternoon",
        6 => "evening",
        _ => "night",
    }
}

/// One logged event as the miner sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub kind: String,
    pub subject: String,
    pub hour: u32,
    pub weekday: u32,
}

/// How often Ko shows up in each 3-hour bucket (event counts, 0..8): the
/// "usually around / usually away" profile the loneliness score is scaled
/// by (`lillith_core::loneliness::habit_factor`). Pure: reads the rows it
/// is given, never the database.
pub fn presence_profile(events: &[EventRow]) -> [f32; 8] {
    let mut out = [0.0f32; 8];
    for e in events {
        out[bucket(e.hour) as usize] += 1.0;
    }
    out
}

/// A guess at what Ko will want next.
#[derive(Debug, Clone, PartialEq)]
pub struct Prediction {
    /// `app`, `request`, ...
    pub kind: String,
    pub subject: String,
    /// 0..1.
    pub confidence: f64,
    /// Weighted number of matching past events.
    pub support: f64,
}

impl Prediction {
    /// "open firefox", "ask about weather".
    pub fn describe(&self) -> String {
        match self.kind.as_str() {
            "app" => format!("open {}", self.subject),
            "request" => format!("ask about {}", self.subject),
            k => format!("{k}: {}", self.subject),
        }
    }
}

fn bucket_weight(a: u32, b: u32) -> f64 {
    let d = (a as i32 - b as i32).rem_euclid(8);
    match d.min(8 - d) {
        0 => 1.0,
        1 => 0.5,
        _ => 0.0,
    }
}

/// Ranks subjects for (`hour`, `weekday`). Subjects need a weighted support
/// of at least `min_support`; at most `limit` results, best first.
pub fn predict(
    events: &[EventRow],
    hour: u32,
    weekday: u32,
    min_support: f64,
    limit: usize,
) -> Vec<Prediction> {
    use std::collections::HashMap;
    let b = bucket(hour);
    let wk = is_weekend(weekday);
    let mut per: HashMap<(&str, &str), f64> = HashMap::new();
    let mut total = 0.0;
    for e in events {
        let mut w = bucket_weight(bucket(e.hour), b);
        if w == 0.0 {
            continue;
        }
        if is_weekend(e.weekday) != wk {
            w *= 0.25;
        }
        if e.weekday == weekday {
            w *= 1.25;
        }
        total += w;
        *per.entry((e.kind.as_str(), e.subject.as_str()))
            .or_default() += w;
    }
    let mut out: Vec<Prediction> = per
        .into_iter()
        .filter(|(_, n)| *n >= min_support)
        .map(|((k, s), n)| {
            let share = (n + 0.1) / (total + 1.0);
            let damp = n / (n + 3.0);
            Prediction {
                kind: k.to_string(),
                subject: s.to_string(),
                confidence: (share * damp).clamp(0.0, 1.0),
                support: n,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.subject.cmp(&b.subject))
    });
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: &str, subject: &str, hour: u32, weekday: u32) -> EventRow {
        EventRow {
            kind: kind.into(),
            subject: subject.into(),
            hour,
            weekday,
        }
    }

    #[test]
    fn local_time_math() {
        // 1970-01-01 00:00 UTC was a Thursday.
        assert_eq!(local_hour_weekday(0.0, 0), (0, 3));
        // 2026-10-23 21:56 MDT (UTC-6) = 2026-10-24 03:56 UTC, a Friday locally.
        let t = 1_792_814_160.0; // 2026-10-24T03:56:00Z
        assert_eq!(local_hour_weekday(t, -6 * 3600), (21, 4));
        assert_eq!(local_hour_weekday(t, 0), (3, 5));
        assert_eq!(bucket(23), 7);
        assert_eq!(bucket(0), 0);
        assert!(is_weekend(6) && !is_weekend(0));
    }

    #[test]
    fn presence_profile_counts_per_bucket() {
        let evs = [
            ev("app", "steam", 20, 0),
            ev("app", "steam", 19, 1),
            ev("chat", "hi", 23, 2),
            ev("app", "vscode", 10, 3),
        ];
        let p = presence_profile(&evs);
        assert_eq!(p[6], 2.0);
        assert_eq!(p[7], 1.0);
        assert_eq!(p[3], 1.0);
        assert_eq!(p.iter().sum::<f32>(), 4.0);
        assert_eq!(presence_profile(&[]), [0.0; 8]);
    }

    #[test]
    fn evening_habit_wins_in_the_evening() {
        let mut evs = Vec::new();
        for d in 0..5 {
            evs.push(ev("app", "steam", 20, d));
            evs.push(ev("app", "steam", 21, d));
            evs.push(ev("app", "vscode", 10, d));
        }
        evs.push(ev("app", "vscode", 19, 2));
        let p = predict(&evs, 20, 3, 2.0, 3);
        assert_eq!(p[0].subject, "steam");
        assert!(p[0].confidence > 0.4, "{p:?}");
        let morning = predict(&evs, 10, 1, 2.0, 3);
        assert_eq!(morning[0].subject, "vscode");
        assert!(morning.iter().all(|x| x.subject != "steam"));
    }

    #[test]
    fn small_samples_are_damped_and_filtered() {
        let evs = vec![ev("app", "gimp", 14, 1)];
        assert!(predict(&evs, 14, 1, 2.0, 5).is_empty());
        let one = predict(&evs, 14, 1, 0.5, 5);
        let many: Vec<_> = (0..20).map(|_| ev("app", "gimp", 14, 1)).collect();
        let lots = predict(&many, 14, 1, 0.5, 5);
        assert!(lots[0].confidence > one[0].confidence);
        assert!(lots[0].confidence <= 1.0);
        assert!(predict(&[], 3, 3, 0.0, 5).is_empty());
    }

    #[test]
    fn weekends_differ() {
        let mut evs = Vec::new();
        for _ in 0..6 {
            evs.push(ev("request", "weather", 9, 5)); // Saturday
            evs.push(ev("app", "vscode", 9, 1)); // Tuesday
        }
        assert_eq!(predict(&evs, 9, 6, 1.0, 2)[0].subject, "weather");
        assert_eq!(predict(&evs, 9, 2, 1.0, 2)[0].subject, "vscode");
    }
}
