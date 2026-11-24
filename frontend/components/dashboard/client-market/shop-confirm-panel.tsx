"use client";

import { Button } from "@heroui/react";
import { LogIn, Server } from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { ClientMarketHost } from "@/lib/types";
import { HostHardwareSummary } from "./host-health";
import { formatHostOffer, hostDisplayLabel } from "./host-utils";

export function ShopConfirmPanel({
  host,
  authed,
  countryName,
  onContinue,
  onLogin,
  onRequestAccess,
}: {
  host: ClientMarketHost | null;
  authed: boolean;
  countryName: string;
  onContinue: (host: ClientMarketHost) => void;
  onLogin: () => void;
  onRequestAccess: (host: ClientMarketHost) => void;
}) {
  const { locale, t } = useLocaleText();
  if (!host) {
    return (
      <aside className="client-market-shop-confirm" aria-label={t("clientMarket.shop.stepConfirm")}>
        <div className="grid justify-items-center gap-3 px-6 py-10 text-center">
          <Server className="h-8 w-8 text-muted-foreground" aria-hidden />
          <p className="max-w-xs text-sm leading-6 text-muted-foreground">{t("clientMarket.shop.confirmEmpty")}</p>
        </div>
      </aside>
    );
  }
  const label = hostDisplayLabel(host);
  const idle = host.status === "idle";
  return (
    <aside className="client-market-shop-confirm" aria-label={t("clientMarket.shop.stepConfirm")}>
      <div className="grid gap-4">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <p className="text-xs text-muted-foreground">{t("clientMarket.shop.confirmTitle")}</p>
            <h2 className="mt-1 flex min-w-0 items-center gap-2 text-base font-semibold">
              <CountryFlag code={host.countryCode} />
              <span className="truncate">{label}</span>
            </h2>
            <p className="mt-1 truncate text-xs text-muted-foreground" title={host.hostOwnerEmail}>{countryName} · {host.hostOwnerEmail}</p>
          </div>
          <strong className="shrink-0 text-lg tabular-nums">{formatHostOffer(host.dailyRateMinor, locale, host.freeDurationDays, host.cyclePriceMinor)}</strong>
        </div>
        <div className="rounded-lg bg-muted px-3 py-2">
          <p className="mb-1 text-xs text-muted-foreground">{t("clientMarket.shop.summarySpecs")}</p>
          <HostHardwareSummary health={host.hostHealth} />
        </div>
        {idle ? <p className="text-xs leading-5 text-muted-foreground">{t("clientMarket.shop.purchaseTerms")}</p> : (
          <p role="status" className="text-sm text-amber-800">{t("clientMarket.shop.notIdle")}</p>
        )}
        {!authed ? (
          <Button variant="primary" onPress={onLogin}><LogIn className="h-4 w-4" />{t("createClient.login")}</Button>
        ) : !host.eligibility.allowed ? (
          <Button variant="primary" isDisabled={!idle} onPress={() => onRequestAccess(host)}>{t("clientMarket.shop.accessRequired")}</Button>
        ) : (
          <Button variant="primary" isDisabled={!idle} onPress={() => onContinue(host)}>{t("clientMarket.shop.continue")}</Button>
        )}
      </div>
    </aside>
  );
}
