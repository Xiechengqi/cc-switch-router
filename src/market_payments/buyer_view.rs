//! The buyer-facing top-up projection. Buyers see four states and at most one
//! action, derived only from persisted order data (no live gateway call), so
//! every surface (wallet page, navbar dialog, checkout) agrees on what to show.
//! Reconciliation keeps tracking every order in the background; a late payment
//! on a closed or stopped order is still credited by the existing workers.
use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BuyerTopupView {
    /// `awaiting_payment` | `confirming` | `credited` | `closed`.
    pub state: &'static str,
    pub can_pay: bool,
    pub can_close: bool,
    /// The Router's automatic check time while the order is payable.
    pub pay_until: Option<String>,
}

pub(super) struct BuyerTopupFacts<'a> {
    pub status: &'a str,
    pub buyer_abandoned: bool,
    /// A hosted checkout URL or direct payment instructions are currently
    /// exposed (already masked for stop, cancel request and open review).
    pub has_payment_handle: bool,
    pub last_gateway_status: Option<&'a str>,
    pub auto_close_at: Option<&'a str>,
}

const FINAL_CLOSED: [&str; 2] = ["expired", "cancelled"];
const CREATING: [&str; 3] = ["created", "creating", "create_unknown"];

fn view(state: &'static str, can_close: bool) -> BuyerTopupView {
    BuyerTopupView {
        state,
        can_pay: false,
        can_close,
        pay_until: None,
    }
}

pub(super) fn buyer_topup_view(facts: &BuyerTopupFacts<'_>, now: DateTime<Utc>) -> BuyerTopupView {
    if facts.status == "credited" {
        return view("credited", false);
    }
    if FINAL_CLOSED.contains(&facts.status) {
        return view("closed", false);
    }
    // Stopped by the buyer: no payment entry, but a late payment still credits.
    if facts.buyer_abandoned {
        return view("confirming", false);
    }
    if CREATING.contains(&facts.status) {
        // No payment handle yet; the buyer may still decide not to pay.
        return view("confirming", true);
    }
    if facts.status != "pending" {
        // verifying, review_required and any unknown state: the platform owns it.
        return view("confirming", false);
    }
    let gateway_progressing = matches!(
        facts.last_gateway_status,
        Some("confirming" | "paid" | "expired" | "canceled" | "cancelled")
    );
    let deadline_open = match facts.auto_close_at {
        None => true,
        Some(raw) => DateTime::parse_from_rfc3339(raw)
            .map(|deadline| deadline.with_timezone(&Utc) > now)
            .unwrap_or(false),
    };
    if gateway_progressing || !facts.has_payment_handle || !deadline_open {
        return view("confirming", false);
    }
    BuyerTopupView {
        state: "awaiting_payment",
        can_pay: true,
        can_close: true,
        pay_until: facts.auto_close_at.map(str::to_owned),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn facts<'a>(status: &'a str) -> BuyerTopupFacts<'a> {
        BuyerTopupFacts {
            status,
            buyer_abandoned: false,
            has_payment_handle: true,
            last_gateway_status: Some("pending"),
            auto_close_at: Some("2026-11-20T12:30:00Z"),
        }
    }

    const NOW: &str = "2026-11-20T12:00:00Z";

    #[test]
    fn payable_pending_order_offers_pay_and_close_until_the_auto_check() {
        let value = buyer_topup_view(&facts("pending"), at(NOW));
        assert_eq!(
            value,
            BuyerTopupView {
                state: "awaiting_payment",
                can_pay: true,
                can_close: true,
                pay_until: Some("2026-11-20T12:30:00Z".into()),
            }
        );
        let mut no_deadline = facts("pending");
        no_deadline.auto_close_at = None;
        assert!(buyer_topup_view(&no_deadline, at(NOW)).can_pay);
    }

    #[test]
    fn terminal_states_never_offer_an_action() {
        for (status, state) in [
            ("credited", "credited"),
            ("expired", "closed"),
            ("cancelled", "closed"),
        ] {
            let mut input = facts(status);
            input.buyer_abandoned = true;
            assert_eq!(buyer_topup_view(&input, at(NOW)), view(state, false));
        }
    }

    #[test]
    fn platform_owned_states_are_confirming_without_actions() {
        for status in ["verifying", "review_required", "something_new"] {
            assert_eq!(
                buyer_topup_view(&facts(status), at(NOW)),
                view("confirming", false)
            );
        }
        let mut stopped = facts("pending");
        stopped.buyer_abandoned = true;
        assert_eq!(
            buyer_topup_view(&stopped, at(NOW)),
            view("confirming", false)
        );
    }

    #[test]
    fn creating_orders_can_still_be_closed_but_not_paid() {
        for status in ["created", "creating", "create_unknown"] {
            assert_eq!(
                buyer_topup_view(&facts(status), at(NOW)),
                view("confirming", true)
            );
        }
    }

    #[test]
    fn pending_order_stops_being_payable_on_any_persisted_doubt() {
        for gateway in ["confirming", "paid", "expired", "canceled", "cancelled"] {
            let mut input = facts("pending");
            input.last_gateway_status = Some(gateway);
            assert_eq!(buyer_topup_view(&input, at(NOW)), view("confirming", false));
        }
        let mut hidden = facts("pending");
        hidden.has_payment_handle = false;
        assert_eq!(
            buyer_topup_view(&hidden, at(NOW)),
            view("confirming", false)
        );
        let mut late = facts("pending");
        late.auto_close_at = Some("2026-11-20T11:59:59Z".into());
        assert_eq!(buyer_topup_view(&late, at(NOW)), view("confirming", false));
        let mut garbled = facts("pending");
        garbled.auto_close_at = Some("not-a-time".into());
        assert!(!buyer_topup_view(&garbled, at(NOW)).can_pay);
        let mut unobserved = facts("pending");
        unobserved.last_gateway_status = None;
        assert!(buyer_topup_view(&unobserved, at(NOW)).can_pay);
    }
}
