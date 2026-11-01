"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { cn } from "@/lib/utils";

type TariffHealth = {publishedRevision: string | null; effectiveFromMs: number | null; acknowledgedInstallations: number};

const shortRevision = (revision: string) => revision.replace(/^sha256:/, "").slice(0, 8);

/**
 * USD-equivalent quotas charge exactly the prices on this page: the Router
 * republishes its tariff from them at startup and after every price change.
 * The status needs the Router owner; other admins see only the explanation.
 */
export function ValueTariffStatus({unpriced, onShowUnpriced}: {unpriced?: number; onShowUnpriced: () => void}) {
  const {locale, t} = useLocaleText();
  const [health, setHealth] = React.useState<TariffHealth>();
  React.useEffect(() => {
    const controller = new AbortController();
    void authFetch("/v1/admin/share-value-pricing/health", {signal: controller.signal, cache: "no-store"})
      .then(parseJson<TariffHealth>)
      .then(data => { if (!controller.signal.aborted) setHealth(data); })
      .catch(() => { if (!controller.signal.aborted) setHealth(undefined); });
    return () => controller.abort();
  }, []);
  const revision = health?.publishedRevision ? shortRevision(health.publishedRevision) : undefined;
  const pending = !!health?.effectiveFromMs && health.effectiveFromMs > Date.now();
  const state = !health ? undefined
    : !revision ? "missing"
    : pending ? "pendingEffective"
    : health.acknowledgedInstallations > 0 ? "live" : "pendingAck";
  const time = health?.effectiveFromMs ? new Date(health.effectiveFromMs).toLocaleString(locale === "zh-CN" ? "zh-CN" : "en-US") : "";
  return <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3 rounded-md border bg-background p-4">
    <div className="grid min-w-0 flex-1 basis-80 gap-1">
      <h3 className="text-sm font-semibold">{t("dashboard.userLimit.value.tariff.title")}</h3>
      <p className="text-sm text-muted-foreground">{t("dashboard.userLimit.value.tariff.body")}</p>
      {state ? <p role="status" className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <span aria-hidden="true" className={cn("h-1.5 w-1.5 shrink-0 rounded-full", state === "live" ? "bg-emerald-500" : "bg-warning")} />
        <span className="min-w-0">{t(`dashboard.userLimit.value.tariff.state.${state}`, {revision: revision ?? "", count: health?.acknowledgedInstallations ?? 0, time})}</span>
      </p> : null}
    </div>
    {unpriced ? <Button size="sm" variant="secondary" className="min-h-11 sm:min-h-9" onClick={onShowUnpriced}>
      {t("dashboard.userLimit.value.tariff.showUnpriced", {count: unpriced})}
    </Button> : null}
  </div>;
}
