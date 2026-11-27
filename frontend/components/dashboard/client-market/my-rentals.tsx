"use client";
/*
 * "My Clients": the buyer's running rentals, on top of the market page.
 * Each card leads with the big console button and the full egress IP (the
 * tenant judges its quality), then the setup checklist, any reclaim notice,
 * the after-trial choice and the secondary actions.
 */
import * as React from "react";
import Link from "next/link";
import { Check, Copy, ExternalLink, Loader2, TriangleAlert } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { trialDuration } from "@/lib/client-checkout";
import { clientMarketTenantHref } from "@/lib/dashboard-nav";
import { formatUsdMoney } from "@/lib/market-money";
import { clientDisplayName, getCurrentOnlineLeases, type OnlineLease } from "@/lib/online-client-market";
import { usePersistentState } from "@/lib/use-persistent-state";
import { cn } from "@/lib/utils";
import { ClientAfterTrialDialog, type ClientAfterTrialTarget } from "./client-after-trial-dialog";
import { CLIENT_MARKET_POLL_MS } from "./host-utils";
import { LeaseActions } from "./lease-actions";
import { ClientEgressLine } from "./online-shop";

const ONBOARDING_KEY = "cc_switch_router_client_onboarding_v1";
const focusRing = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";

export function consoleHref(leaseId: string) {
  return `/client-market/leases/${encodeURIComponent(leaseId)}/open`;
}

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
  // One owner for the checklist storage, so cards never overwrite each other.
  const [onboarding, setOnboarding, hydrated] = usePersistentState<OnboardingState>(ONBOARDING_KEY, {});
  const setStep = React.useCallback(
    (leaseId: string, step: StepId, value: boolean) =>
      setOnboarding((current) => {
        const steps = { ...current[leaseId] };
        if (value) steps[step] = true;
        else delete steps[step];
        return { ...current, [leaseId]: steps };
      }),
    [setOnboarding],
  );
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
    <section className="grid min-w-0 gap-3" aria-labelledby="my-clients-title">
      <div className="flex min-w-0 items-center gap-3">
        <h2 id="my-clients-title" className="shrink-0 whitespace-nowrap text-xs font-semibold uppercase tracking-wide text-slate-500">
          {t("clientMarket.mine.title")}
          <span className="ml-1.5 tabular-nums text-slate-400">{leases.length}</span>
        </h2>
        <Link href={clientMarketTenantHref()} className={cn("ml-auto inline-flex h-9 items-center rounded px-1 text-xs font-medium text-accent hover:underline", focusRing)}>
          {t("clientMarket.mine.records")}
        </Link>
      </div>
      <div className="grid min-w-0 grid-cols-1 gap-3 lg:grid-cols-2">
        {leases.map((lease) => (
          <RentalCard
            key={lease.leaseId}
            lease={lease}
            highlighted={lease.leaseId === highlightId}
            steps={hydrated ? onboarding[lease.leaseId] || {} : null}
            onStep={(step, value) => setStep(lease.leaseId, step, value)}
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
  steps,
  onStep,
  onChanged,
  onAfterTrial,
  onTerminationOpen,
}: {
  lease: OnlineLease;
  highlighted: boolean;
  /** Ticked checklist steps; null until browser storage is read. */
  steps: Partial<Record<StepId, true>> | null;
  onStep: (step: StepId, value: boolean) => void;
  onChanged: () => void;
  onAfterTrial: (target: ClientAfterTrialTarget) => void;
  onTerminationOpen: (id: string) => void;
}) {
  const { locale, t } = useLocaleText();
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
  return (
    <article
      id={`my-client-${lease.leaseId}`}
      tabIndex={-1}
      aria-label={name}
      className={cn(
        "grid min-w-0 scroll-mt-20 content-start gap-4 rounded-2xl border bg-card p-4 shadow-sm outline-none transition-shadow duration-300 sm:p-5",
        highlighted ? "border-accent/50 shadow-[0_8px_24px_rgba(0,82,255,0.18)] ring-2 ring-accent/40" : "border-border",
      )}
    >
      <header className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="grid min-w-0 gap-1">
          <h3 className="truncate text-base font-semibold tracking-tight" title={name}>{name}</h3>
          <EgressIp lease={lease} />
        </div>
        <span
          className={cn(
            "inline-flex min-h-7 shrink-0 items-center gap-1.5 rounded-full px-2.5 text-xs font-medium",
            tone === "attention" ? "bg-amber-50 text-amber-900" : tone === "pending" ? "bg-slate-100 text-slate-700" : "bg-emerald-50 text-emerald-800",
          )}
        >
          {tone === "pending" ? (
            <Loader2 className="size-3 animate-spin motion-reduce:animate-none" aria-hidden />
          ) : (
            <span className={cn("size-1.5 rounded-full", tone === "attention" ? "bg-amber-500" : "bg-emerald-500")} aria-hidden />
          )}
          {status}
        </span>
      </header>

      {reclaimAt || lease.reclaimed ? (
        <div role="status" className="flex gap-2.5 rounded-xl border border-amber-200 bg-amber-50 px-3 py-2.5 text-sm leading-6 text-amber-950">
          <TriangleAlert className="mt-1 size-4 shrink-0 text-amber-600" aria-hidden />
          <p className="min-w-0">
            {reclaimAt ? t("clientMarket.mine.reclaimBanner", { date: date(reclaimAt) }) : t("clientMarket.mine.reclaimedBanner")}
            {lease.reclaimReason ? ` ${t("clientMarket.mine.reclaimReason", { reason: t(`clientMarket.reclaim.reason.${lease.reclaimReason}`) })}` : ""}
            {lease.reclaimNote ? (
              <span className="mt-1 block whitespace-pre-wrap break-words text-amber-900">{t("clientMarket.mine.reclaimNote", { note: lease.reclaimNote })}</span>
            ) : null}
          </p>
        </div>
      ) : null}

      <div className="flex min-w-0 flex-wrap items-center gap-x-6 gap-y-2 text-xs">
        <p className="grid gap-0.5">
          <span className="text-muted-foreground">{t("rentalAudit.price")}</span>
          <strong className="text-sm tabular-nums">
            {price}
            {lease.cyclePriceMinor > 0 ? <span className="text-xs font-normal text-muted-foreground">{t("online.perMonth")}</span> : null}
          </strong>
        </p>
        {until ? (
          <p className="grid gap-0.5">
            <span className="text-muted-foreground">{t(untilLabel)}</span>
            <strong className="text-sm font-medium tabular-nums">{date(until)}</strong>
          </p>
        ) : null}
        {trial && !reclaimAt && lease.contractId ? (
          <p className="grid gap-0.5">
            <span className="text-muted-foreground">{t("clientMarket.mine.afterTrial")}</span>
            <button
              type="button"
              className={cn("min-h-9 w-fit rounded text-left text-sm font-medium text-accent hover:underline", focusRing)}
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
              <span className="ml-1 text-xs font-normal text-muted-foreground">· {t("clientMarket.mine.change")}</span>
            </button>
          </p>
        ) : null}
      </div>

      {active && steps ? <Onboarding lease={lease} done={steps} onStep={onStep} /> : starting ? <p className="text-sm text-muted-foreground">{t("clientMarket.mine.provisioning")}</p> : null}

      <footer className="flex min-w-0 flex-wrap items-center gap-2 border-t border-border pt-3">
        {active ? (
          <a
            href={consoleHref(lease.leaseId)}
            target="_blank"
            rel="noopener noreferrer"
            className={cn(
              "group inline-flex h-9 items-center gap-1.5 rounded-lg border border-slate-300 bg-white text-slate-900 shadow-sm hover:border-slate-400 hover:bg-slate-50 active:bg-slate-100 px-3 text-xs font-semibold transition-colors",
              focusRing,
            )}
            onClick={() => onStep("console", true)}
          >
            {t("online.console")}
            <ExternalLink className="size-4 transition-transform duration-200 group-hover:translate-x-0.5" aria-hidden />
          </a>
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
        <Link href={clientMarketTenantHref(lease.leaseId)} className={cn("ml-auto inline-flex h-9 items-center rounded px-1 text-xs text-muted-foreground hover:text-foreground", focusRing)}>
          {t("clientMarket.mine.details")}
        </Link>
      </footer>
    </article>
  );
}

function EgressIp({ lease }: { lease: OnlineLease }) {
  const { t } = useLocaleText();
  const [copied, setCopied] = React.useState(false);
  return (
    <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1">
      <ClientEgressLine listing={{ country: lease.country, egressIp: lease.egressIp ?? null }} emphasize />
      {lease.egressIp ? (
        <button
          type="button"
          aria-label={t("clientMarket.mine.copyIp")}
          title={t("clientMarket.mine.copyIp")}
          className={cn("inline-flex size-11 items-center justify-center rounded-lg text-muted-foreground hover:bg-muted hover:text-foreground", focusRing)}
          onClick={() =>
            void navigator.clipboard.writeText(lease.egressIp!).then(() => {
              setCopied(true);
              window.setTimeout(() => setCopied(false), 1500);
            })
          }
        >
          {copied ? <Check className="size-3.5 text-emerald-600" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
        </button>
      ) : null}
      {lease.egressIp ? <span className="w-full text-[11px] text-muted-foreground">{t("clientMarket.mine.ipHint")}</span> : null}
      {lease.egressIp && lease.egressDiffers ? <span className="w-full text-[11px] text-amber-800">{t("clientRentals.egressDiffers")}</span> : null}
    </div>
  );
}

type StepId = "console" | "account";
type OnboardingState = Record<string, Partial<Record<StepId, true>>>;

/**
 * Three steps after the rental starts. The Router knows when a Share exists
 * (step 3); the first two are ticked by the tenant and kept in this browser.
 */
function Onboarding({
  lease,
  done,
  onStep,
}: {
  lease: OnlineLease;
  done: Partial<Record<StepId, true>>;
  onStep: (step: StepId, value: boolean) => void;
}) {
  const { t } = useLocaleText();
  const [copied, setCopied] = React.useState(false);
  const shareDone = !!lease.shareUrl;
  const toggle = (step: StepId) => onStep(step, !done[step]);
  if (done.console && done.account && shareDone) {
    return lease.shareUrl ? <ShareLine url={lease.shareUrl} copied={copied} onCopied={setCopied} /> : null;
  }
  const steps: { id: StepId | "share"; label: string; hint?: React.ReactNode; checked: boolean }[] = [
    { id: "console", label: t("clientMarket.onboarding.console"), checked: !!done.console },
    {
      id: "account",
      label: t("clientMarket.onboarding.account"),
      checked: !!done.account,
    },
    {
      id: "share",
      label: t("clientMarket.onboarding.share"),
      hint: lease.shareUrl ? <ShareLine url={lease.shareUrl} copied={copied} onCopied={setCopied} /> : t("clientMarket.onboarding.shareHint"),
      checked: shareDone,
    },
  ];
  return (
    <ol className="grid gap-2 rounded-xl bg-muted/50 p-3" aria-label={t("clientMarket.onboarding.title")}>
      {steps.map((step, index) => (
        <li key={step.id} className="flex min-w-0 items-start gap-3">
          {step.id === "share" ? (
            <span
              className={cn(
                "mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-full text-xs font-semibold",
                step.checked ? "bg-slate-900 text-white" : "border border-border bg-card text-muted-foreground",
              )}
              aria-hidden
            >
              {step.checked ? <Check className="size-3.5" /> : index + 1}
            </span>
          ) : (
            <button
              type="button"
              role="checkbox"
              aria-checked={step.checked}
              aria-label={step.label}
              className={cn("group/step -m-2.5 flex size-11 shrink-0 items-center justify-center rounded-full", focusRing)}
              onClick={() => toggle(step.id as StepId)}
            >
              <span
                className={cn(
                  "flex size-6 items-center justify-center rounded-full text-xs font-semibold transition-colors",
                  step.checked ? "bg-slate-900 text-white" : "border border-border bg-card text-muted-foreground group-hover/step:border-accent/40",
                )}
              >
                {step.checked ? <Check className="size-3.5" aria-hidden /> : index + 1}
              </span>
            </button>
          )}
          <div className="grid min-w-0 gap-0.5 pt-0.5">
            <span className={cn("text-sm", step.checked ? "text-muted-foreground line-through decoration-muted-foreground/40" : "font-medium")}>{step.label}</span>
            {step.hint && !(step.checked && step.id !== "share") ? <span className="text-xs leading-5 text-muted-foreground">{step.hint}</span> : null}
          </div>
        </li>
      ))}
    </ol>
  );
}

function ShareLine({ url, copied, onCopied }: { url: string; copied: boolean; onCopied: (value: boolean) => void }) {
  const { t } = useLocaleText();
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1.5 text-xs">
      <span className="text-muted-foreground">{t("clientMarket.onboarding.shareUrl")}</span>
      <span className="min-w-0 break-all font-mono text-foreground">{url}</span>
      <button
        type="button"
        aria-label={t("lease.copy")}
        className={cn("inline-flex size-11 items-center justify-center rounded-md text-muted-foreground hover:bg-muted hover:text-foreground", focusRing)}
        onClick={() =>
          void navigator.clipboard.writeText(url).then(() => {
            onCopied(true);
            window.setTimeout(() => onCopied(false), 1500);
          })
        }
      >
        {copied ? <Check className="size-3.5 text-emerald-600" aria-hidden /> : <Copy className="size-3.5" aria-hidden />}
      </button>
    </span>
  );
}
