"use client";

import * as React from "react";
import { Alert, Button, Card, Chip } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { CompactSelect } from "@/components/common/compact-select";
import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { messages, type MessageKey } from "@/lib/i18n";
import { deliveryLabels, deliveryState, providerFailureCount, type MarketNotificationDiagnostics } from "@/lib/market-notification-diagnostics";

const copy = {
  en: { title: "Marketplace email delivery", hint: "Gateway acceptance is not delivery. Historical accepted mail is never automatically resent.", accepted: "Gateway accepted", delivered: "Delivered", delayed: "Delayed", failed: "Failed / rejected", all: "All statuses", event: "Event / recipient", submission: "Submission", delivery: "Final delivery", detail: "Diagnostics", empty: "No matching channel tasks.", lastEvent: "Last pause event", lastAccepted: "Last pause submission", pending: "Pending pause notices", policy: "Pause cooldown {minutes} min · {count} per service / day · {budget} service emails / day", polling: "Delivery tracking", webhook: "Signed webhook configured", pollOnly: "Polling fallback; a read-enabled Resend key is required", attempts: "attempts", reference: "Reference" },
  "zh-CN": { title: "市场邮件投递", hint: "网关接收不等于送达；已接收的历史邮件不会自动重发。以下统计覆盖全部市场通知历史。", accepted: "网关已接收", delivered: "已投递", delayed: "投递延迟", failed: "失败 / 拒收", all: "全部状态", event: "事件 / 收件人", submission: "提交状态", delivery: "最终投递", detail: "诊断信息", empty: "没有匹配的渠道任务。", lastEvent: "最近暂停事件", lastAccepted: "最近暂停提交", pending: "暂停提醒待处理", policy: "暂停冷却 {minutes} 分钟 · 单服务 {count} 封/日 · 服务邮件总预算 {budget} 封/日", polling: "投递追踪", webhook: "已配置签名回调", pollOnly: "使用查询补偿，Resend API Key 需有邮件读取权限", attempts: "次尝试", reference: "关联编号" },
} as const;

export function MarketNotificationDiagnosticsPanel() {
  const { locale, t } = useLocaleText();
  const text = copy[locale];
  const [data, setData] = React.useState<MarketNotificationDiagnostics | null>(null);
  const [status, setStatus] = React.useState("all");
  const [cursors, setCursors] = React.useState<(string | null)[]>([null]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const version = React.useRef(0);
  const cursor = cursors[cursors.length - 1];
  const load = React.useCallback(async () => {
    const request = ++version.current;
    setLoading(true); setError("");
    const query = new URLSearchParams();
    if (cursor) query.set("cursor", cursor);
    if (status !== "all") query.set("status", status);
    try {
      const result = await parseJson<MarketNotificationDiagnostics>(await authFetch(`/v1/admin/market-notifications?${query}`, { cache: "no-store" }));
      if (version.current === request) setData(result);
    } catch (cause) {
      if (version.current === request) setError(cause instanceof Error ? cause.message : String(cause));
    } finally { if (version.current === request) setLoading(false); }
  }, [cursor, status]);
  React.useEffect(() => { void load(); return () => { version.current += 1; }; }, [load]);
  const date = (value?: string | null) => value && Number.isFinite(Date.parse(value)) ? new Date(value).toLocaleString(locale) : "—";
  const label = (value: string) => deliveryLabels[locale][value as keyof typeof deliveryLabels.en] || value;
  const kindLabel = (kind: string) => {
    const key = `marketNotifications.kind.${kind}`;
    return key in messages.en ? t(key as MessageKey) : kind;
  };
  return <Card className="rounded-lg">
    <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
      <div><Card.Title>{text.title}</Card.Title><Card.Description>{text.hint}</Card.Description></div>
      <Button variant="outline" onClick={() => void load()} isDisabled={loading} aria-label={t("common.reload")}>
        {loading ? <Loader2 className="size-4 animate-spin" /> : <RefreshCw className="size-4" />}{t("common.reload")}
      </Button>
    </Card.Header>
    <Card.Content className="grid gap-4">
      {error ? <Alert status="danger">{error}</Alert> : null}
      {data ? <>
        <dl className="grid grid-cols-2 gap-3 rounded-lg bg-muted/40 p-4 lg:grid-cols-4">
          {[[text.accepted, data.overview.submissions.sent || 0], [text.delivered, data.overview.provider.delivered || 0], [text.delayed, data.overview.provider.delayed || 0], [text.failed, providerFailureCount(data.overview.provider)]].map(([name, count]) =>
            <div key={name}><dt className="text-xs text-muted-foreground">{name}</dt><dd className="mt-1 text-xl font-semibold tabular-nums">{count}</dd></div>)}
        </dl>
        <div className="grid gap-1 text-xs text-muted-foreground">
          <p>{text.lastEvent}: {date(data.overview.pause.eventCreatedAt)} · {text.lastAccepted}: {date(data.overview.pause.acceptedAt)} · {text.pending}: {data.overview.pause.pending}</p>
          <p>{text.policy.replace("{minutes}", String(data.limits.pauseCooldownSecs / 60)).replace("{count}", String(data.limits.pauseDailyLimit)).replace("{budget}", String(data.limits.emailDailyLimit))}</p>
          <p>{text.polling}: {data.webhookConfigured ? text.webhook : text.pollOnly} · {date(data.overview.tracking.lastWebhookAt || data.overview.tracking.lastPollAt)}</p>
        </div>
        {data.overview.tracking.pollErrorCode ? <Alert status="warning">{data.overview.tracking.pollErrorCode} · {text.pollOnly}</Alert> : null}
      </> : null}
      <div className="flex items-center justify-between gap-3">
        <CompactSelect value={status} onChange={(next) => { setData(null); setStatus(next); setCursors([null]); }} ariaLabel={t("notifications.status")} triggerClassName="min-w-44 text-sm"
          options={[{ value: "all", label: text.all }, ...["accepted", "delayed", "delivered", "failed", "suppressed", "queued", "uncertain"].map((value) => ({ value, label: label(value) }))]} />
        <div className="flex gap-2">
          <Button size="sm" variant="outline" isDisabled={loading || cursors.length === 1} onClick={() => { setData(null); setCursors((current) => current.slice(0, -1)); }}>{t("common.previous")}</Button>
          <Button size="sm" variant="outline" isDisabled={loading || !data?.nextCursor} onClick={() => { if (data?.nextCursor) { setCursors((current) => [...current, data.nextCursor]); setData(null); } }}>{t("common.next")}</Button>
        </div>
      </div>
      <div className="max-h-[32rem] overflow-auto rounded-lg border" aria-busy={loading}>
        <table className="w-full min-w-[820px] text-left text-sm">
          <thead className="sticky top-0 bg-muted text-xs text-muted-foreground"><tr>{[text.event, text.submission, text.delivery, text.detail].map((title) => <th key={title} className="px-4 py-3 font-medium">{title}</th>)}</tr></thead>
          <tbody>{data?.items.map((item) => <tr key={item.id} className="align-top">
            <td className="px-4 py-3"><div>{kindLabel(item.eventKind)}</div><div className="mt-1 text-xs text-muted-foreground">{item.targetMasked} · {item.channel}</div><div className="mt-1 max-w-60 break-all font-mono text-[10px] text-muted-foreground">{item.objectId}</div></td>
            <td className="px-4 py-3"><Chip size="sm" variant="soft">{label(item.submissionStatus === "sent" ? "accepted" : item.submissionStatus)}</Chip><div className="mt-1 text-xs text-muted-foreground">{date(item.acceptedAt || item.firstAttemptAt || item.createdAt)}</div><div className="text-xs text-muted-foreground">{item.attempts} {text.attempts}</div></td>
            <td className="px-4 py-3"><span>{item.channel === "email" && item.submissionStatus === "sent" ? label(deliveryState(item)) : "—"}</span><div className="mt-1 text-xs text-muted-foreground">{date(item.providerStatusAt)}</div></td>
            <td className="max-w-72 px-4 py-3 text-xs"><div className="break-words text-muted-foreground">{item.suppressionReason || item.providerError || item.submissionError}</div><details className="mt-1 text-muted-foreground"><summary className="cursor-pointer">{text.reference}</summary><div className="mt-1 break-all font-mono">{item.eventId}<br />{item.providerMessageId}</div></details></td>
          </tr>)}</tbody>
        </table>
        {!loading && !data?.items.length ? <p className="p-8 text-center text-sm text-muted-foreground">{text.empty}</p> : null}
        {loading && !data ? <p className="p-8 text-center text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      </div>
    </Card.Content>
  </Card>;
}
