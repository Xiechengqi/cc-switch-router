//! English-only transactional receipts. The template admits structured facts,
//! never seller descriptions, notes, raw gateway errors or payment secrets.
//! Hallmark · component: financial receipt · genre: modern-minimal
//! theme: existing sign-in branding · hierarchy: outcome / amount / facts / link
//! pre-emit critique: P5 H5 E4 S5 R5 V4
use super::{AppError, Connection, OptionalExtension, db_error, params};
use super::{Claimed, escape_html};
use chrono::{DateTime, Utc};
use serde_json::Value;
use serde_json::json;

pub(super) fn enrich_service_tx(conn: &Connection, item: &mut Claimed) -> Result<(), AppError> {
    let Some(id) = item.payload.get("contractId").and_then(Value::as_str) else {
        return Ok(());
    };
    let row: Option<(String,String,Option<String>)>=conn.query_row(
        "SELECT c.product_kind,COALESCE(c.service_ref,c.product_ref),COALESCE(s.share_name,j.subdomain)
         FROM market_platform_contracts c
         LEFT JOIN share_market_subscriptions rental ON c.product_kind='share' AND rental.id=c.product_ref
         LEFT JOIN shares s ON s.share_id=rental.share_id
         LEFT JOIN provisioning_jobs j ON c.product_kind='client_host' AND j.id=c.product_ref
         WHERE c.id=?1 AND c.buyer_user_id=?2",params![id,item.recipient_user_id],
        |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(db_error("describe service notification"))?;
    if let Some((kind, reference, name)) = row {
        let name = name
            .filter(|s| s.is_ascii() && !s.chars().any(char::is_control) && !s.trim().is_empty());
        item.payload["serviceName"] = json!(name.unwrap_or_else(|| {
            format!(
                "{} {}",
                if kind == "share" {
                    "Share"
                } else {
                    "Client hosting"
                },
                reference
                    .chars()
                    .filter(|c| c.is_ascii() && !c.is_control())
                    .take(100)
                    .collect::<String>()
            )
        }));
    }
    Ok(())
}

fn field<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str).filter(|s| {
        !s.is_empty() && s.len() <= 320 && s.is_ascii() && !s.chars().any(char::is_control)
    })
}
fn minor(amount: i64, currency: &str) -> String {
    let n = i128::from(amount);
    let sign = if n < 0 { "-" } else { "" };
    let n = n.abs();
    format!("{sign}{currency} {}.{:02}", n / 100, n % 100)
}
pub(super) fn amount(payload: &Value) -> Option<String> {
    if let Some(n) = payload
        .get("amountUnits")
        .and_then(Value::as_str)
        .and_then(|v| v.parse::<i64>().ok())
    {
        let n = i128::from(n);
        let sign = if n < 0 { "-" } else { "" };
        let n = n.abs();
        let dollars = n / 8_640_000;
        let remainder = n % 8_640_000;
        let micros = remainder * 1_000_000 / 8_640_000;
        if dollars == 0 && remainder > 0 && micros == 0 {
            return Some(format!("{sign}<USD 0.000001"));
        }
        let fraction = format!("{micros:06}");
        let fraction = fraction.trim_end_matches('0');
        return Some(format!("{sign}USD {dollars}.{:0<2}", fraction));
    }
    payload
        .get("amountMinor")
        .and_then(Value::as_i64)
        .map(|n| minor(n, "USD"))
}

fn destination(item: &Claimed, dashboard: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(dashboard).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    if item.event_kind.starts_with("client_termination_")
        && let Some(id) = field(&item.payload, "terminationId")
    {
        url.set_path("/client-market/");
        url.query_pairs_mut().append_pair("terminationId", id);
        return Some(url.to_string());
    }
    // Very old notifications may lack a case ID. The records now live with
    // the account's Client rentals (or, for Providers, what they rent out),
    // not on the wallet or Provider income tab.
    if item.event_kind.starts_with("client_termination_") {
        let role = field(&item.payload, "role");
        if role == Some("handler") {
            url.set_path("/client-market/");
        } else if role == Some("provider") {
            url.set_path("/account/provider-settings/");
            url.query_pairs_mut()
                .append_pair("tab", "listings")
                .append_pair("market", "client");
        } else {
            url.set_path("/account/rentals/");
            url.query_pairs_mut().append_pair("market", "client");
        }
        if role != Some("handler")
            && let Some(id) = field(&item.payload, "installationId")
        {
            url.query_pairs_mut().append_pair("focus", id);
        }
        return Some(url.to_string());
    }
    let provider = matches!(
        item.event_kind.as_str(),
        "earnings_converted" | "earnings_held" | "earnings_released" | "commission_policy_changed"
    );
    let handler = field(&item.payload, "role") == Some("handler");
    url.set_path(if provider {
        "/account/provider-settings/"
    } else if handler {
        "/settings/"
    } else {
        "/account/wallet/"
    });
    if provider {
        url.query_pairs_mut().append_pair("tab", "income");
    }
    if handler {
        url.query_pairs_mut().append_pair("panel", "market_finance");
    }
    if let Some(id) = field(&item.payload, "orderId") {
        let key = if item.event_kind.starts_with("c2c_") {
            "c2cOrder"
        } else {
            "topup"
        };
        url.query_pairs_mut().append_pair(key, id);
    }
    Some(url.to_string())
}

pub(super) fn render(item: &Claimed, dashboard: &str) -> (String, String, String) {
    let p = &item.payload;
    let role = field(p, "role").unwrap_or("");
    let action = field(p, "action").unwrap_or("");
    let (title, intro) = match item.event_kind.as_str() {
        "client_termination_claim" => (
            "Client termination compensation claim",
            if action == "responded" {
                "The Provider has formally responded. Any remaining contested compensation stays held for an Owner decision, unless the buyer voluntarily ends the claim. Chat messages alone do not decide the financial outcome."
            } else if role == "provider" {
                "The buyer has requested a compensation waiver. Respond by reducing or waiving compensation, or formally request an Owner decision before the response deadline. If you do not respond within 72 hours, the remaining compensation is automatically waived. Open the Client chat from the termination record to discuss the issue."
            } else {
                "Your compensation claim is open. Only the contested compensation is held; other unused rent returns after verified cleanup. If the Provider does not formally respond within 72 hours, the remaining compensation is waived. You may discuss the issue in the Client chat or voluntarily end your claim from the termination record."
            },
        ),
        "client_termination_settled" => (
            "Client rental termination settlement",
            if role == "provider" {
                "The Client rental has ended. Any termination compensation is separate from service income, carries no service commission and moves to your wallet automatically after its seven-day settlement window. Disputed amounts remain held for review."
            } else {
                "The Client rental has ended. The amount shown is the total unused rent returned as platform credit for this termination, not a new top-up or an external cash refund. Open the termination record to review the settlement."
            },
        ),
        "topup_credited" => (
            "Top-up credited",
            "Your top-up has been credited to your platform wallet and is ready to use.",
        ),
        "topup_closed_unpaid" => (
            "Top-up closed",
            "This order closed without a confirmed payment. No credit was added to your wallet.",
        ),
        "topup_review_required" => (
            "Payment needs review",
            "Payment evidence for this order needs checking. Do not pay for this order again. Review the order or contact your Router administrator.",
        ),
        "topup_reversed" => (
            "Wallet credit adjusted",
            "An externally confirmed payment reversal was recorded. The corresponding wallet credit has been adjusted. This is not a platform cash-refund service.",
        ),
        "wallet_transfer_sent" => (
            "Transfer sent",
            "Your platform credit was transferred to the recipient below. This transfer cannot be undone.",
        ),
        "wallet_transfer_received" => (
            "Transfer received",
            "Platform credit was transferred to your wallet and is ready to use.",
        ),
        "c2c_order_updated" => match (action, field(p, "status").unwrap_or(""), role) {
            ("release_reminder", _, "seller") => (
                "C2C payment awaiting your review",
                "The buyer previously reported payment. Check your payment account and complete the order. The platform never releases credit automatically.",
            ),
            (_, "released", _) => (
                "C2C trade completed",
                if role == "seller" {
                    "Reserved platform credit was transferred from your wallet to the buyer."
                } else {
                    "Reserved platform credit was transferred to your wallet. The trade is complete."
                },
            ),
            ("overdue_escalated", _, _) => (
                "C2C order escalated",
                "This unresolved payment-declared order has been escalated for independent review. Reserved credit remains protected.",
            ),
            (_, "paid", "seller") => (
                "C2C buyer reported payment",
                "Check actual receipt in your own payment account before releasing credit. A buyer's payment declaration is not proof of receipt.",
            ),
            (_, "disputed", _) | ("late_payment_reported", _, _) => (
                "C2C order needs review",
                if role == "handler" {
                    "An order requires independent review. Review the evidence in the management page; do not act on payment claims in an email."
                } else {
                    "An issue was reported for this order. Review the evidence in the order. Reserved credit will not expire automatically."
                },
            ),
            (_, "awaiting_payment", _) => (
                "C2C order ready",
                "Credit is reserved. Review the live order and its payment window before taking any action.",
            ),
            (_, "cancelled" | "expired", _) => (
                "C2C order closed",
                "This order is no longer payable. Review the order if you already made an external payment.",
            ),
            _ => (
                "C2C order updated",
                "Review the current order after signing in before taking any action.",
            ),
        },
        "c2c_seller_changed" => match field(p, "reason") {
            Some("paid_order_overdue") => (
                "C2C selling automatically paused",
                "A payment-declared order is overdue. New orders are paused. Check your existing orders, verify receipt and explicitly resume selling when ready.",
            ),
            Some("suspended") => (
                "C2C selling suspended",
                "An administrator suspended new C2C sales. Existing orders still need handling. Contact your Router administrator for details.",
            ),
            Some("unsuspended") => (
                "C2C selling suspension lifted",
                "An administrator lifted your selling suspension. New sales remain disabled until you explicitly enable selling. Existing orders still need handling.",
            ),
            Some("auto_accept_consent_required") => (
                "Confirm C2C selling terms",
                "Review and confirm the automatic reservation terms in your wallet to continue selling. Existing orders remain available.",
            ),
            _ => (
                "C2C seller settings updated",
                "Review your current selling settings and existing orders in your wallet.",
            ),
        },
        "commission_policy_changed" => (
            "Provider service fee updated",
            "The rate below applies to new service periods from the effective time. Committed periods stay unchanged. Automatic income credit has no additional service fee.",
        ),
        "earnings_converted" => (
            "Earnings credited to wallet",
            "Matured provider earnings are available in your platform wallet for services, C2C trades or transfers. Cash withdrawal is not provided.",
        ),
        "earnings_held" => (
            "Provider earnings on hold",
            "Affected earnings are held while the related issue is reviewed. Review your income records for details.",
        ),
        "earnings_released" => (
            "Provider earnings hold released",
            "The related earnings hold was released. Eligible matured earnings are credited to your wallet automatically.",
        ),
        "wallet_low_runway" | "renewal_upcoming_shortfall" => (
            "Wallet funding needed",
            "Your available balance may not cover upcoming renewals or continued service. Review the current funding needs in your wallet.",
        ),
        "contract_suspended" => (
            "Service paused",
            "A service was paused. Review the current status and funding requirements in your account.",
        ),
        "service_status_summary" => (
            "Service status summary",
            "Several service updates were combined to avoid repeated emails. The snapshot below is not a new charge. Sign in for the current status.",
        ),
        "contract_resumed" => (
            "Service resumed",
            "Your service is active again. Review your account for the latest service status.",
        ),
        "contract_terminating_soon" => (
            "Service ending soon",
            "A paused service is approaching termination. Review the service and resolve its funding requirements before the deadline.",
        ),
        "contract_terminated" => (
            "Service ended",
            "A service has ended. Review its final status and records in your account.",
        ),
        "dispute_resolved" => (
            "Service dispute resolved",
            "A decision was recorded for your service dispute. Review the decision and any platform-credit adjustment in your account.",
        ),
        // Read-only historical events never revive a retired cash-refund flow.
        "refund_status" => (
            "Historical wallet adjustment",
            "Review the historical adjustment in your wallet records. Platform cash refunds are not available.",
        ),
        _ => (
            "Wallet update",
            "Review the latest wallet activity after signing in.",
        ),
    };
    let mut rows: Vec<(&str, String)> = Vec::new();
    if let Some(name) = field(p, "serviceName") {
        rows.push(("Service name", name.chars().take(160).collect()));
    }
    if item.event_kind == "contract_suspended" {
        let reason = match field(p, "reason") {
            Some("insufficient_funds") => "Insufficient available wallet balance",
            Some("finance_hold") => "Service is on hold for financial review",
            _ => "Service is paused; check your account for details",
        };
        rows.push(("Reason", reason.into()));
    }
    for (key, label) in [
        ("suspensionStartedAt", "Pause started"),
        ("statusAsOf", "Status as of"),
    ] {
        if let Some(at) = field(p, key).and_then(|s| DateTime::parse_from_rfc3339(s).ok()) {
            rows.push((
                label,
                at.with_timezone(&Utc)
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string(),
            ));
        }
    }
    if item.event_kind == "service_status_summary" {
        for (key, label) in [
            ("pausedCount", "Paused services"),
            ("activeCount", "Active services"),
        ] {
            if let Some(n) = p.get(key).and_then(Value::as_i64) {
                rows.push((label, n.to_string()));
            }
        }
    }
    if super::safety::is_service(super::delivery::purpose(item).unwrap_or("")) {
        rows.push(("Notification reference", item.id.clone()));
    }
    if let Some(n) = p.get("feeMinor").and_then(Value::as_i64) {
        rows.push(("Maintenance fee", minor(n, "USD")));
    }
    if let Some(n) = p.get("payMinor").and_then(Value::as_i64) {
        let currency = field(p, "currency")
            .filter(|c| matches!(*c, "USD" | "CNY"))
            .unwrap_or("USD");
        rows.push((
            if item.event_kind.starts_with("c2c_") {
                "External payment total"
            } else {
                "Payment total"
            },
            minor(n, currency),
        ));
    }
    for (key, label) in [
        ("recipientEmail", "Recipient"),
        ("senderEmail", "Sender"),
        ("orderId", "Order"),
        ("transferId", "Transfer"),
        ("conversionId", "Reference"),
        ("contractId", "Service"),
    ] {
        if key == "senderEmail" && item.event_kind == "wallet_transfer_sent"
            || key == "recipientEmail" && item.event_kind == "wallet_transfer_received"
        {
            continue;
        }
        if let Some(value) = field(p, key) {
            rows.push((label, value.into()));
        }
    }
    if item.event_kind == "commission_policy_changed" {
        if let Some(bps) = p
            .get("rateBps")
            .and_then(Value::as_i64)
            .filter(|n| (0..=10000).contains(n))
        {
            rows.push(("Service fee", format!("{}.{:02}%", bps / 100, bps % 100)));
        }
        if let Some(time) = field(p, "effectiveAt") {
            if let Ok(at) = chrono::DateTime::parse_from_rfc3339(time) {
                rows.push((
                    "Effective",
                    at.with_timezone(&chrono::Utc)
                        .format("%Y-%m-%d %H:%M:%S UTC")
                        .to_string(),
                ));
            }
        }
    }
    if item.event_kind == "contract_terminating_soon" {
        if let Some(time) = field(p, "terminatesAt") {
            if let Ok(at) = chrono::DateTime::parse_from_rfc3339(time) {
                rows.push((
                    "Service deadline",
                    at.with_timezone(&chrono::Utc)
                        .format("%Y-%m-%d %H:%M:%S UTC")
                        .to_string(),
                ));
            }
        }
    }
    if let Some(count) = p
        .get("relatedCount")
        .and_then(Value::as_i64)
        .filter(|n| *n > 1)
    {
        rows.push(("Related services", count.to_string()));
    }
    if let Some(time) = field(p, "notificationTime") {
        if let Ok(at) = chrono::DateTime::parse_from_rfc3339(time) {
            rows.push((
                if super::safety::is_service(super::delivery::purpose(item).unwrap_or("")) {
                    "Event occurred"
                } else {
                    "Recorded"
                },
                at.with_timezone(&chrono::Utc)
                    .format("%Y-%m-%d %H:%M:%S UTC")
                    .to_string(),
            ));
        }
    }
    if matches!(
        item.event_kind.as_str(),
        "client_termination_settled" | "client_termination_claim"
    ) {
        if let Some(reference) = field(p, "clientReference") {
            rows.push(("Client", reference.into()));
        }
        if let Some(deadline) = field(p, "responseDeadline") {
            rows.push(("Provider response deadline (72 hours)", deadline.into()));
        }
        for (key, label) in [
            ("walletReturnUnits", "Total wallet return"),
            ("compensationUnits", "Total Provider compensation"),
        ] {
            if let Some(raw) = field(p, key)
                && let Some(value) = amount(&json!({"amountUnits":raw}))
            {
                rows.push((label, value));
            }
        }
    }
    let amount = amount(p);
    let amount_html=amount.as_ref().map(|n|format!("<p style=\"margin:0 0 24px;font-size:32px;line-height:1.2;font-weight:700;font-variant-numeric:tabular-nums;color:#0f172a;overflow-wrap:anywhere\">{}</p>",escape_html(n))).unwrap_or_default();
    let facts=rows.iter().map(|(k,v)|format!("<tr><th align=\"left\" valign=\"top\" style=\"padding:8px 16px 8px 0;color:#475569;font-size:13px;font-weight:400\">{}</th><td align=\"right\" style=\"padding:8px 0;color:#0f172a;font-size:14px;word-break:break-all\">{}</td></tr>",escape_html(k),escape_html(v))).collect::<String>();
    let url = destination(item, dashboard);
    let cta = if item.event_kind.starts_with("client_termination_") {
        "View termination"
    } else if item.event_kind.starts_with("c2c_order") {
        "View order"
    } else if item.event_kind.starts_with("topup_") {
        "View top-up"
    } else if item.event_kind.starts_with("earnings_")
        || item.event_kind == "commission_policy_changed"
        || matches!(
            item.event_kind.as_str(),
            "client_termination_settled" | "client_termination_claim"
        ) && role == "provider"
    {
        "View provider income"
    } else {
        "View wallet"
    };
    let link_html=url.as_ref().map(|url|format!("<a href=\"{}\" style=\"display:inline-block;margin-top:24px;padding:12px 20px;border-radius:8px;background:#2563eb;color:#ffffff;text-decoration:none;font-size:14px;font-weight:600;white-space:nowrap\">{cta}</a>",escape_html(url))).unwrap_or_default();
    let body = format!(
        "{amount_html}<p style=\"margin:0 0 24px;color:#475569;font-size:15px;line-height:1.6\">{}</p><table role=\"presentation\" width=\"100%\" cellspacing=\"0\" cellpadding=\"0\">{facts}</table>{link_html}",
        escape_html(intro)
    );
    let outgoing = item.event_kind == "wallet_transfer_sent"
        || item.event_kind == "c2c_order_updated"
            && role == "seller"
            && field(p, "status") == Some("released");
    let footer = if outgoing {
        "If you did not authorize this transfer, secure your account and contact your Router administrator immediately."
    } else {
        "TokenSwitch account notification. Sign in to review the latest details. Never share your sign-in codes or payment credentials."
    };
    let html = crate::store::render_email_layout_with_footer(title, intro, &body, footer);
    let facts = rows
        .iter()
        .map(|(k, v)| format!("{k}: {v}"))
        .collect::<Vec<_>>()
        .join("\n");
    let text = format!(
        "TokenSwitch\n\n{title}\n\n{}\n\n{intro}\n\n{facts}\n\n{cta}: {}\n\n{footer}",
        amount.unwrap_or_default(),
        url.as_deref().unwrap_or("Sign in to your Router account")
    );
    (format!("TokenSwitch: {title}"), html, text)
}
