"use client";
/*
 * Switch a running Share trial between "trial only" (the seat returns when
 * the trial ends, nothing is charged) and keeping it afterwards (the first
 * period is charged when the trial ends). Keeping it needs the balance now;
 * a shortfall is topped up right here instead of on another page.
 */
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Loader2, Wallet } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { InPlaceTopup } from "@/components/market/in-place-topup";
import { ApiError } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import { sameLoginContext } from "@/lib/password-auth";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { updateShareAfterTrial } from "@/lib/market-wallet-api";
import { validWalletQuoteFunding } from "@/lib/market-wallet-quote";
import { formatUnitsCentsUp, suggestedTopupAmount } from "@/lib/market-wallet-money";
import { trialHours } from "@/lib/client-checkout";
import type { MarketWalletQuoteFunding, ShareMarketSubscription } from "@/lib/types";
import { shareMarketMutationError } from "./market-utils";

/** `price` is the rental's frozen price, already formatted. */
export type ShareAfterTrialTarget = { subscription: ShareMarketSubscription; to: "continue" | "end"; price: string };

function fundingFrom(reason: unknown): MarketWalletQuoteFunding | null {
  if (!(reason instanceof ApiError) || reason.code !== "MARKET_WALLET_REQUIRED") return null;
  const funding = reason.details as MarketWalletQuoteFunding | undefined;
  return funding && validWalletQuoteFunding(funding) && BigInt(funding.shortfallUnits) > 0n ? funding : null;
}

export function ShareAfterTrialDialog({
  target,
  onClose,
  onChanged,
}: {
  target: ShareAfterTrialTarget | null;
  onClose: () => void;
  onChanged: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const [busy, setBusy] = React.useState(false);
  const [topupBusy, setTopupBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [funding, setFunding] = React.useState<MarketWalletQuoteFunding | null>(null);
  const key = target ? `${target.subscription.id}:${target.to}` : "";
  React.useEffect(() => { setBusy(false); setError(""); setNotice(""); setFunding(null); setTopupBusy(false); }, [key]);

  if (!target) return null;
  const { subscription, to, price } = target;
  const contract = subscription.walletContract;
  const hours = contract ? trialHours(contract.trialSecondsRemaining, locale) : "";
  const topup = !!funding;

  const submit = async () => {
    if (busy || !contract?.id) return;
    const auth = readAuthState();
    const current = () => sameLoginContext(auth, readAuthState());
    setBusy(true); setError(""); setNotice("");
    try {
      await updateShareAfterTrial(contract.id, to, to === "continue" ? "end" : "continue");
      if (!current()) return;
      if (session?.user?.id) notifyMarketBalanceChanged(session.user.id);
      onClose();
      void Promise.resolve().then(onChanged).catch(() => undefined);
    } catch (reason) {
      if (!current()) return;
      const shortfall = to === "continue" ? fundingFrom(reason) : null;
      if (shortfall) setFunding(shortfall);
      else setError(shareMarketMutationError(reason, t));
    } finally { if (current()) setBusy(false); }
  };
  const close = () => { if (!busy && !topupBusy) onClose(); };

  return (
    <Modal.Backdrop isOpen isDismissable={!busy && !topupBusy} onOpenChange={(open) => !open && close()}>
      <Modal.Container placement="center" className="p-0 sm:p-4">
        <Modal.Dialog className="light flex h-dvh w-full max-w-none flex-col overflow-hidden rounded-none bg-card text-card-foreground sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-[30rem] sm:rounded-2xl">
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" isDisabled={busy || topupBusy} />
          <Modal.Header>
            <div className="min-w-0 pr-8">
              <Modal.Heading className="flex items-center gap-2">
                {topup ? <Wallet className="size-5 shrink-0 text-muted-foreground" aria-hidden /> : null}
                {t(topup ? "shareRent.topup.title" : to === "continue" ? "shareRent.afterTrial.continueTitle" : "shareRent.afterTrial.endTitle")}
              </Modal.Heading>
              <p className="mt-1 truncate text-xs text-muted-foreground" title={subscription.shareName}>{subscription.shareName}</p>
            </div>
          </Modal.Header>
          <Modal.Body className="grid min-h-0 min-w-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain">
            {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
            {funding ? (
              <InPlaceTopup
                active
                purpose={t("shareRent.afterTrial.topupPurpose", { missing: formatUnitsCentsUp(funding.shortfallUnits, "$", locale) })}
                requiredUnits={(BigInt(funding.availableUnits) + BigInt(funding.shortfallUnits)).toString()}
                suggestedAmount={suggestedTopupAmount(funding.shortfallUnits)}
                idPrefix="share-after-trial-topup"
                onBusyChange={setTopupBusy}
                onFunded={() => { setFunding(null); setNotice(t("shareRent.afterTrial.funded")); }}
              />
            ) : (
              <>
                <p className="text-sm leading-6">
                  {to === "continue"
                    ? t("shareRent.afterTrial.continueBody", { hours, price })
                    : t("shareRent.afterTrial.endBody", { hours })}
                </p>
                {notice ? <p role="status" className="rounded-lg bg-accent/5 px-3 py-2 text-sm text-accent">{notice}</p> : null}
              </>
            )}
          </Modal.Body>
          <Modal.Footer className="flex-wrap justify-between gap-2">
            {topup ? (
              <Button className="min-h-11" variant="ghost" isDisabled={topupBusy} onPress={() => setFunding(null)}>{t("shareRent.topup.back")}</Button>
            ) : (
              <>
                <Button className="min-h-11" variant="ghost" isDisabled={busy} onPress={close}>{t("common.cancel")}</Button>
                <Button className="min-h-11" variant="primary" isDisabled={busy || !contract?.id} onPress={() => void submit()}>
                  {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
                  {t(to === "continue" ? "shareRent.afterTrial.continueAction" : "shareRent.afterTrial.endAction")}
                </Button>
              </>
            )}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
