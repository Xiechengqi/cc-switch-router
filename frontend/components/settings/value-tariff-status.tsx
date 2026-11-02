"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { cn } from "@/lib/utils";
import { readAuthState } from "@/lib/auth";
import { parseValueTariffHealth, valueTariffState, type ValueTariffHealth } from "@/lib/value-tariff-delivery";

const shortRevision = (revision: string) => revision.replace(/^sha256:/, "").slice(0, 8);
const currentActor = () => {const auth = readAuthState(); return `${auth.email ?? ""}:${auth.loginEpoch ?? ""}:${!!auth.accessToken}`;};
const pageVisible = () => document.visibilityState === "visible";

/**
 * USD-equivalent quotas charge exactly the prices on this page: the Router
 * republishes its tariff from them at startup and after every price change.
 * The status needs the Router owner; other admins see only the explanation.
 */
export function ValueTariffStatus({unpriced, onShowUnpriced, refreshRevision = 0}: {unpriced?: number; onShowUnpriced: () => void; refreshRevision?: number}) {
  const {locale, t} = useLocaleText();
  const [actor, setActor] = React.useState(currentActor);
  const [attempt, retry] = React.useReducer(n => n + 1, 0);
  const [snapshot, setSnapshot] = React.useState<{actor: string; revision: number; health?: ValueTariffHealth; failed?: boolean}>();
  React.useEffect(() => {
    const update = () => setActor(currentActor());
    window.addEventListener("router-auth-changed",update); window.addEventListener("storage",update);
    return () => {window.removeEventListener("router-auth-changed",update);window.removeEventListener("storage",update);};
  },[]);
  React.useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let expiry: ReturnType<typeof setTimeout> | undefined;
    let pending = false, polls = 0;
    const refresh = async () => {
      if (pending || controller.signal.aborted || !pageVisible() || actor !== currentActor() || !readAuthState().accessToken || polls >= 120) return;
      pending = true; polls++;
      const request = new AbortController();
      const abort = () => request.abort();
      controller.signal.addEventListener("abort", abort, {once:true});
      const deadline = setTimeout(abort, 10_000);
      const auth = readAuthState();
      let delay = 15_000;
      try {
        const health = parseValueTariffHealth(await authFetch("/v1/admin/share-value-pricing/health",{signal:request.signal,cache:"no-store"},auth).then(parseJson<unknown>));
        if (!controller.signal.aborted && pageVisible() && actor === currentActor()) {
          setSnapshot({actor,revision:refreshRevision,health});
          clearTimeout(expiry);
          // Expire proof even if a later auth refresh or fetch never completes.
          expiry = setTimeout(() => {
            if (!controller.signal.aborted && actor === currentActor()) setSnapshot({actor,revision:refreshRevision,failed:true});
          },180_000);
        }
        delay = ["live","noServers"].includes(valueTariffState(health)) ? 30_000 : 5_000;
      } catch {
        if (!controller.signal.aborted && pageVisible() && actor === currentActor()) setSnapshot({actor,revision:refreshRevision,failed:true});
      } finally {
        clearTimeout(deadline);
        controller.signal.removeEventListener("abort", abort);
        pending = false;
        if (!controller.signal.aborted && polls < 120) timer = setTimeout(() => void refresh(),delay);
      }
    };
    const visible = () => {
      if (document.visibilityState === "hidden") {
        // Do not leave a stale green confirmation visible on return.
        setSnapshot(undefined);
      } else if (polls < 120) {
        clearTimeout(timer); void refresh();
      } else if (actor === currentActor()) {
        setSnapshot({actor,revision:refreshRevision,failed:true});
      }
    };
    void refresh(); document.addEventListener("visibilitychange",visible);
    return () => {controller.abort();clearTimeout(timer);clearTimeout(expiry);document.removeEventListener("visibilitychange",visible);};
  }, [actor, refreshRevision, attempt]);
  const current = snapshot?.actor === actor && actor === currentActor() && snapshot.revision === refreshRevision ? snapshot : undefined;
  const health = current?.health;
  const revision = health?.publishedRevision ? shortRevision(health.publishedRevision) : undefined;
  const state = health ? valueTariffState(health) : current?.failed ? "unavailable" : undefined;
  const time = health?.effectiveFromMs ? new Date(health.effectiveFromMs).toLocaleString(locale === "zh-CN" ? "zh-CN" : "en-US") : "";
  return <div className="flex flex-wrap items-start justify-between gap-x-6 gap-y-3 rounded-md border bg-background p-4">
    <div className="grid min-w-0 flex-1 basis-80 gap-1">
      <h3 className="text-sm font-semibold">{t("dashboard.userLimit.value.tariff.title")}</h3>
      <p className="text-sm text-muted-foreground">{t("dashboard.userLimit.value.tariff.body")}</p>
      {state ? <p role="status" className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <span aria-hidden="true" className={cn("h-1.5 w-1.5 shrink-0 rounded-full", state === "live" ? "bg-emerald-500" : "bg-warning")} />
        <span className="min-w-0">{t(`dashboard.userLimit.value.tariff.state.${state}`, {revision: revision ?? "", count: health?.delivery?.acknowledgedInstallations ?? 0,
          active: health?.delivery?.activatedInstallations ?? 0,total:health?.delivery?.expectedInstallations ?? 0,time})}</span>
      </p> : null}
    </div>
    <div className="flex flex-wrap gap-2"><Button size="sm" variant="outline" onClick={retry}>{t("dashboard.userLimit.value.readiness.retry")}</Button>
    {unpriced ? <Button size="sm" variant="secondary" className="min-h-11 sm:min-h-9" onClick={onShowUnpriced}>
      {t("dashboard.userLimit.value.tariff.showUnpriced", {count: unpriced})}
    </Button> : null}</div>
  </div>;
}
