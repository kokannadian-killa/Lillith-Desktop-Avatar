//! Wire QuantumSec shield events into `lillith-memory` habit logs.
//!
//! Keeps quantumsec free of a hard `lillith-memory` dependency: callers use
//! [`event_for_shield`] / [`habit_for_shield`] with `Brain::log_event` from
//! the app crate. Additive: extends what memory can see without changing
//! existing observe paths.

use crate::shield::ShieldEvent;
use crate::status::StatusLevel;
use lillith_core::Event;

/// Map a shield event to a desktop [`Event`] (for rules / future observe branches).
///
/// Names are `quantumsec.<kind>` so they never collide with window.* / chat.*.
pub fn event_for_shield(ev: &ShieldEvent) -> Option<Event> {
    match ev {
        ShieldEvent::Status(status) => Some(Event::with_detail(
            format!("quantumsec.status.{}", level_slug(status.level)),
            status.summary.clone(),
        )),
        ShieldEvent::LockdownProposed(p) => Some(Event::with_detail(
            "quantumsec.lockdown.proposed",
            p.summary.clone(),
        )),
        ShieldEvent::LockdownExecuted { id, commands_run } => Some(Event::with_detail(
            "quantumsec.lockdown.executed",
            format!("{id} ran {commands_run}"),
        )),
        ShieldEvent::LockdownRefused { id, reason } => Some(Event::with_detail(
            "quantumsec.lockdown.refused",
            format!("{id}: {reason}"),
        )),
    }
}

/// Habit-log pair `(kind, subject)` for [`lillith_memory::Brain::log_event`].
pub fn habit_for_shield(ev: &ShieldEvent) -> Option<(&'static str, String)> {
    match ev {
        ShieldEvent::Status(status) => {
            Some(("quantumsec", format!("status:{}", level_slug(status.level))))
        }
        ShieldEvent::LockdownProposed(_) => Some(("quantumsec", "lockdown_proposed".into())),
        ShieldEvent::LockdownExecuted { .. } => Some(("quantumsec", "lockdown_executed".into())),
        ShieldEvent::LockdownRefused { .. } => Some(("quantumsec", "lockdown_refused".into())),
    }
}

fn level_slug(level: StatusLevel) -> &'static str {
    match level {
        StatusLevel::Nominal => "nominal",
        StatusLevel::Elevated => "elevated",
        StatusLevel::Critical => "critical",
        StatusLevel::Paused => "paused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status::ShieldStatus;

    #[test]
    fn status_and_lockdown_map() {
        let st = ShieldStatus {
            level: StatusLevel::Elevated,
            summary: "test".into(),
            protons: vec![],
            pending_lockdown: None,
            taken_unix: 1,
        };
        let ev = event_for_shield(&ShieldEvent::Status(st)).unwrap();
        assert!(ev.name.starts_with("quantumsec.status."));
        let (k, s) = habit_for_shield(&ShieldEvent::LockdownRefused {
            id: "x".into(),
            reason: "no".into(),
        })
        .unwrap();
        assert_eq!(k, "quantumsec");
        assert_eq!(s, "lockdown_refused");
    }
}
