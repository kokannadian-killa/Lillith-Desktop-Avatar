//! Desktop notification monitor → `notify.received`, `calendar.reminder`,
//! `update.available`.
//!
//! A dedicated session-bus connection calls `BecomeMonitor` with a match
//! rule for `org.freedesktop.Notifications.Notify` method calls, so Lillith
//! sees every notification any app sends (plasmashell still shows them).
//! Only `app_name` and `summary` are used; the body is never read, logged
//! or stored. The summary becomes the `{summary}` of the rule line.

use crate::Sink;
use futures_util::StreamExt;
use lillith_core::Event;
use std::collections::HashMap;
use zbus::zvariant::OwnedValue;

/// Interface + member we monitor.
pub const INTERFACE: &str = "org.freedesktop.Notifications";
pub const MEMBER: &str = "Notify";

/// The `Notify` arguments we need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub app_name: String,
    pub summary: String,
}

/// Maps a notification to an event name, or `None` to ignore it.
pub fn classify(n: &Notification) -> Option<&'static str> {
    let app = n.app_name.to_ascii_lowercase();
    let summary = n.summary.to_ascii_lowercase();
    // Our own (future) notifications and empty pings.
    if app.contains("lillith") || (n.summary.trim().is_empty() && app.is_empty()) {
        return None;
    }
    const CALENDAR_APPS: [&str; 6] = [
        "korganizer",
        "merkuro",
        "kalendar",
        "kalendarac", // KDE reminder daemon
        "evolution-alarm",
        "gnome-calendar",
    ];
    if CALENDAR_APPS.iter().any(|a| app.contains(a))
        || summary.starts_with("reminder:")
        || (app.contains("thunderbird") && summary.contains("reminder"))
    {
        return Some("calendar.reminder");
    }
    const UPDATE_APPS: [&str; 7] = [
        "discover",
        "pamac",
        "octopi",
        "arch-update",
        "garuda",
        "update-notifier",
        "packagekit",
    ];
    let mentions_update = summary.contains("update") || summary.contains("upgrade");
    if mentions_update && (UPDATE_APPS.iter().any(|a| app.contains(a)) || app.is_empty()) {
        return Some("update.available");
    }
    Some("notify.received")
}

/// Builds the event for a notification.
pub fn to_event(n: &Notification) -> Option<Event> {
    let name = classify(n)?;
    let summary = n.summary.trim();
    Some(if summary.is_empty() {
        Event::new(name)
    } else {
        // Keep bubbles short.
        let short: String = summary.chars().take(80).collect();
        Event::with_detail(name, short)
    })
}

type NotifyArgs = (
    String,
    u32,
    String,
    String,
    String,
    Vec<String>,
    HashMap<String, OwnedValue>,
    i32,
);

/// Runs until the bus connection closes. Setup errors are returned so the
/// caller can log them once.
pub async fn run(sink: Sink) -> zbus::Result<()> {
    let conn = zbus::connection::Builder::session()?.build().await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::MethodCall)
        .interface(INTERFACE)?
        .member(MEMBER)?
        .build();
    zbus::fdo::MonitoringProxy::new(&conn)
        .await?
        .become_monitor(&[rule], 0)
        .await?;
    tracing::info!("notification monitor: watching {INTERFACE}.{MEMBER}");
    let mut stream = zbus::MessageStream::from(&conn);
    while let Some(msg) = stream.next().await {
        let Ok(msg) = msg else { continue };
        if msg.header().member().map(|m| m.as_str()) != Some(MEMBER) {
            continue;
        }
        let Ok(args) = msg.body().deserialize::<NotifyArgs>() else {
            continue;
        };
        let n = Notification {
            app_name: args.0,
            summary: args.3,
        };
        if let Some(ev) = to_event(&n) {
            tracing::debug!(app = %n.app_name, event = %ev.name, "notification");
            sink(ev);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(app: &str, summary: &str) -> Notification {
        Notification {
            app_name: app.into(),
            summary: summary.into(),
        }
    }

    #[test]
    fn classifies() {
        assert_eq!(
            classify(&n("Thunderbird", "New mail from Sam")),
            Some("notify.received")
        );
        assert_eq!(
            classify(&n("kalendarac", "Stand-up in 5 minutes")),
            Some("calendar.reminder")
        );
        assert_eq!(
            classify(&n("Merkuro", "Dentist")),
            Some("calendar.reminder")
        );
        assert_eq!(
            classify(&n("Discover", "12 updates available")),
            Some("update.available")
        );
        assert_eq!(
            classify(&n("garuda-update", "System upgrade ready")),
            Some("update.available")
        );
        assert_eq!(
            classify(&n("Firefox", "Update your password")),
            Some("notify.received"),
            "the word alone is not enough"
        );
        assert_eq!(classify(&n("lillith", "hi")), None);
        assert_eq!(classify(&n("", "  ")), None);
    }

    #[test]
    fn events_carry_short_summary() {
        let ev = to_event(&n("Signal", "Ko: are you there?")).unwrap();
        assert_eq!(
            ev,
            Event::with_detail("notify.received", "Ko: are you there?")
        );
        let long = "x".repeat(200);
        assert_eq!(to_event(&n("a", &long)).unwrap().detail.unwrap().len(), 80);
        assert_eq!(
            to_event(&n("app", "")).unwrap(),
            Event::new("notify.received")
        );
    }

    #[test]
    fn events_have_rules() {
        let rules = lillith_core::rules::RuleSet::bundled();
        for e in ["notify.received", "calendar.reminder", "update.available"] {
            assert!(rules.find(e).is_some(), "{e}");
        }
    }
}
