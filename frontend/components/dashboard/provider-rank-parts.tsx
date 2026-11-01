"use client";

/* Hallmark · component: Provider rank parts · genre: modern-minimal · theme: existing Router system
 * states: ranked · leader · collecting · exploration · paused · rising · falling
 * The pieces a Provider's rank is told with, shared by the public ranking
 * and the Provider's own standing so both say it the same way.
 */

import * as React from "react";
import { ArrowDown, ArrowUp, BadgeCheck, Check, PauseCircle, Sparkles } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { isFormallyRanked, rankProgress } from "@/components/dashboard/providers-page-utils";
import type { MarketProvider, MarketProviderRankGeneration } from "@/lib/types";
import { cn } from "@/lib/utils";

/** The official operator's name is the Router's to say, in the reader's language. */
export function useProviderName() {
  const { t } = useLocaleText();
  return React.useCallback(
    (provider: Pick<MarketProvider, "official" | "displayName">) =>
      provider.official ? t("providers.officialName") : provider.displayName,
    [t],
  );
}

const PILL = "inline-flex h-6 items-center gap-1 whitespace-nowrap rounded-full px-2 text-[11px] font-semibold";

export function ProviderBadges({ provider }: { provider: Pick<MarketProvider, "official" | "exploration" | "status"> }) {
  const { t } = useLocaleText();
  const paused = provider.status === "paused";
  if (!provider.official && !provider.exploration && !paused) return null;
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1.5">
      {provider.official ? (
        <span className={cn(PILL, "bg-primary/10 text-primary")}>
          <BadgeCheck className="h-3.5 w-3.5" aria-hidden="true" />
          {t("providers.official")}
        </span>
      ) : null}
      {provider.exploration && !paused ? (
        <span className={cn(PILL, "bg-amber-50 font-medium text-amber-800")} title={t("providers.exploration.hint")}>
          <Sparkles className="h-3 w-3" aria-hidden="true" />
          {t("providers.exploration")}
          <span className="sr-only">. {t("providers.exploration.hint")}</span>
        </span>
      ) : null}
      {paused ? (
        <span className={cn(PILL, "bg-muted font-medium text-muted-foreground")} title={t("providers.paused.hint")}>
          <PauseCircle className="h-3 w-3" aria-hidden="true" />
          {t("providers.paused")}
          <span className="sr-only">. {t("providers.paused.hint")}</span>
        </span>
      ) : null}
    </span>
  );
}

export function RankChange({ change }: { change?: number }) {
  const { t } = useLocaleText();
  if (!change) return null;
  const up = change > 0;
  const label = t(up ? "providers.rankChange.up" : "providers.rankChange.down", { count: Math.abs(change) });
  const Icon = up ? ArrowUp : ArrowDown;
  return (
    <span
      className={cn("inline-flex items-center gap-0.5 font-mono text-[11px] font-semibold tabular-nums", up ? "text-emerald-700" : "text-rose-700")}
      title={label}
    >
      <Icon className="h-3 w-3" aria-hidden="true" />
      {Math.abs(change)}
      <span className="sr-only">{label}</span>
    </span>
  );
}

export function RankMark({ provider }: { provider: MarketProvider }) {
  const { t } = useLocaleText();
  if (!isFormallyRanked(provider)) {
    return (
      <span className="inline-flex min-h-7 items-center whitespace-nowrap rounded-full bg-muted px-2.5 text-xs font-medium text-muted-foreground">
        {t("providers.collecting")}
      </span>
    );
  }
  const leader = provider.rankPosition === 1;
  return (
    <span className="inline-flex flex-col items-start gap-0.5">
      <span
        className={cn(
          "inline-flex min-w-11 items-baseline justify-center gap-0.5 rounded-lg px-2 py-0.5 font-mono text-lg font-semibold tabular-nums",
          leader
            ? "bg-gradient-to-br from-accent to-accent-secondary text-white shadow-[0_4px_14px_rgba(0,82,255,0.25)]"
            : "text-foreground",
        )}
      >
        <span className={cn("text-xs font-medium", leader ? "text-white/80" : "text-muted-foreground")}>#</span>
        {provider.rankPosition}
      </span>
      <RankChange change={provider.rankChange} />
    </span>
  );
}

/** Each threshold on its own, so a reader can see which one is still missing. */
export function RankProgressChips({
  provider,
  generation,
}: {
  provider: MarketProvider;
  generation: Pick<MarketProviderRankGeneration, "minimumIndependentBuyers" | "minimumObservationDays">;
}) {
  const { t } = useLocaleText();
  const progress = rankProgress(provider, generation);
  const items = [
    { key: "buyers", met: progress.buyers.met, label: t("providers.progress.buyers", { count: progress.buyers.count, min: progress.buyers.min }) },
    { key: "days", met: progress.days.met, label: t("providers.progress.days", { count: progress.days.count, min: progress.days.min }) },
  ];
  return (
    <span className="flex min-w-0 flex-wrap items-center gap-1.5" aria-label={t("providers.progress.label")}>
      {items.map((item) => (
        <span
          key={item.key}
          className={cn(
            "inline-flex h-6 items-center gap-1 whitespace-nowrap rounded-full px-2 font-mono text-[11px] tabular-nums",
            item.met ? "bg-emerald-50 text-emerald-800" : "bg-muted text-muted-foreground",
          )}
        >
          {item.met
            ? <Check className="h-3 w-3" aria-hidden="true" />
            : <span className="h-1.5 w-1.5 rounded-full bg-muted-foreground/50" aria-hidden="true" />}
          {item.label}
        </span>
      ))}
    </span>
  );
}
