//! Notification kind registry and graded routing.
//!
//! Every operator-facing notification kind is registered exactly once here.
//! Producers (metrics, clock health, market finance, share/client lifecycle)
//! report only *facts*; this registry owns the wording, the audience tier
//! ([`NoticeLevel`]) and the flap-suppression window. Wording therefore stays
//! consistent across the in-app center, Bark and Telegram, and "should this
//! page a human" is decided in one place instead of at every call site.
//!
//! Adding a new operator alert or owner signal without registering it here is
//! a startup/compile-time error (see [`describe`]), never a silent fallback.

/// How much a notification may interrupt a human.
///
/// Delivery routing key: outbound channels (Bark/Telegram/email) declare the
/// minimum level they carry; the in-app center always receives every level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NoticeLevel {
    /// Record only: in-app center feed. Never interrupts.
    Record = 0,
    /// Attention: in-app center, surfaced. Outbound only when the channel's
    /// minimum level allows it.
    Attention = 1,
    /// Action: something is broken or money needs a decision. Outbound
    /// channels default to this minimum.
    Action = 2,
}

impl NoticeLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Attention => "attention",
            Self::Action => "action",
        }
    }

    /// Parse a stored level. Legacy severity values map onto the tier ladder:
    /// `critical` → action, `warning` → attention, `info` → record. Unknown or
    /// empty values default to action so a malformed setting fails loud
    /// (outbound), not silent.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "record" | "info" => Self::Record,
            "attention" | "warning" => Self::Attention,
            _ => Self::Action,
        }
    }

    pub fn meets(self, minimum: Self) -> bool {
        self >= minimum
    }
}

/// Alert severity is kept for the incident pipeline (escalation ordering and
/// the existing three-band model); the registry derives it from the level so
/// producers cannot pick an inconsistent pair.
pub fn severity_for_level(level: NoticeLevel) -> &'static str {
    match level {
        NoticeLevel::Action => "critical",
        NoticeLevel::Attention => "warning",
        NoticeLevel::Record => "info",
    }
}

/// One sentence of context shown under the title and a one-line suggested
/// next step. Placeholders `{field}` are filled from the alert's details /
/// payload facts at render time; missing fields render as an empty string.
pub struct NoticeCopy {
    pub title_en: &'static str,
    pub title_zh: &'static str,
    pub summary_en: &'static str,
    pub summary_zh: &'static str,
    pub action_en: Option<&'static str>,
    pub action_zh: Option<&'static str>,
}

pub struct NoticeKind {
    pub kind: &'static str,
    pub level: NoticeLevel,
    /// Minimum seconds between a resolve and a re-fire of the same
    /// fingerprint before the re-fire is folded into the existing incident
    /// instead of notifying again. Resource metrics flap; money does not.
    pub flap_suppress_secs: i64,
    pub copy: NoticeCopy,
}

macro_rules! leveled {
    ($name:ident, $level:expr) => {
        const fn $name(
            kind: &'static str,
            flap: i64,
            title_en: &'static str,
            title_zh: &'static str,
            summary_en: &'static str,
            summary_zh: &'static str,
            action_en: Option<&'static str>,
            action_zh: Option<&'static str>,
        ) -> NoticeKind {
            base(
                kind, $level, flap, title_en, title_zh, summary_en, summary_zh, action_en,
                action_zh,
            )
        }
    };
}

leveled!(record, NoticeLevel::Record);
leveled!(attention, NoticeLevel::Attention);
leveled!(action, NoticeLevel::Action);

/// `const` variant constructor; the `level` helpers below wrap it.
const fn base(
    kind: &'static str,
    level: NoticeLevel,
    flap: i64,
    title_en: &'static str,
    title_zh: &'static str,
    summary_en: &'static str,
    summary_zh: &'static str,
    action_en: Option<&'static str>,
    action_zh: Option<&'static str>,
) -> NoticeKind {
    NoticeKind {
        kind,
        level,
        flap_suppress_secs: flap,
        copy: NoticeCopy {
            title_en,
            title_zh,
            summary_en,
            summary_zh,
            action_en,
            action_zh,
        },
    }
}

const NO_FLAP: i64 = 0;
/// Resource metrics flap around thresholds; fold re-fires inside 15 minutes.
const METRIC_FLAP_SECS: i64 = 15 * 60;

/// Operator alert kinds produced from metrics (`metrics/mod.rs`) and clock
/// health (`clock_health.rs`), plus market Owner signals
/// (`market_notifications.rs`) and client-presence signals (`store.rs`).
pub const KINDS: &[NoticeKind] = &[
    // ---- Metrics: resource pressure. Attention unless sustained; flap a lot.
    attention(
        "fd_pressure",
        METRIC_FLAP_SECS,
        "Router file descriptor pressure",
        "Router 文件描述符不足",
        "FD usage reached {fdUsagePercent}% of the process limit.",
        "FD 用量已达进程上限的 {fdUsagePercent}%。",
        Some("Raise the nofile limit or restart the Router during a quiet window."),
        Some("提高 nofile 上限，或在低峰期重启 Router。"),
    ),
    attention(
        "host_cpu_pressure",
        METRIC_FLAP_SECS,
        "Host CPU pressure",
        "主机 CPU 负载过高",
        "Host CPU usage reached {cpuPercent}%.",
        "主机 CPU 使用率已达 {cpuPercent}%。",
        Some("Check for runaway processes; sustained load slows request proxying."),
        Some("检查异常进程；持续高负载会拖慢请求转发。"),
    ),
    attention(
        "host_memory_pressure",
        METRIC_FLAP_SECS,
        "Host memory pressure",
        "主机内存不足",
        "Host memory usage reached {memoryUsagePercent}%.",
        "主机内存使用率已达 {memoryUsagePercent}%。",
        Some("Free memory or resize the host before the OOM killer picks a victim."),
        Some("释放内存或扩容，避免触发 OOM。"),
    ),
    attention(
        "host_disk_pressure",
        METRIC_FLAP_SECS,
        "Host disk pressure",
        "主机磁盘空间不足",
        "Primary disk usage reached {diskUsagePercent}%.",
        "主磁盘使用率已达 {diskUsagePercent}%。",
        Some("Prune logs and old metrics; a full disk stops the business database."),
        Some("清理日志与历史数据；磁盘写满会导致业务库不可用。"),
    ),
    // ---- Metrics: data path health.
    action(
        "route_lifecycle",
        NO_FLAP,
        "SSH route lifecycle mismatch",
        "SSH 路由生命周期不一致",
        "The proxy route table and SSH forwarding state disagree ({mismatches} mismatches).",
        "代理路由表与 SSH 转发状态不一致（{mismatches} 处）。",
        Some("Requests to affected Shares may be dropped; inspect recent tunnel churn."),
        Some("受影响的 Share 请求可能被丢弃，请检查最近的隧道变动。"),
    ),
    attention(
        "route_failover",
        NO_FLAP,
        "Model route failover",
        "模型路由候选切换",
        "A request failed over from the primary Share to a fallback candidate.",
        "一次请求从主 Share 切换到了候选 Share。",
        Some("Check the primary Share's health; the circuit breaker will auto-recover."),
        Some("检查主 Share 状态；熔断器会在冷却后自动恢复。"),
    ),
    record(
        "db_error",
        METRIC_FLAP_SECS,
        "Router database errors",
        "Router 数据库错误",
        "{newErrors} new database errors (total {total}).",
        "新增 {newErrors} 个数据库错误（累计 {total}）。",
        Some("Check server logs for the failing statement."),
        Some("查看服务端日志定位失败的语句。"),
    ),
    action(
        "ssh_emfile",
        METRIC_FLAP_SECS,
        "SSH listener file descriptor exhaustion",
        "SSH 监听 FD 耗尽",
        "The SSH tunnel listener ran out of file descriptors ({total} accept failures).",
        "SSH 隧道监听器 FD 耗尽（{total} 次 accept 失败）。",
        Some("New Client tunnels cannot connect until FDs are freed."),
        Some("释放 FD 前新的 Client 隧道无法接入。"),
    ),
    record(
        "ssh_bridge_write_stall",
        METRIC_FLAP_SECS,
        "SSH bridge write progress stalled",
        "SSH 桥接写入停滞",
        "{newStalls} bridge writes stalled (total {total}).",
        "{newStalls} 次桥接写入停滞（累计 {total}）。",
        None,
        None,
    ),
    record(
        "ssh_channel_open_timeout",
        METRIC_FLAP_SECS,
        "SSH forwarded channel open timed out",
        "SSH 转发通道打开超时",
        "{newTimeouts} channel opens timed out (total {total}).",
        "{newTimeouts} 次通道打开超时（累计 {total}）。",
        None,
        None,
    ),
    record(
        "ssh_channel_open_session_error",
        METRIC_FLAP_SECS,
        "SSH session failed while opening a channel",
        "SSH 通道会话错误",
        "{newErrors} terminal session errors (total {total}).",
        "{newErrors} 次会话错误（累计 {total}）。",
        None,
        None,
    ),
    attention(
        "ssh_forward_capacity",
        METRIC_FLAP_SECS,
        "SSH forwarding capacity exhausted",
        "SSH 转发容量耗尽",
        "Forwarded connections were rejected at the limit ({total} rejections, {activeBridges} active bridges).",
        "转发连接达到上限被拒绝（累计 {total} 次，当前 {activeBridges} 条桥接）。",
        Some("Raise the forwarding limit or shed stale tunnels."),
        Some("提高转发上限或清理闲置隧道。"),
    ),
    record(
        "share_request_watchdog_release",
        METRIC_FLAP_SECS,
        "Share request leases were force-released",
        "Share 请求租约被强制释放",
        "{newReleases} requests exceeded their lifecycle deadline (total {total}).",
        "{newReleases} 个请求超过生命周期上限被回收（累计 {total}）。",
        None,
        None,
    ),
    record(
        "proxy_downstream_stall",
        METRIC_FLAP_SECS,
        "Proxy downstream delivery stalled",
        "代理下行投递停滞",
        "{newTimeouts} clients stopped consuming responses (total {total}).",
        "{newTimeouts} 个客户端停止消费响应（累计 {total}）。",
        None,
        None,
    ),
    attention(
        "llm_error_rate",
        METRIC_FLAP_SECS,
        "LLM request error rate",
        "LLM 请求错误率偏高",
        "Error rate reached {errorRate} at {rpm} requests/min.",
        "错误率达到 {errorRate}，当前每分钟 {rpm} 个请求。",
        Some("Inspect upstream provider health and recent Share runtime reports."),
        Some("检查上游供应商状态与最近的 Share 运行时上报。"),
    ),
    record(
        "llm_rate_limit",
        METRIC_FLAP_SECS,
        "LLM rate limiting",
        "LLM 触发限流",
        "Rate limits hit {rateLimitPerMinute} per minute.",
        "限流达到每分钟 {rateLimitPerMinute} 次。",
        None,
        None,
    ),
    record(
        "client_metrics_unavailable",
        METRIC_FLAP_SECS,
        "Client status metrics unavailable",
        "Client 状态指标不可用",
        "Router could not read the Client presence state.",
        "Router 无法读取 Client 在线状态。",
        None,
        None,
    ),
    // ---- Clock health.
    action(
        "router_clock_skew",
        METRIC_FLAP_SECS,
        "Router clock drift",
        "Router 时钟偏移",
        "Router clock is {direction} by {offsetSeconds}s.",
        "Router 时钟{directionZh} {offsetSeconds} 秒。",
        Some("Fix the host clock; signatures and payment verification depend on it."),
        Some("校准主机时钟；签名校验与支付核对都依赖准确时间。"),
    ),
    attention(
        "router_clock_assurance_lost",
        METRIC_FLAP_SECS,
        "Router clock assurance unavailable",
        "Router 时钟可信源不可用",
        "Only {validSources} of {totalSources} time sources answer.",
        "{totalSources} 个时间源中仅 {validSources} 个可用。",
        Some("Check outbound NTP/HTTPS reachability from the host."),
        Some("检查主机对外的 NTP/HTTPS 连通性。"),
    ),
    record(
        "router_ntp_unsynchronized",
        METRIC_FLAP_SECS,
        "Router NTP is not synchronized",
        "Router NTP 未同步",
        "External time is aligned, but the primary NTP guard is unavailable.",
        "外部时间目前对齐，但主 NTP 守护不可用。",
        None,
        None,
    ),
    // ---- Client presence (fired from store.rs presence watcher).
    attention(
        "client_offline",
        5 * 60,
        "Client offline",
        "Client 已离线",
        "Client {clientLabel} is confirmed offline.",
        "Client {clientLabel} 已确认离线。",
        Some("Check the host and restart cc-switch-server if needed."),
        Some("检查主机状态，必要时重启 cc-switch-server。"),
    ),
    record(
        "client_recovered",
        NO_FLAP,
        "Client recovered",
        "Client 已恢复",
        "Client {clientLabel} is back online.",
        "Client {clientLabel} 已恢复在线。",
        None,
        None,
    ),
    record(
        "client_removed",
        NO_FLAP,
        "Client monitoring ended",
        "Client 监控已结束",
        "Monitoring for {clientLabel} has ended.",
        "对 {clientLabel} 的监控已结束。",
        None,
        None,
    ),
    // ---- Market Owner signals (fired from market_notifications.rs).
    attention(
        "market_share_termination_finance_pending",
        NO_FLAP,
        "Share revoked; financial settlement requires review",
        "Share 已回收，财务结算待处理",
        "{summary}",
        "{summary}",
        Some("Open the finance desk and settle the terminated rental."),
        Some("打开财务工作台完成该退租的结算。"),
    ),
    action(
        "market_finance_critical_case",
        NO_FLAP,
        "Market finance reconciliation required",
        "市场财务需要人工核对",
        "{summary}",
        "{summary}",
        Some("Review the open reconciliation case before further payouts."),
        Some("在继续结算前处理未决的核对案例。"),
    ),
    action(
        "market_finance_audit_mismatch",
        NO_FLAP,
        "Market finance audit found mismatches",
        "市场财务审计发现不一致",
        "{summary}",
        "{summary}",
        Some("Reconcile the journal before the next settlement run."),
        Some("在下次结算前完成对账。"),
    ),
    action(
        "market_payment_secret_unreadable",
        NO_FLAP,
        "Payment channel secret cannot be decrypted",
        "支付渠道密钥无法解密",
        "{summary}",
        "{summary}",
        Some("Re-enter the channel credentials; top-ups are blocked until then."),
        Some("重新录入渠道凭据；在此之前充值无法处理。"),
    ),
    action(
        "market_payment_channel_auth_failed",
        NO_FLAP,
        "Payment channel credentials rejected",
        "支付渠道凭据被拒绝",
        "{summary}",
        "{summary}",
        Some("Rotate the channel credentials in payment settings."),
        Some("在支付设置中轮换渠道凭据。"),
    ),
    attention(
        "market_topup_reconcile_failing",
        NO_FLAP,
        "Top-up reconciliation keeps failing",
        "充值核对持续失败",
        "{summary}",
        "{summary}",
        Some("Check gateway reachability and recent top-up orders."),
        Some("检查支付网关连通性与近期充值订单。"),
    ),
    attention(
        "market_topup_review_stale",
        NO_FLAP,
        "Top-ups need owner review",
        "有充值等待人工核对",
        "{summary}",
        "{summary}",
        Some("Buyers are waiting; review the pending top-ups."),
        Some("买家正在等待，请核对挂起的充值。"),
    ),
    attention(
        "market_gateway_identity_conflict",
        NO_FLAP,
        "Payment gateway identity conflict",
        "支付网关身份冲突",
        "{summary}",
        "{summary}",
        Some("Verify the merchant account bound to this Router."),
        Some("确认绑定到本 Router 的商户账号。"),
    ),
    attention(
        "market_gateway_orphan_paid_order",
        NO_FLAP,
        "Paid gateway order without Router top-up",
        "网关已付款但无对应充值单",
        "{summary}",
        "{summary}",
        Some("Credit or refund the orphan order from the finance desk."),
        Some("在财务工作台为该订单补账或退款。"),
    ),
    attention(
        "market_gateway_external_refund",
        NO_FLAP,
        "Merchant recorded a gateway refund",
        "商户侧记录了网关退款",
        "{summary}",
        "{summary}",
        Some("Confirm the refund matches a Router reversal."),
        Some("确认该退款与 Router 冲正记录一致。"),
    ),
    action(
        "market_duplicate_external_reference",
        NO_FLAP,
        "Duplicate external transfer reference",
        "外部转账流水号重复",
        "{summary}",
        "{summary}",
        Some("Possible double-credit; freeze the affected wallet entries."),
        Some("可能重复入账，请冻结相关钱包记录。"),
    ),
    record(
        "market_channel_promoted_live",
        NO_FLAP,
        "Payment channel promoted to live",
        "支付渠道已上线",
        "{summary}",
        "{summary}",
        None,
        None,
    ),
    record(
        "market_refund_requested",
        NO_FLAP,
        "Buyer balance refund requested",
        "买家申请余额退款",
        "{summary}",
        "{summary}",
        Some("Review the refund request in the finance desk."),
        Some("在财务工作台处理退款申请。"),
    ),
    record(
        "market_dispute_opened",
        NO_FLAP,
        "Service dispute opened",
        "新服务争议",
        "{summary}",
        "{summary}",
        Some("Respond before the dispute deadline."),
        Some("请在争议截止前响应。"),
    ),
    attention(
        "market_c2c_dispute",
        NO_FLAP,
        "C2C balance trade needs evidence review",
        "C2C 余额交易需要审核凭证",
        "{summary}",
        "{summary}",
        Some("Review both parties' evidence and rule on the order."),
        Some("审核双方凭证并裁决该订单。"),
    ),
    attention(
        "market_client_compensation_dispute",
        NO_FLAP,
        "Client termination compensation needs review",
        "Client 退租补偿需要复核",
        "{summary}",
        "{summary}",
        Some("Decide the compensation claim before it goes overdue."),
        Some("在逾期前裁决补偿申诉。"),
    ),
    attention(
        "market_client_compensation_dispute_overdue",
        NO_FLAP,
        "Client termination compensation decision is overdue",
        "Client 退租补偿裁决已逾期",
        "{summary}",
        "{summary}",
        Some("Rule now; the renter's funds are frozen while undecided."),
        Some("请立即裁决；未决期间租用方资金处于冻结状态。"),
    ),
    attention(
        "market_client_cleanup_finalization_failed",
        NO_FLAP,
        "Client is clean but financial finalization needs review",
        "Client 已清理但财务收尾待复核",
        "{summary}",
        "{summary}",
        Some("Re-run finalization from the client market console."),
        Some("在 Client 市场控制台重试收尾。"),
    ),
    attention(
        "market_earning_hold_legacy",
        NO_FLAP,
        "Earnings hold needs review",
        "收益冻结需要复核",
        "{summary}",
        "{summary}",
        Some("Release or escalate the held earning lot."),
        Some("解冻或升级处理该笔冻结收益。"),
    ),
    attention(
        "market_earning_unlock_failed",
        NO_FLAP,
        "Provider earning lot failed to unlock",
        "Provider 收益解冻失败",
        "{summary}",
        "{summary}",
        Some("Retry the unlock; the Provider cannot spend these funds."),
        Some("重试解冻；该笔资金目前不可用。"),
    ),
    attention(
        "market_service_notice_circuit",
        NO_FLAP,
        "Service notification safety policy activated",
        "服务通知安全策略已触发",
        "{summary}",
        "{summary}",
        Some("Outbound user notifications are paused; check the diagnostics panel."),
        Some("用户外发通知已暂停，请查看诊断面板。"),
    ),
    // ---- Share control plane (share_market.rs).
    action(
        "share_control_dead_letter",
        NO_FLAP,
        "Share control operation exhausted retries",
        "Share 控制操作重试耗尽",
        "Share {shareId} control operation failed repeatedly and was parked.",
        "Share {shareId} 的控制操作多次失败，已进入死信。",
        Some("Inspect the dead letter queue; renters may keep stale access until resolved."),
        Some("检查死信队列；处理前租用方可能保留过期权限。"),
    ),
    action(
        "share_grant_contract_violation",
        NO_FLAP,
        "Share grant violated the rental contract",
        "Share 授权违反租约",
        "Share {shareId} applied an invalid entitlement; automatic revocation was queued.",
        "Share {shareId} 下发了非法授权，已自动排队回收。",
        Some("Review the Server's share descriptor sync for tampering or bugs."),
        Some("检查该 Server 的 descriptor 同步是否异常。"),
    ),
];

/// Look up a registered kind. Operator alert conditions and owner signals use
/// a `market_`-prefixed or bare kind string; both spellings resolve here.
pub fn describe(kind: &str) -> Option<&'static NoticeKind> {
    if let Some(entry) = KINDS.iter().find(|entry| entry.kind == kind) {
        return Some(entry);
    }
    KINDS
        .iter()
        .find(|entry| entry.kind.strip_prefix("market_") == Some(kind))
}

/// The level of a kind; unregistered kinds default to `Record` for the
/// in-app center (never paged) so an oversight fails quiet, not loud — while
/// `severity_for` below is strict for the operator pipeline.
pub fn level_for(kind: &str) -> NoticeLevel {
    describe(kind).map(|entry| entry.level).unwrap_or(NoticeLevel::Record)
}

pub fn flap_suppress_secs_for(kind: &str) -> i64 {
    describe(kind)
        .map(|entry| entry.flap_suppress_secs)
        .unwrap_or(NO_FLAP)
}

/// The region whose domain matches this Router's tunnel domain; empty when
/// the deployment uses a custom domain outside the built-in region list.
pub fn region_for_domain(regions_file: &str, tunnel_domain: &str) -> String {
    let domain = tunnel_domain.trim().trim_start_matches("*.").to_ascii_lowercase();
    regions_file
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(|line| line.split_once(':'))
        .find(|(_, url)| {
            let host = url
                .trim()
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("*.")
                .trim_end_matches('/')
                .to_ascii_lowercase();
            !host.is_empty() && (domain == host || domain.ends_with(&format!(".{host}")))
        })
        .map(|(name, _)| name.trim().to_string())
        .unwrap_or_default()
}

/// Replace `{field}` placeholders in a template from a fact bag. Unknown
/// fields render empty so a template never leaks raw braces to a phone.
pub fn fill(template: &str, facts: &serde_json::Value) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) => {
                let key = &after[..end];
                out.push_str(&fact_string(facts, key));
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                out.push_str(after);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Read a fact as a display string. Numbers are formatted for humans:
/// ratios named `*Rate`/`*Percent` render as percents, large byte counts stay
/// raw (the producer formats those), everything else uses plain display.
fn fact_string(facts: &serde_json::Value, key: &str) -> String {
    let Some(value) = facts.get(key) else {
        return String::new();
    };
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => {
            if let Some(float) = number.as_f64() {
                if key.ends_with("Rate") && (0.0..=1.0).contains(&float) {
                    return format!("{:.0}%", float * 100.0);
                }
                if key.ends_with("Percent") {
                    return format!("{float:.0}");
                }
                if float.fract() == 0.0 {
                    return format!("{}", float as i64);
                }
                return format!("{float:.2}");
            }
            number.to_string()
        }
        serde_json::Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for entry in KINDS {
            assert!(seen.insert(entry.kind), "duplicate kind {}", entry.kind);
        }
    }

    #[test]
    fn legacy_severity_maps_onto_levels() {
        assert_eq!(NoticeLevel::parse("critical"), NoticeLevel::Action);
        assert_eq!(NoticeLevel::parse("warning"), NoticeLevel::Attention);
        assert_eq!(NoticeLevel::parse("info"), NoticeLevel::Record);
        assert_eq!(NoticeLevel::parse("action"), NoticeLevel::Action);
        assert_eq!(NoticeLevel::parse("garbage"), NoticeLevel::Action);
    }

    #[test]
    fn level_orders_record_below_action() {
        assert!(NoticeLevel::Action.meets(NoticeLevel::Attention));
        assert!(!NoticeLevel::Record.meets(NoticeLevel::Attention));
    }

    #[test]
    fn every_metrics_and_clock_kind_is_registered() {
        for kind in [
            "fd_pressure",
            "host_cpu_pressure",
            "host_memory_pressure",
            "host_disk_pressure",
            "route_lifecycle",
            "db_error",
            "ssh_emfile",
            "ssh_bridge_write_stall",
            "ssh_channel_open_timeout",
            "ssh_channel_open_session_error",
            "ssh_forward_capacity",
            "share_request_watchdog_release",
            "proxy_downstream_stall",
            "llm_error_rate",
            "llm_rate_limit",
            "client_metrics_unavailable",
            "router_clock_skew",
            "router_clock_assurance_lost",
            "router_ntp_unsynchronized",
        ] {
            assert!(describe(kind).is_some(), "unregistered metrics kind {kind}");
        }
    }

    #[test]
    fn every_market_owner_signal_is_registered() {
        for kind in [
            "share_termination_finance_pending",
            "finance_critical_case",
            "finance_audit_mismatch",
            "payment_secret_unreadable",
            "payment_channel_auth_failed",
            "topup_reconcile_failing",
            "topup_review_stale",
            "gateway_identity_conflict",
            "gateway_orphan_paid_order",
            "gateway_external_refund",
            "duplicate_external_reference",
            "channel_promoted_live",
            "refund_requested",
            "dispute_opened",
            "c2c_dispute",
            "client_compensation_dispute",
            "client_compensation_dispute_overdue",
            "client_cleanup_finalization_failed",
            "earning_hold_legacy",
            "earning_unlock_failed",
            "service_notice_circuit",
        ] {
            assert!(
                describe(&format!("market_{kind}")).is_some(),
                "unregistered owner signal {kind}"
            );
        }
    }

    #[test]
    fn fill_formats_rates_and_percents() {
        let facts = serde_json::json!({"errorRate": 0.32, "rpm": 18.0, "summary": "三笔订单"});
        assert_eq!(
            fill("rate {errorRate} at {rpm}", &facts),
            "rate 32% at 18"
        );
        assert_eq!(fill("{summary}", &facts), "三笔订单");
        assert_eq!(fill("[{missing}]", &facts), "[]");
    }

    #[test]
    fn fill_leaves_unbalanced_braces_literal() {
        let facts = serde_json::json!({});
        assert_eq!(fill("a { b", &facts), "a { b");
    }
}
