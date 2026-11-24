"use client";

import { ChevronRight } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatHostHealthTime, hostHealthDisplayState, hostHealthRecord } from "@/lib/host-health-view";
import type { HostHealth } from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";

export function HostHardwareSummary({ health }: { health?: HostHealth | null }) {
  const { locale, t } = useLocaleText();
  const snapshot = health?.snapshot;
  if (!snapshot) return <span className="text-muted-foreground">{t("hostHealth.specsPending")}</span>;
  const number = (value: number, divisor = 1) => new Intl.NumberFormat(locale, { maximumFractionDigits: divisor === 1 ? 3 : 1 }).format(value / divisor);
  const cpu = snapshot.effectiveVcpu.value;
  const memory = snapshot.memoryTotalBytes.value;
  const platform = [snapshot.os, snapshot.arch].filter(Boolean).join("-");
  return <div className="host-hardware-summary tabular-nums" title={t("hostHealth.selfReported")}>
    <span>{platform} · {cpu != null ? t("hostHealth.cpu", { value: number(cpu) }) : t("hostHealth.cpuUnknown")}</span>
    <span className="text-muted-foreground">{memory != null ? t("hostHealth.memory", { value: number(memory, 2 ** 30) }) : t("hostHealth.memoryUnknown")}</span>
  </div>;
}

/** The list and quote share presentation; reads and management live in one drawer. */
export function HostHealthSummary({ health, hostLabel, variant = "checkout", onOpen, specs = true }: {
  health?: HostHealth | null;
  hostLabel?: string;
  variant?: "table" | "checkout";
  onOpen?: () => void;
  /** Shop confirmation already shows specifications in their own block. */
  specs?: boolean;
}) {
  const { locale, t } = useLocaleText();
  const state = hostHealthDisplayState(health);
  const record = hostHealthRecord(health);
  const label = t(("hostHealth.state." + state) as MessageKey);
  const tone = state === "healthy" ? "normal" : ["missed", "first_overdue", "deployment_failed", "revoked"].includes(state) ? "warning" : "neutral";
  const limitations: { full: MessageKey; short: MessageKey }[] = [];
  if (health?.supervision === "nohup") limitations.push({ full: "hostHealth.noAutostart", short: "hostHealth.noAutostartShort" });
  if (health?.clockSynchronized === false) limitations.push({ full: "hostHealth.clockUnsynced", short: "hostHealth.clockUnsyncedShort" });
  const content = <>
    <span className="host-health-status" data-tone={tone}><span className="host-health-dot" aria-hidden="true" />{label}</span>
    {health?.lastReceivedAt || record ? <span className="host-health-meta tabular-nums">
      {health?.lastReceivedAt ? <time dateTime={health.lastReceivedAt} title={formatHostHealthTime(health.lastReceivedAt, locale)}>{formatHostHealthTime(health.lastReceivedAt, locale, variant === "table")}</time> : null}
      {record ? <span title={t("hostHealth.record", { received: health!.received24h, observed: health!.observable24h })}>{record}</span> : null}
    </span> : null}
    {variant === "checkout" ? <>
      {specs ? <HostHardwareSummary health={health} /> : null}
      {health?.nextDueAt && ["awaiting_first", "first_overdue"].includes(state) ? <time className="text-muted-foreground" dateTime={health.nextDueAt}>{t("hostHealth.next", { time: formatHostHealthTime(health.nextDueAt, locale) })}</time> : null}
    </> : null}
    {limitations.length ? <span className="host-health-limitation" title={limitations.map(item => t(item.full)).join(" · ")}>{limitations.map((item, index) => <span key={item.full}>{index ? " · " : ""}{t(variant === "table" ? item.short : item.full)}</span>)}</span> : null}
  </>;
  if (onOpen) return <button type="button" className={`host-health-summary host-health-summary--${variant}`} aria-label={t("hostHealth.openFor", { host: hostLabel || "Host" })} onClick={event => { event.stopPropagation(); onOpen(); }}>
    <span className="host-health-lines">{content}</span><ChevronRight className="host-health-chevron" aria-hidden="true" />
  </button>;
  return <section aria-label={[t("hostHealth.title"), hostLabel].filter(Boolean).join(" · ")} className="host-health-summary host-health-summary--checkout">
    {variant === "checkout" ? <span className="host-health-caption">{t("hostHealth.title")}</span> : null}{content}
    {record ? <span className="sr-only">{t("hostHealth.record", { received: health!.received24h, observed: health!.observable24h })}</span> : null}
  </section>;
}
