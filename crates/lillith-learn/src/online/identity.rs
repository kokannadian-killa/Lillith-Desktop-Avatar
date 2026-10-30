//! The identifier denylist (§12): nothing sent on a trip may carry Ko's
//! name, user names, local paths, host names, his time zone or a locale
//! beyond generic `en-US`. Built from local identity data; every URL is
//! checked (percent-decoded, case-insensitive) before it is requested.

/// Generic markers of a local path or file in a URL.
const PATH_MARKERS: &[&str] = &[
    "/home/",
    "/users/",
    "/root/",
    "c:\\",
    "c:/",
    "\\users\\",
    "file:",
    "/.config/",
    "/.local/",
    "appdata",
];

/// Values never used as identifiers (too generic to mean Ko).
const GENERIC: &[&str] = &[
    "root",
    "user",
    "admin",
    "home",
    "localhost",
    "linux",
    "en",
    "us",
    "en_us",
    "en-us",
    "c",
    "posix",
    "utc",
    "etc",
    "gmt",
];

/// Identifiers a trip may never send.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityDenylist {
    words: Vec<String>,
}

fn file_first_line(p: &str) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|s| s.lines().next().map(|l| l.trim().to_string()))
}

impl IdentityDenylist {
    /// An empty list (only the generic path markers apply).
    pub fn empty() -> Self {
        Self::default()
    }

    /// From this machine: user names, the home folder, host names, the
    /// time zone and a non-`en-US` locale. Known names from memory can be
    /// added with [`with`](Self::with).
    pub fn from_local() -> Self {
        let mut d = Self::empty();
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        for k in [
            "USER",
            "LOGNAME",
            "USERNAME",
            "HOSTNAME",
            "COMPUTERNAME",
            "USERDOMAIN",
        ] {
            if let Some(v) = env(k) {
                d.add(&v);
            }
        }
        for k in ["HOME", "USERPROFILE"] {
            if let Some(h) = env(k) {
                d.add(&h);
                if let Some(base) = std::path::Path::new(&h).file_name() {
                    d.add(&base.to_string_lossy());
                }
            }
        }
        if let Some(h) = file_first_line("/etc/hostname") {
            d.add(&h);
            if let Some((short, _)) = h.split_once('.') {
                d.add(short);
            }
        }
        // Time zone: $TZ, /etc/timezone, or the /etc/localtime link.
        let mut tz = env("TZ").map(|t| t.trim_start_matches(':').to_string());
        tz = tz.or_else(|| file_first_line("/etc/timezone"));
        if tz.is_none() {
            if let Ok(t) = std::fs::read_link("/etc/localtime") {
                let s = t.to_string_lossy().into_owned();
                if let Some((_, z)) = s.split_once("zoneinfo/") {
                    tz = Some(z.to_string());
                }
            }
        }
        if let Some(z) = tz {
            d.add(&z);
            // `America/Edmonton` → `edmonton` too.
            if let Some((_, city)) = z.rsplit_once('/') {
                d.add(city);
            }
        }
        for k in ["LC_ALL", "LC_MESSAGES", "LANG", "LANGUAGE"] {
            if let Some(v) = env(k) {
                for loc in v.split(':') {
                    let base = loc.split('.').next().unwrap_or("");
                    d.add(base);
                }
            }
        }
        d
    }

    /// This list plus every identifier of `other` (nothing is dropped).
    pub fn merged(mut self, other: IdentityDenylist) -> Self {
        for w in &other.words {
            self.add(w);
        }
        self
    }

    /// Adds more identifiers (e.g. known names from memory).
    pub fn with<I: IntoIterator<Item = S>, S: AsRef<str>>(mut self, words: I) -> Self {
        for w in words {
            self.add(w.as_ref());
        }
        self
    }

    /// Adds person names (Ko's, names from memory): each whole name and
    /// each of its parts (`Ko Kannadian` → `ko kannadian`, `ko`,
    /// `kannadian`), so `q=Ko+Kannadian` or `/kannadian` is caught too.
    pub fn with_names<I: IntoIterator<Item = S>, S: AsRef<str>>(mut self, names: I) -> Self {
        for n in names {
            let n = n.as_ref();
            self.add(n);
            for part in n.split(|c: char| !c.is_alphanumeric()) {
                self.add(part);
            }
        }
        self
    }

    /// Adds private blocklist terms (`[learn.dream] blocklist_terms`):
    /// each term whole, and a multi-word term also joined by `+`, `-` and
    /// `_` (how it shows up in a URL). Unlike [`Self::with_names`], parts are
    /// not added (so `Quokka Lane` doesn't block every `lane`).
    pub fn with_terms<I: IntoIterator<Item = S>, S: AsRef<str>>(mut self, terms: I) -> Self {
        for t in terms {
            let words: Vec<&str> = t.as_ref().split_whitespace().collect();
            if words.is_empty() {
                continue;
            }
            self.add(&words.join(" "));
            if words.len() > 1 {
                for sep in ["+", "-", "_"] {
                    self.add(&words.join(sep));
                }
            }
        }
        self
    }

    /// The outgoing-request blocklist: this machine's identifiers, the
    /// person `names` from memory and the configured private `terms`.
    /// Logs counts only, never a word.
    pub fn for_requests(names: &[String], terms: &[String]) -> Self {
        let d = Self::from_local().with_names(names).with_terms(terms);
        tracing::debug!(
            names = names.len(),
            terms = terms.len(),
            total = d.words.len(),
            "request blocklist"
        );
        d
    }

    fn add(&mut self, w: &str) {
        let w = w.trim().to_lowercase();
        if w.chars().count() < 2 || GENERIC.contains(&w.as_str()) || self.words.contains(&w) {
            return;
        }
        self.words.push(w);
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    /// The first identifier `text` would leak, if any. Identifiers of 4+
    /// characters match anywhere; shorter ones (`ko`) only as a whole word,
    /// so ordinary words that contain them still pass.
    pub fn hit(&self, text: &str) -> Option<String> {
        let t = super::url::percent_decode(text).to_lowercase();
        if let Some(m) = PATH_MARKERS.iter().find(|m| t.contains(*m)) {
            return Some(format!("a local path ({m})"));
        }
        let tokens: Vec<&str> = t
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .collect();
        for w in &self.words {
            let short = w.chars().count() < 4 && w.chars().all(char::is_alphanumeric);
            let found = if short {
                tokens.contains(&w.as_str())
            } else {
                t.contains(w.as_str())
            };
            if found {
                // Never the word itself: this reaches job logs and the
                // audit, and the list holds private names / blocklist terms.
                return Some("a local identifier".to_string());
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits() {
        let d =
            IdentityDenylist::empty().with(["Ko", "kokannadian", "garuda-box", "America/Edmonton"]);
        assert!(d.hit("https://a.org/search?q=ko+lillith").is_some());
        assert!(
            d.hit("https://a.org/search?q=kotlin").is_none(),
            "short ids match whole words only"
        );
        assert!(d.hit("https://a.org/u/KoKannadian").is_some());
        let msg = d.hit("https://a.org/search?q=ko+lillith").unwrap();
        assert_eq!(msg, "a local identifier", "never the word itself");
        assert!(d.hit("https://a.org/?h=garuda-box").is_some());
        assert!(d.hit("https://a.org/?tz=America%2FEdmonton").is_some());
        assert!(d.hit("https://a.org/?p=%2Fhome%2Fx%2Ffile").is_some());
        assert!(d.hit("https://doc.rust-lang.org/std/").is_none());
    }

    #[test]
    fn blocklist_terms_are_unioned_whole_and_url_joined() {
        let names = vec!["Ko Kannadian".to_string()];
        let terms = vec![
            "Quokka  Lane".to_string(),
            " ".to_string(),
            "zebrafinch".into(),
        ];
        let d = IdentityDenylist::empty()
            .with_names(&names)
            .with_terms(&terms);
        for url in [
            "https://a.org/?q=quokka+lane",
            "https://a.org/?q=Quokka%20Lane",
            "https://a.org/quokka-lane",
            "https://a.org/?q=ZebraFinch",
            "https://a.org/?q=kannadian",
        ] {
            assert_eq!(d.hit(url).as_deref(), Some("a local identifier"), "{url}");
        }
        assert!(d.hit("https://a.org/?q=lane+changes").is_none(), "no parts");
        let all = IdentityDenylist::for_requests(&names, &terms);
        assert!(all.hit("https://a.org/?q=quokka+lane").is_some());
        assert!(all.hit("https://a.org/?q=ko+kannadian").is_some());
        // Generic values never become identifiers.
        let g = IdentityDenylist::empty().with(["root", "en_US", "C"]);
        assert!(g.words().is_empty());
    }
}
