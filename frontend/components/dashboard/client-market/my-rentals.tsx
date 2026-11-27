"use client";
/*
 * "My Clients": the buyer's running rentals, shown under the catalog's "Mine"
 * tab. Each card follows the Share market card: status dot, name and price on
 * the first row, then the egress IP, the state, two small facts and one row
 * of actions. "Console" opens the workspace in the dashboard's console window.
 */
import * as React from "react";
import { Button, toast } from "@heroui/react";
import { Copy, Loader2, TriangleAlert } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientConsole } from "@/components/dashboard/client-console";
import { ClientConsoleIcon } from "@/components/dashboard/client-ops-menu";
import { MarketShareCardMetric, MARKET_SHARE_CARD_GRID_CLASS } from "@/components/dashboard/share-market/market-share-card";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { trialDuration } from "@/lib/client-checkout";
import { formatUsdMoney } from "@/lib/market-money";
import { clientDisplayName, getCurrentOnlineLeases, type OnlineLease } from "@/lib/online-client-market";
import { cn } from "@/lib/utils";
import { ClientAfterTrialDialog, type ClientAfterTrialTarget } from "./client-after-trial-dialog";
import { CLIENT_MARKET_POLL_MS } from "./host-utils";
import { LeaseActions } from "./lease-actions";
import { ClientEgressLine } from "./online-shop";

const focusRing = "focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-primary";

function inTrial(lease: OnlineLease) {
  return (
    !!lease.contractId &&
    !lease.paidStarted &&
    (lease.trialSecondsRemaining ?? 0) > 0 &&
    lease.cyclePriceMinor > 0 &&
    ["pending_activation", "trialing"].includes(lease.contractStatus || "")
  );
}

export function MyClientRentals({
  revision,
  highlightId,
  onTerminationOpen,
}: {
  revision: number;
  /** A rental the buyer just ordered: highlighted and scrolled into view. */
  highlightId: string | null;
  onTerminationOpen: (id: string) => void;
}) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : undefined;
  return actor ? <Rentals key={actor} revision={revision} highlightId={highlightId} onTerminationOpen={onTerminationOpen} /> : null;
}

function Rentals({
  revision,
  highlightId,
  onTerminationOpen,
}: {
  revision: number;
  highlightId: string | null;
  onTerminationOpen: (id: string) => void;
}) {
  const { t } = useLocaleText();
  const [leases, setLeases] = React.useState<OnlineLease[] | null>(null);
  const [local, bump] = React.useReducer((n: number) => n + 1, 0);
  const [afterTrial, setAfterTrial] = React.useState<ClientAfterTrialTarget | null>(null);
  React.useEffect(() => {
    const control = new AbortController();
    const read = () =>
      void getCurrentOnlineLeases(control.signal)
        .then((value) => {
          if (!control.signal.aborted) setLeases(value.leases.filter((lease) => lease.releasedAt == null));
        })
        .catch(() => undefined);
    read();
    const poll = () => {
      if (document.visibilityState === "visible") read();
    };
    const timer = window.setInterval(poll, CLIENT_MARKET_POLL_MS);
    document.addEventListener("visibilitychange", poll);
    return () => {
      control.abort();
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", poll);
    };
  }, [revision, local]);
  const scrolled = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!highlightId || scrolled.current === highlightId) return;
    const card = document.getElementById(`my-client-${highlightId}`);
    if (!card) return;
    scrolled.current = highlightId;
    card.scrollIntoView({ block: "nearest", behavior: "smooth" });
    card.focus({ preventScroll: true });
  }, [highlightId, leases]);
  if (!leases?.length) return null;
  return (
    <section className="grid min-w-0 gap-2" aria-labelledby="my-clients-title">
      <h3 id="my-clients-title" className="shrink-0 whitespace-nowrap text-xs font-semibold uppercase tracking-wide text-slate-500">
        {t("clientMarket.mine.title")}
        <span className="ml-1.5 tabular-nums text-slate-400">{leases.length}</span>
      </h3>
      <div className={MARKET_SHARE_CARD_GRID_CLASS}>
        {leases.map((lease) => (
          <RentalCard
            key={lease.leaseId}
            lease={lease}
            highlighted={lease.leaseId === highlightId}
            onChanged={bump}
            onAfterTrial={setAfterTrial}
            onTerminationOpen={onTerminationOpen}
          />
        ))}
      </div>
      <ClientAfterTrialDialog target={afterTrial} onClose={() => setAfterTrial(null)} onChanged={bump} />
    </section>
  );
}

function RentalCard({
  lease,
  highlighted,
  onChanged,
  onAfterTrial,
  onTerminationOpen,
}: {
  lease: OnlineLease;
  highlighted: boolean;
  onChanged: () => void;
  onAfterTrial: (target: ClientAfterTrialTarget) => void;
  onTerminationOpen: (id: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const name = clientDisplayName({ title: lease.title, label: lease.alias });
  const active = lease.state === "active";
  const starting = lease.state === "starting";
  // Anything else (ending, cleaning) is on its way out.
  const leaving = !active && !starting;
  const reclaimAt = leaving ? null : lease.reclaimEffectiveAt ?? null;
  const trial = inTrial(lease);
  const trialOnly = trial && lease.afterTrial === "end";
  const date = (sec: number | null | undefined) =>
    sec ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(sec * 1000)) : "";
  const status = reclaimAt
    ? t("clientMarket.mine.status.reclaim", { date: date(reclaimAt) })
    : starting
      ? t("clientMarket.mine.status.starting")
      : leaving
        ? t("clientMarket.mine.status.ending")
        : trialOnly
          ? t("clientMarket.mine.status.trialOnly", { duration: trialDuration(lease.trialSecondsRemaining ?? 0, locale) })
          : trial
            ? t("clientMarket.mine.status.trial", { duration: trialDuration(lease.trialSecondsRemaining ?? 0, locale) })
            : t("clientMarket.mine.status.active");
  const tone = reclaimAt || leaving ? "attention" : starting ? "pending" : "good";
  const until = lease.periodEnd ?? lease.expiresAt;
  const untilLabel = trialOnly
    ? "clientMarket.mine.trialEnds"
    : lease.autoRenew && lease.cyclePriceMinor > 0
      ? "clientMarket.mine.nextCharge"
      : "clientMarket.mine.until";
  const price = lease.cyclePriceMinor > 0 ? formatUsdMoney(lease.cyclePriceMinor, locale) : t("online.free");
  const showAfterTrial = trial && !reclaimAt && !!lease.contractId;
  return (
    <article
      id={`my-client-${lease.leaseId}`}
      tabIndex={-1}
      aria-label={name}
      className={cn(
        "grid min-w-0 scroll-mt-20 content-start gap-2.5 rounded-xl border bg-white p-3 shadow-sm outline-none",
        highlighted
          ? "border-primary ring-2 ring-primary/20"
          : tone === "attention"
            ? "border-amber-300 ring-1 ring-amber-200"
            : "border-slate-200",
      )}
    >
      <header className="flex min-w-0 items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-1.5">
          {tone === "pending" ? (
            <Loader2 className="size-3 shrink-0 animate-spin text-slate-400 motion-reduce:animate-none" aria-hidden />
          ) : (
            <span className={cn("h-2 w-2 shrink-0 rounded-full", tone === "attention" ? "bg-amber-500" : "bg-emerald-500")} aria-hidden />
          )}
          <strong className="min-w-0 truncate text-xs font-semibold text-foreground" title={name}>{name}</strong>
        </div>
        <strong className="shrink-0 whitespace-nowrap text-[13px] font-semibold tabular-nums text-slate-900">
          {price}
          {lease.cyclePriceMinor > 0 ? <span className="text-[11px] font-normal text-muted-foreground">{t("online.perMonth")}</span> : null}
        </strong>
      </header>

      <EgressIp lease={lease} />

      <p className={cn(
        "truncate text-[11px] font-semibold",
        tone === "attention" ? "text-amber-700" : tone === "pending" ? "text-slate-500" : "text-emerald-700",
      )} title={status}>
        {status}
      </p>

      {reclaimAt || lease.reclaimed ? (
        <div role="status" className="flex gap-2 rounded-lg border border-amber-200 bg-amber-50 px-2.5 py-2 text-[11px] leading-5 text-amber-950">
          <TriangleAlert className="mt-0.5 size-3.5 shrink-0 text-amber-600" aria-hidden />
          <p className="min-w-0">
            {reclaimAt ? t("clientMarket.mine.reclaimBanner", { date: date(reclaimAt) }) : t("clientMarket.mine.reclaimedBanner")}
            {lease.reclaimReason ? ` ${t("clientMarket.mine.reclaimReason", { reason: t(`clientMarket.reclaim.reason.${lease.reclaimReason}`) })}` : ""}
            {lease.reclaimNote ? (
              <span className="mt-1 block whitespace-pre-wrap break-words text-amber-900">{t("clientMarket.mine.reclaimNote", { note: lease.reclaimNote })}</span>
            ) : null}
          </p>
        </div>
      ) : starting ? (
        <p className="text-[11px] leading-5 text-slate-500">{t("clientMarket.mine.provisioning")}</p>
      ) : null}

      {until || showAfterTrial ? (
        <dl className="grid grid-cols-2 gap-2">
          {until ? <MarketShareCardMetric label={t(untilLabel)} value={date(until)} /> : null}
          {showAfterTrial ? (
            <div className="min-w-0">
              <dt className="truncate text-[11px] text-muted-foreground">{t("clientMarket.mine.afterTrial")}</dt>
              <dd>
                <button
                  type="button"
                  className={cn("max-w-full truncate rounded text-left text-[11px] font-semibold text-accent hover:underline", focusRing)}
                  onClick={() =>
                    onAfterTrial({
                      contractId: lease.contractId!,
                      name,
                      to: trialOnly ? "continue" : "end",
                      price,
                      trialSecondsRemaining: lease.trialSecondsRemaining ?? 0,
                    })
                  }
                >
                  {t(trialOnly ? "clientMarket.mine.afterTrialEnd" : "clientMarket.mine.afterTrialContinue")}
                  <span className="ml-1 font-normal text-muted-foreground">· {t("clientMarket.mine.change")}</span>
                </button>
              </dd>
            </div>
          ) : null}
        </dl>
      ) : null}

      <footer className="flex min-w-0 flex-wrap items-center gap-2 border-t border-slate-100 pt-2">
        {active ? (
          <Button
            size="sm"
            variant="primary"
            className="h-9"
            onPress={() => openConsole({ clientId: lease.leaseId, kind: "console", url: lease.clientUrl, title: lease.alias || name })}
          >
            <ClientConsoleIcon className="h-3.5 w-3.5 shrink-0 text-sky-600" />
            {t("dashboard.clientConsole")}
          </Button>
        ) : null}
        <LeaseActions
          id={lease.leaseId}
          active={active}
          canEnd={active || starting}
          address={lease.clientUrl}
          showConsole={false}
          onChanged={onChanged}
        />
        {lease.terminationId ? (
          <button type="button" className={cn("h-9 rounded px-1 text-xs text-accent hover:underline", focusRing)} onClick={() => onTerminationOpen(lease.terminationId!)}>
            {t("online.exitRecord")}
          </button>
        ) : null}
      </footer>
    </article>
  );
}

function EgressIp({ lease }: { lease: OnlineLease }) {
  const { t } = useLocaleText();
  const ip = lease.egressIp;
  const copy = async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      toast.success(t("common.copySuccess"));
    } catch {
      toast.danger(t("common.copyFailed"));
    }
  };
  return (
    <div className="grid min-w-0 gap-0.5">
      <div className="flex min-w-0 items-center gap-1.5">
        <ClientEgressLine listing={{ country: lease.country, egressIp: ip ?? null }} />
        {ip ? (
          <button
            type="button"
            aria-label={t("clientMarket.mine.copyIp")}
            title={t("clientMarket.mine.copyIp")}
            className="inline-flex h-6 w-6 shrink-0 items-center justify-center rounded-md border border-slate-200 bg-white text-slate-700 transition-colors hover:border-slate-300 hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
            onClick={() => void copy(ip)}
          >
            <Copy className="h-3 w-3" aria-hidden />
          </button>
        ) : null}
      </div>
      {ip && lease.egressDiffers ? <p className="text-[11px] text-amber-800">{t("clientRentals.egressDiffers")}</p> : null}
    </div>
  );
}
