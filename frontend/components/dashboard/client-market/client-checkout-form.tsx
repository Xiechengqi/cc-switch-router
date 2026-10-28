"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Dices, Eye, EyeOff, Loader2 } from "lucide-react";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatHostOffer } from "./host-utils";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import {
  clientCheckoutState,
  trialDuration,
  type ClientDraft,
  type ClientSubdomainCheck,
} from "@/lib/client-checkout";
import type { ClientMarketAllocationQuote } from "@/lib/types";

// Hallmark · component scope · existing app tokens · critique: P4 H5 E4 S5 R5 V4
export function ClientCheckoutForm({
  quote,
  drafts,
  checks,
  seconds,
  busy,
  onDraft,
  onRandom,
}: {
  quote: ClientMarketAllocationQuote;
  drafts: Record<string, ClientDraft>;
  checks: Record<string, ClientSubdomainCheck>;
  seconds: number;
  busy: boolean;
  onDraft: (id: string, patch: Partial<ClientDraft>) => void;
  onRandom: (id: string) => void;
}) {
  const { locale, t } = useLocaleText();
  const [visible, setVisible] = React.useState<Record<string, boolean>>({});
  const state = clientCheckoutState(quote);
  return (
    <div className="grid min-w-0 gap-5">
      <p
        className={`text-xs tabular-nums ${seconds <= 30 ? "text-destructive" : "text-muted-foreground"}`}
      >
        {seconds > 0
          ? t("clientCheckout.reserved", { seconds })
          : t("clientCheckout.expired")}
      </p>
      {state.paid && quote.terminationPolicy ? <p className="rounded-lg bg-muted p-3 text-xs leading-5 text-muted-foreground">{t("clientTermination.purchaseTerms", { rate: String(quote.terminationPolicy.rateBps / 100) })}</p> : null}
      {quote.items.map((item, index) => {
        const draft = drafts[item.id] || {
          subdomain: "",
          password: "",
          autoRenew: false,
        };
        const check =
          checks[item.id]?.value === draft.subdomain.trim().toLowerCase()
            ? checks[item.id]
            : undefined;
        const price = formatHostOffer(
          item.dailyRateMinor,
          locale,
          item.freeDurationDays,
          item.cyclePriceMinor,
        );
        const addressId = `client-address-${item.id}`;
        const passwordId = `client-password-${item.id}`;
        return (
          <section
            key={item.id}
            className={`grid min-w-0 gap-4 ${index > 0 ? "border-t border-border pt-5" : ""}`}
          >
            <div className="flex min-w-0 flex-wrap items-start justify-between gap-2">
              <div className="min-w-0 flex-1">
                <div className="flex min-w-0 items-center gap-2">
                  <CountryFlag code={item.countryCode} />
                  <strong className="min-w-0 break-words text-sm">
                    {quote.items.length > 1 ? `${index + 1}. ` : ""}
                    {item.hostname || item.ip || t("createClient.title")}
                  </strong>
                </div>
                <details className="mt-1 text-xs text-muted-foreground">
                  <summary className="cursor-pointer rounded focus-visible:outline-2 focus-visible:outline-ring">
                    {t("clientCheckout.details")}
                  </summary>
                  <div className="mt-2 grid gap-1 break-all">
                    <span>{item.hostOwnerEmail}</span>
                    <span>{item.ip}</span>
                  </div>
                </details>
              </div>
              <strong className="shrink-0 text-sm tabular-nums">{price}</strong>
            </div>
            <div className="grid gap-1.5">
              <label htmlFor={addressId} className="text-sm font-medium">
                {t("clientCheckout.address")}
              </label>
              <div className="flex min-w-0 gap-2">
                <input
                  id={addressId}
                  value={draft.subdomain}
                  disabled={busy}
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  aria-invalid={check?.status === "unavailable"}
                  aria-describedby={`${addressId}-status`}
                  onChange={(event) =>
                    onDraft(item.id, { subdomain: event.target.value })
                  }
                  className="h-10 min-w-0 flex-1 rounded-md border border-border bg-background px-3 font-mono text-sm focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
                />
                <Button
                  isIconOnly
                  variant="ghost"
                  isDisabled={busy}
                  aria-label={t("createClient.randomSubdomain")}
                  onPress={() => onRandom(item.id)}
                >
                  <Dices className="h-4 w-4" />
                </Button>
              </div>
              <span
                id={`${addressId}-status`}
                className={`min-h-4 text-xs ${check?.status === "unavailable" ? "text-destructive" : "text-muted-foreground"}`}
                role="status"
              >
                {check?.status === "checking"
                  ? t("createClient.subdomainChecking")
                  : check?.status === "available"
                    ? t("createClient.subdomainAvailable")
                    : check?.message}
              </span>
            </div>
            <div className="grid gap-1.5">
              <label htmlFor={passwordId} className="text-sm font-medium">
                {t("createClient.password")}
              </label>
              <div className="flex min-w-0 gap-2">
                <input
                  id={passwordId}
                  type={visible[item.id] ? "text" : "password"}
                  value={draft.password}
                  disabled={busy}
                  autoComplete="new-password"
                  minLength={8}
                  aria-describedby={`${passwordId}-hint`}
                  onChange={(event) =>
                    onDraft(item.id, { password: event.target.value })
                  }
                  className="h-10 min-w-0 flex-1 rounded-md border border-border bg-background px-3 text-sm focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50"
                />
                <Button
                  isIconOnly
                  variant="ghost"
                  aria-label={t(
                    visible[item.id]
                      ? "clientCheckout.hidePassword"
                      : "clientCheckout.showPassword",
                  )}
                  onPress={() =>
                    setVisible((current) => ({
                      ...current,
                      [item.id]: !current[item.id],
                    }))
                  }
                >
                  {visible[item.id] ? (
                    <EyeOff className="h-4 w-4" />
                  ) : (
                    <Eye className="h-4 w-4" />
                  )}
                </Button>
              </div>
              <span
                id={`${passwordId}-hint`}
                className="text-xs text-muted-foreground"
              >
                {t("createClient.passwordHint")}
              </span>
            </div>
            {(item.cyclePriceMinor != null || item.dailyRateMinor != null) && !state.unsupported ? (
              <div className="grid gap-2 rounded-lg bg-muted p-3 text-sm" data-client-trial>
                <dl className="grid grid-cols-2 gap-x-4 gap-y-1 text-xs">
                  {([
                    ["clientTrial.allowance", item.trialAllowanceSeconds],
                    ["clientTrial.consumed", item.trialSecondsConsumed],
                    ["clientTrial.remaining", item.trialSecondsRemaining],
                  ] as const).map(([key, value]) => <React.Fragment key={key}>
                    <dt className="text-muted-foreground">{t(key)}</dt>
                    <dd className="text-right font-medium tabular-nums">{trialDuration(value, locale)}</dd>
                  </React.Fragment>)}
                </dl>
                <p className="text-xs leading-5">{item.trialAllowanceSeconds === 0
                  ? t("clientTrial.disabled") : item.trialSecondsRemaining > 0
                    ? t("clientCheckout.trialHint") : t("clientTrial.exhausted", { price })}</p>
                {item.trialSecondsRemaining > 0 ? <p className="text-xs leading-5 text-muted-foreground">{t("clientCheckout.trialCharge", { price })}</p> : null}
                <p className="text-xs leading-5 text-muted-foreground">{t("clientTrial.rules")}</p>
              </div>
            ) : null}
            {item.pricingModel === "prepaid_calendar_month" &&
            !state.unsupported ? (
              <label className="flex items-start gap-2 text-xs leading-5">
                <input
                  type="checkbox"
                  className="mt-1 shrink-0 accent-primary"
                  disabled={busy}
                  checked={draft.autoRenew}
                  onChange={(event) =>
                    onDraft(item.id, { autoRenew: event.target.checked })
                  }
                />
                <span>{t("clientCheckout.autoRenew")}</span>
              </label>
            ) : null}
          </section>
        );
      })}
      {state.unsupported ? (
        <p role="alert" className="text-sm text-destructive">
          {t("clientCheckout.unsupported")}
        </p>
      ) : state.paid && quote.walletFunding ? (
        <div className="grid gap-2 border-t border-border pt-4 text-sm">
          <div className="flex items-baseline justify-between gap-3">
            <span className="text-muted-foreground">
              {t("marketWallet.available")}
            </span>
            <strong className="break-all text-right tabular-nums">
              {formatWalletUnits(
                quote.walletFunding.availableUnits,
                "$",
                locale,
              )}
            </strong>
          </div>
          {state.trial ? (
            <div className="flex items-baseline justify-between gap-3">
              <span>{t("clientCheckout.requiredBalance")}</span>
              <strong className="break-all text-right tabular-nums">{formatWalletUnits(quote.walletFunding.requiredUnits, "$", locale)}</strong>
            </div>
          ) : (
            <>
              <div className="flex items-baseline justify-between gap-3">
                <span>{t("clientCheckout.reservedNow")}</span>
                <strong className="break-all text-right tabular-nums">
                  {formatWalletUnits(
                    quote.walletFunding.requiredUnits,
                    "$",
                    locale,
                  )}
                </strong>
              </div>
              <p className="text-xs text-muted-foreground">
                {t("clientCheckout.captureHint")}
              </p>
            </>
          )}
          {BigInt(quote.walletFunding.committedUnits || "0") > 0n ? (
            <details className="text-xs text-muted-foreground">
              <summary className="cursor-pointer">
                {t("marketWallet.existingCommitments")}
              </summary>
              <span className="tabular-nums">
                {formatWalletUnits(
                  quote.walletFunding.committedUnits,
                  "$",
                  locale,
                )}
              </span>
            </details>
          ) : null}
          {state.shortfall ? (
            <p role="alert" className="font-medium text-destructive">
              {t("marketWallet.shortfall", {
                amount: formatWalletUnits(
                  quote.walletFunding.shortfallUnits,
                  "$",
                  locale,
                ),
              })}
            </p>
          ) : null}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground">
          {t("clientCheckout.freeHint")}
        </p>
      )}
      {busy ? (
        <span
          role="status"
          className="inline-flex items-center gap-2 text-xs text-muted-foreground"
        >
          <Loader2 className="h-3 w-3 animate-spin" />
          {t("common.loading")}
        </span>
      ) : null}
    </div>
  );
}
