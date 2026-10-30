//! **Blood Seal** — QuantumSec consent gate built on the Soul Contract card.
//!
//! Same mouse-only Yes discipline (`SIGN IN BLOOD`), same popup chrome.
//! Callers pass the exact command block; approve/deny logging lives in
//! `lillith-quantumsec::consent` (this module only paints and reports the
//! click). Additive: the original [`crate::egui_contract::soul_contract_popup`]
//! is unchanged.

use crate::egui_contract::{
    soul_contract_popup, ContractChoice, ContractView, Initiator,
};
use egui::{Context, Id};

/// Title shown on the blood-seal card for a lockdown.
pub const LOCKDOWN_TITLE: &str = "BLOOD SEAL";
/// Title for a bot-deploy consent.
pub const BOT_DEPLOY_TITLE: &str = "BLOOD SEAL · DEPLOY";

/// Build a [`ContractView`] for a QuantumSec gated action.
pub fn blood_seal_view<'a>(
    title: &'a str,
    summary: &'a str,
    target: Option<&'a str>,
    exact_commands: &'a str,
    busy: bool,
) -> ContractView<'a> {
    ContractView {
        title,
        summary,
        target,
        detail: Some(exact_commands),
        busy,
        initiator: Initiator::You,
        scroll_detail: true,
    }
}

/// Show the blood-seal modal (Soul Contract plate with QuantumSec copy).
///
/// Returns [`ContractChoice::Yes`] only on a primary-pointer click on
/// **SIGN IN BLOOD — YES** (never keyboard). Esc → No.
pub fn blood_seal_popup(
    ctx: &Context,
    id: Id,
    title: &str,
    summary: &str,
    target: Option<&str>,
    exact_commands: &str,
    busy: bool,
) -> Option<ContractChoice> {
    let view = blood_seal_view(title, summary, target, exact_commands, busy);
    soul_contract_popup(ctx, id, &view)
}

/// Convenience: lockdown proposal → blood seal.
pub fn lockdown_blood_seal(
    ctx: &Context,
    id: Id,
    summary: &str,
    detail_block: &str,
    local_simulation: bool,
) -> Option<ContractChoice> {
    let target = if local_simulation {
        Some("LOCAL SIMULATION ONLY")
    } else {
        Some("REAL COMMANDS — review carefully")
    };
    blood_seal_popup(
        ctx,
        id,
        LOCKDOWN_TITLE,
        summary,
        target,
        detail_block,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_marks_scroll_and_you() {
        let v = blood_seal_view(
            LOCKDOWN_TITLE,
            "Propose lockdown",
            Some("LOCAL SIMULATION ONLY"),
            "$ echo test\n$ true",
            false,
        );
        assert!(v.scroll_detail);
        assert_eq!(v.initiator, Initiator::You);
        assert_eq!(v.title, LOCKDOWN_TITLE);
        assert!(v.detail.unwrap().contains("echo"));
    }
}
