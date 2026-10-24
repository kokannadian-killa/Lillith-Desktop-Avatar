//! The worker fetch policy (§12 anonymity hard rule, §5.2). Not
//! configurable down: config can only add hosts to `block_hosts` or keep
//! `allow_http` off.
//!
//! Workers have no network; every GET they want is a request to the parent,
//! checked here before anything leaves the machine. What the parent then
//! sends is fixed by [`crate::online::fetch`]: a plain HTTP/1.1 GET (no JS
//! engine, no browser), pinned Tor-Browser-like `User-Agent` / `Accept` /
//! `Accept-Language: en-US`, no `Referer`, no cookies, no cache or
//! revalidation, through Tor SOCKS5 with the host name resolved by Tor
//! (`socks5h`: no local DNS), on a fresh circuit per worker (SOCKS
//! user/password isolation with random per-worker credentials).

use crate::online::{IdentityDenylist, Url, UrlError};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A request a worker must never make. Every violation kills the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation(pub String);

/// Query keys that pick a language / region; only generic `en` / `en-US`
/// (or the US region) may be sent.
const LOCALE_KEYS: &[&str] = &[
    "hl", "lang", "language", "locale", "gl", "lr", "cr", "setlang", "mkt",
];

fn locale_ok(v: &str) -> bool {
    let v = crate::online::url::percent_decode(v).to_ascii_lowercase();
    matches!(
        v.as_str(),
        "" | "en" | "en-us" | "en_us" | "us" | "lang_en" | "countryus" | "en-us,en"
    )
}

/// What one job may fetch.
#[derive(Debug, Clone)]
pub struct FetchPolicy {
    /// Hosts of the planned addresses (exact host match).
    pub hosts: Vec<String>,
    pub deny: IdentityDenylist,
    pub block_hosts: Vec<String>,
    pub allow_http: bool,
    /// The canary planted in the worker's task.
    pub canary: String,
}

impl FetchPolicy {
    fn blocked(&self, host: &str) -> bool {
        self.block_hosts
            .iter()
            .any(|b| host == b || host.ends_with(&format!(".{b}")))
    }

    /// Vets a planned address (before the job starts) and returns it
    /// cleaned (tracking parameters stripped).
    pub fn vet_planned(&self, u: &Url) -> Result<Url, Violation> {
        let u = u.strip_tracking();
        if !u.is_https() && !self.allow_http {
            return Err(Violation(format!(
                "{u} is plain http:// ([learn] allow_http = false)"
            )));
        }
        if self.blocked(u.host()) {
            return Err(Violation(format!("{} is on [learn] block_hosts", u.host())));
        }
        let s = u.to_string();
        if let Some(w) = self.deny.hit(&s) {
            return Err(Violation(format!("identifier: the address would send {w}")));
        }
        if let Some((_, q)) = s.split_once('?') {
            for kv in q.split('&') {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                if LOCALE_KEYS.contains(&k.to_ascii_lowercase().as_str()) && !locale_ok(v) {
                    return Err(Violation(format!("locale: {k}={v} is not generic en-US")));
                }
            }
        }
        Ok(u)
    }

    /// Checks one GET a worker asks for. `Err` = kill + quarantine.
    pub fn check(&self, raw: &str) -> Result<Url, Violation> {
        if !self.canary.is_empty() && raw.contains(&self.canary) {
            return Err(Violation(
                "canary: the planted canary showed up in a request".into(),
            ));
        }
        let u = match Url::parse(raw) {
            Ok(u) => u,
            Err(UrlError::Local) => {
                return Err(Violation(format!(
                    "lan: {raw:?} reaches loopback / Ko's network"
                )))
            }
            Err(e) => return Err(Violation(format!("bad address {raw:?}: {e}"))),
        };
        if !self.hosts.iter().any(|h| h == u.host()) {
            return Err(Violation(format!("unexpected domain: {}", u.host())));
        }
        self.vet_planned(&u)
    }
}

impl FetchPolicy {
    /// Checks a redirect hop from `from` to `raw`: everything [`check`]
    /// does, and an `https://` → `http://` downgrade is always refused,
    /// even with `allow_http` on (a Tor exit node could read or change the
    /// rest of the chain; §5.2).
    ///
    /// [`check`]: FetchPolicy::check
    pub fn check_redirect(&self, from: &Url, raw: &str) -> Result<Url, Violation> {
        let to = self.check(raw)?;
        if from.is_https() && !to.is_https() {
            return Err(Violation(format!(
                "https to http downgrade refused: {from} -> {to}"
            )));
        }
        Ok(to)
    }
}

/// One fetch at a time across every worker of this process, with a minimum
/// interval and random jitter on top (§11.4, §12 timing).
static LAST_FETCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Pacing of the parent's fetches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pacer {
    /// `[learn.dream] fetch_interval_s`.
    pub interval: Duration,
    /// Extra random delay in `0..=jitter`.
    pub jitter: Duration,
}

impl Pacer {
    /// From `[learn.dream] fetch_interval_s`, with up to half of it again as
    /// jitter.
    pub fn from_config(cfg: &lillith_core::config::LearnConfig) -> Self {
        let interval = Duration::from_secs(u64::from(cfg.dream.fetch_interval_s.max(1)));
        Self {
            interval,
            jitter: interval / 2,
        }
    }

    /// The delay a fetch waits after `last` (pure; `r` is a random u64).
    pub fn delay_after(&self, last: Option<Instant>, now: Instant, r: u64) -> Duration {
        let Some(last) = last else {
            return Duration::ZERO;
        };
        let j = if self.jitter.is_zero() {
            Duration::ZERO
        } else {
            let span = self.jitter.as_millis() as u64 + 1;
            Duration::from_millis(r % span)
        };
        (last + self.interval + j).saturating_duration_since(now)
    }

    /// Holds the process-wide fetch slot and runs `f` once the pacing delay
    /// is over (only one fetch runs at a time). `None` when `stop()` turned
    /// true while waiting.
    pub fn run<T>(&self, stop: &dyn Fn() -> bool, f: impl FnOnce() -> T) -> Option<T> {
        let mut slot = LAST_FETCH.lock().unwrap_or_else(|p| p.into_inner());
        let wait = self.delay_after(*slot, Instant::now(), super::random_u64());
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if stop() {
                return None;
            }
            std::thread::sleep((until - Instant::now()).min(Duration::from_millis(20)));
        }
        if stop() {
            return None;
        }
        let out = f();
        *slot = Some(Instant::now());
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> FetchPolicy {
        FetchPolicy {
            hosts: vec!["docs.example.org".into()],
            deny: IdentityDenylist::empty().with_names(["Ko Kannadian"]),
            block_hosts: vec!["blocked.example".into()],
            allow_http: false,
            canary: "cnry-7f3a9".into(),
        }
    }

    #[test]
    fn https_to_http_redirect_is_refused_even_with_allow_http() {
        for allow_http in [false, true] {
            let p = FetchPolicy {
                allow_http,
                ..policy()
            };
            let from = Url::parse("https://docs.example.org/a").unwrap();
            let v = p
                .check_redirect(&from, "http://docs.example.org/b")
                .unwrap_err();
            // Off: plain http is refused anyway; on: the downgrade rule.
            let want = if allow_http {
                "downgrade"
            } else {
                "plain http"
            };
            assert!(v.0.contains(want), "allow_http={allow_http}: {v:?}");
            assert!(p
                .check_redirect(&from, "https://docs.example.org/b")
                .is_ok());
        }
        // http -> http stays allowed only with allow_http on.
        let on = FetchPolicy {
            allow_http: true,
            ..policy()
        };
        let plain = Url::parse("http://docs.example.org/a").unwrap();
        assert!(on
            .check_redirect(&plain, "http://docs.example.org/b")
            .is_ok());
        assert!(policy()
            .check_redirect(&plain, "http://docs.example.org/b")
            .is_err());
    }

    #[test]
    fn only_planned_hosts_over_https_without_identifiers_or_locale() {
        let p = policy();
        assert!(p.check("https://docs.example.org/rust/traits").is_ok());
        let u = p
            .check("https://docs.example.org/a?utm_source=x&q=traits")
            .unwrap();
        assert_eq!(
            u.to_string(),
            "https://docs.example.org/a?q=traits",
            "tracking stripped"
        );
        for (raw, why) in [
            ("https://evil.example.net/", "unexpected domain"),
            ("https://127.0.0.1/", "lan"),
            ("https://192.168.1.10/", "lan"),
            ("https://nas.lan/", "lan"),
            ("http://localhost:8080/", "lan"),
            ("http://docs.example.org/", "http://"),
            ("https://docs.example.org/?q=kannadian", "identifier"),
            ("https://docs.example.org/home/box/x", "identifier"),
            ("https://docs.example.org/?hl=de", "locale"),
            ("https://docs.example.org/?q=cnry-7f3a9", "canary"),
        ] {
            let e = p.check(raw).expect_err(raw);
            assert!(e.0.contains(why), "{raw}: {e:?}");
        }
        assert!(p.check("https://docs.example.org/?hl=en-US").is_ok());
    }

    #[test]
    fn pacing_adds_interval_and_bounded_jitter() {
        let p = Pacer {
            interval: Duration::from_secs(20),
            jitter: Duration::from_secs(10),
        };
        let t = Instant::now();
        assert_eq!(p.delay_after(None, t, 7), Duration::ZERO, "first fetch");
        for r in [0u64, 1, 999, 10_000, u64::MAX] {
            let d = p.delay_after(Some(t), t, r);
            assert!(
                d >= Duration::from_secs(20) && d <= Duration::from_secs(30),
                "{d:?}"
            );
        }
        assert_ne!(
            p.delay_after(Some(t), t, 1234),
            p.delay_after(Some(t), t, 5678),
            "jitter varies"
        );
        let cfg = lillith_core::config::LearnConfig::default();
        let d = Pacer::from_config(&cfg);
        assert_eq!(
            d.interval,
            Duration::from_secs(20),
            "[learn.dream] fetch_interval_s"
        );
        assert_eq!(d.jitter, Duration::from_secs(10));
    }
}
