"use client";

import * as React from "react";
import { Info } from "lucide-react";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, getShareListingPricing } from "@/lib/api";
import { formatPercent, formatUsdMicrosPerMillion } from "@/lib/usd-micros";
import { formatTokenMillions } from "@/lib/token-units";
import type { ShareListingPricingResponse, ShareListingUsageMix } from "@/lib/types";

const MIX_KEYS = ["input", "output", "cacheRead", "cacheWrite"] as const;
type MixKey = (typeof MIX_KEYS)[number];

function formatRate(value: string, locale: string) {
  return formatUsdMicrosPerMillion(value, locale) ?? "—";
}

function MixBar({
  mix,
  locale,
  t,
}: {
  mix: ShareListingUsageMix;
  locale: string;
  t: ReturnType<typeof useLocaleText>["t"];
}) {
  const labels: Record<MixKey, string> = {
    input: t("shareMarket.pricing.input"),
    output: t("shareMarket.pricing.output"),
    cacheRead: t("shareMarket.pricing.cacheRead"),
    cacheWrite: t("shareMarket.pricing.cacheWrite"),
  };
  return (
    <section className="grid gap-2 border-t border-slate-200 pt-3">
      <h4 className="text-xs font-semibold text-slate-700">
        {t("shareMarket.pricing.usageMix", { days: mix.windowDays })}
      </h4>
      <div className="grid gap-1.5">
        {MIX_KEYS.map((key) => {
          const share = mix.composition[key] || 0;
          const width = Math.max(0, Math.min(100, share * 100));
          return (
            <div key={key} className="grid grid-cols-[4.5rem_minmax(0,1fr)_2.75rem] items-center gap-2">
              <span className="truncate text-[11px] text-slate-500">{labels[key]}</span>
              <div className="h-1.5 overflow-hidden rounded-full bg-slate-100">
                <div
                  className="h-full rounded-full bg-slate-700"
                  style={{ width: `${width}%` }}
                />
              </div>
              <span className="text-right font-mono text-[11px] text-slate-600">
                {formatPercent(share * 100, locale)}%
              </span>
            </div>
          );
        })}
      </div>
      <p className="flex items-start gap-1.5 text-[11px] leading-4 text-slate-500">
        <Info className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden />
        <span>{t("shareMarket.pricing.mixDisclaimer")}</span>
      </p>
    </section>
  );
}

export function ListingPricingCard({ listingId }: { listingId: string }) {
  const { t, locale } = useLocaleText();
  const [pricing, setPricing] = React.useState<ShareListingPricingResponse | null>(
    null,
  );

  React.useEffect(() => {
    let cancelled = false;
    setPricing(null);
    void getShareListingPricing(listingId)
      .then((data) => {
        if (!cancelled) setPricing(data);
      })
      .catch((error) => {
        if (cancelled) return;
        // 404 is the non-public / missing listing case: omit the card rather
        // than advertising that a private listing exists.
        if (error instanceof ApiError && error.status === 404) {
          setPricing(null);
          return;
        }
        setPricing(null);
      });
    return () => {
      cancelled = true;
    };
  }, [listingId]);

  if (!pricing || !pricing.models.length) return null;

  return (
    <section className="grid gap-3 rounded-lg border border-slate-200 bg-white p-3">
      <div className="flex items-start justify-between gap-2">
        <h3 className="text-xs font-semibold uppercase text-slate-500">
          {t("shareMarket.pricing.title")}
        </h3>
        <span className="font-mono text-[10px] text-slate-400">
          {t("shareMarket.pricing.catalogRevision", {
            revision: pricing.catalogRevision.slice(0, 12),
          })}
        </span>
      </div>
      <ul className="grid gap-3">
        {pricing.models.map((model) => {
          const long =
            model.longContextThreshold != null
              ? t("shareMarket.pricing.longContext", {
                  threshold: formatTokenMillions(model.longContextThreshold, locale),
                })
              : null;
          return (
            <li key={model.modelKey} className="grid gap-1">
              <div className="text-sm font-medium text-slate-900">{model.displayName}</div>
              <div className="flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[11px] leading-4 text-slate-600">
                <span>
                  {t("shareMarket.pricing.input")} {formatRate(model.rates.input, locale)}
                </span>
                <span>
                  {t("shareMarket.pricing.output")} {formatRate(model.rates.output, locale)}
                </span>
                <span>
                  {t("shareMarket.pricing.cacheRead")} {formatRate(model.rates.cacheRead, locale)}
                </span>
                <span>
                  {t("shareMarket.pricing.cacheWrite")} {formatRate(model.rates.cacheWrite5m, locale)}
                </span>
              </div>
              <div className="text-[11px] text-slate-400">
                {t("shareMarket.pricing.perMillion")}
                {long ? ` · ${long}` : ""}
              </div>
            </li>
          );
        })}
      </ul>
      {pricing.usageMix ? (
        <MixBar mix={pricing.usageMix} locale={locale} t={t} />
      ) : null}
    </section>
  );
}
