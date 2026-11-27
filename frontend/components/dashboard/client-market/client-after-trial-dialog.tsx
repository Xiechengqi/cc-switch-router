"use client";
/*
 * Switch a running Client trial between "trial only" (the rental ends when
 * the trial ends, nothing is charged) and keeping it afterwards (the first
 * month is charged when the trial ends). Keeping it needs the balance now; a
 * shortfall is topped up in place. Same contract endpoint as Share.
 */
import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Loader2, Wallet } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { InPlaceTopup } from "@/components/market/in-place-topup";
import { ApiError } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import { trialHours } from "@/lib/client-checkout";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { updateShareAfterTrial } from "@/lib/market-wallet-api";
import { formatUnitsCentsUp, suggestedTopupAmount } from "@/lib/market-wallet-money";
import { validWalletQuoteFunding } from "@/lib/market-wallet-quote";
import { onlineMarketError } from "@/lib/online-client-market";
import { sameLoginContext } from "@/lib/password-auth";
import type { MarketWalletQuoteFunding } from "@/lib/types";

export type ClientAfterTrialTarget = {
  contractId: string;
  name: string;
  to: "continue" | "end";
  /** The rental's frozen monthly price, formatted. */
  price: string;
  trialSecondsRemaining: number;
};

function fundingFrom(reason: unknown): MarketWalletQuoteFunding | null {
  if (!(reason instanceof ApiError) || reason.code !== "MARKET_WALLET_REQUIRED") return null;
  const funding = reason.details as MarketWalletQuoteFunding | undefined;
  return funding && validWalletQuoteFunding(funding) && BigInt(funding.shortfallUnits) > 0n ? funding : null;
}

export function ClientAfterTrialDialog({
  target,
  onClose,
  onChanged,
}: {
  target: ClientAfterTrialTarget | null;
  onClose: () => void;
  onChanged: () => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const [busy, setBusy] = React.useState(false);
  const [topupBusy, setTopupBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [funding, setFunding] = React.useState<MarketWalletQuoteFunding | null>(null);
  const key = target ? `${target.contractId}:${target.to}` : "";
  React.useEffect(() => {
    setBusy(false);
    setError("");
    setNotice("");
    setFunding(null);
    setTopupBusy(false);
  }, [key]);
  if (!target) return null;
  const { contractId, name, to, price } = target;
  const hours = trialHours(target.trialSecondsRemaining, locale);

  const submit = async () => {
    if (busy) return;
    const auth = readAuthState();
    const current = () => sameLoginContext(auth, readAuthState());
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await updateShareAfterTrial(contractId, to, to === "continue" ? "end" : "continue");
      if (!current()) return;
      if (session?.user?.id) notifyMarketBalanceChanged(session.user.id);
      onClose();
      onChanged();
    } catch (reason) {
      if (!current()) return;
      const shortfall = to === "continue" ? fundingFrom(reason) : null;
      if (shortfall) setFunding(shortfall);
      else setError(onlineMarketError(reason, t));
    } finally {
      if (current()) setBusy(false);
    }
  };
  const close = () => {
    if (!busy && !topupBusy) onClose();
  };
  return (
    <Modal.Backdrop isOpen isDismissable={!busy && !topupBusy} onOpenChange={(open) => !open && close()}>
      <Modal.Container placement="center" className="p-0 sm:p-4">
        <Modal.Dialog className="light flex h-dvh w-full max-w-none flex-col overflow-hidden rounded-none bg-card text-card-foreground sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-[30rem] sm:rounded-2xl">
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" isDisabled={busy || topupBusy} />
          <Modal.Header>
            <div className="min-w-0 pr-8">
              <Modal.Heading className="flex items-center gap-2">
                {funding ? <Wallet className="size-5 shrink-0 text-muted-foreground" aria-hidden /> : null}
                {t(funding ? "shareRent.topup.title" : to === "continue" ? "clientMarket.afterTrial.continueTitle" : "clientMarket.afterTrial.endTitle")}
              </Modal.Heading>
              <p className="mt-1 truncate text-xs text-muted-foreground" title={name}>{name}</p>
            </div>
          </Modal.Header>
          <Modal.Body className="grid min-h-0 min-w-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain">
            {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
            {funding ? (
              <InPlaceTopup
                active
                purpose={t("clientMarket.afterTrial.topupPurpose", { missing: formatUnitsCentsUp(funding.shortfallUnits, "$", locale) })}
                requiredUnits={(BigInt(funding.availableUnits) + BigInt(funding.shortfallUnits)).toString()}
                suggestedAmount={suggestedTopupAmount(funding.shortfallUnits)}
                idPrefix="client-after-trial-topup"
                onBusyChange={setTopupBusy}
                onFunded={() => {
                  setFunding(null);
                  setNotice(t("clientMarket.afterTrial.funded"));
                }}
              />
            ) : (
              <>
                <p className="text-sm leading-6">
                  {to === "continue"
                    ? t("clientMarket.afterTrial.continueBody", { hours, price })
                    : t("clientMarket.afterTrial.endBody", { hours })}
                </p>
                {notice ? <p role="status" className="rounded-lg bg-accent/5 px-3 py-2 text-sm text-accent">{notice}</p> : null}
              </>
            )}
          </Modal.Body>
          <Modal.Footer className="flex-wrap justify-between gap-2">
            {funding ? (
              <Button className="min-h-11" variant="ghost" isDisabled={topupBusy} onPress={() => setFunding(null)}>
                {t("shareRent.topup.back")}
              </Button>
            ) : (
              <>
                <Button className="min-h-11" variant="ghost" isDisabled={busy} onPress={close}>
                  {t("common.cancel")}
                </Button>
                <Button className="min-h-11" variant="primary" isDisabled={busy} onPress={() => void submit()}>
                  {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
                  {t(to === "continue" ? "clientMarket.afterTrial.continueAction" : "clientMarket.afterTrial.endAction")}
                </Button>
              </>
            )}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
