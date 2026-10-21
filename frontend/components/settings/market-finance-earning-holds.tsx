"use client";

/* Owner view of active Provider earning holds: release manual/legacy holds and
 * place a manual hold on a contract. */

import * as React from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getMarketFinanceEarningHolds,
  placeMarketFinanceEarningHold,
  releaseMarketFinanceEarningHold,
  validMarketReasonCode,
  type MarketEarningHold,
} from "@/lib/market-wallet-api";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { codeLabel, OWNER_DIALOG_CLASS, OwnerField, ownerDateText, ownerErrorText } from "@/components/settings/market-finance-shared";

const RELEASABLE = new Set(["manual", "legacy_unknown"]);

export function MarketFinanceEarningHolds({ refreshKey, onChanged }: { refreshKey: number; onChanged: () => Promise<unknown> }) {
  const { locale, t } = useLocaleText();
  const [holds, setHolds] = React.useState<MarketEarningHold[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [releasing, setReleasing] = React.useState<MarketEarningHold | null>(null);
  const [releaseReason, setReleaseReason] = React.useState("");
  const [dialogError, setDialogError] = React.useState("");
  const [contractId, setContractId] = React.useState("");
  const [placeReason, setPlaceReason] = React.useState("");
  const [busy, setBusy] = React.useState(false);

  const load = React.useCallback(async () => {
    try {
      setHolds(await getMarketFinanceEarningHolds());
      setError("");
    } catch (cause) {
      setError(ownerErrorText(t, cause));
    } finally {
      setLoading(false);
    }
  }, [t]);

  React.useEffect(() => { void load(); }, [load, refreshKey]);

  const release = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!releasing || busy || !validMarketReasonCode(releaseReason) || !RELEASABLE.has(releasing.reasonKind)) return;
    setBusy(true);
    setDialogError("");
    try {
      await releaseMarketFinanceEarningHold({
        reasonKind: releasing.reasonKind as "manual" | "legacy_unknown",
        reasonRef: releasing.reasonRef,
        reasonCode: releaseReason.trim(),
      });
      setReleasing(null);
      setNotice(t("settings.marketFinance.holds.released"));
      await Promise.all([load(), onChanged()]).catch(() => {});
    } catch (cause) {
      setDialogError(ownerErrorText(t, cause));
    } finally {
      setBusy(false);
    }
  };

  const placeValid = !!contractId.trim() && validMarketReasonCode(placeReason);
  const place = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (busy || !placeValid) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await placeMarketFinanceEarningHold(contractId.trim(), placeReason.trim());
      setContractId("");
      setPlaceReason("");
      setNotice(t("settings.marketFinance.holds.placed"));
      await Promise.all([load(), onChanged()]).catch(() => {});
    } catch (cause) {
      setError(ownerErrorText(t, cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="grid min-w-0 gap-3" aria-labelledby="market-finance-holds-title">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="grid gap-1">
          <h4 id="market-finance-holds-title" className="text-sm font-semibold">{t("settings.marketFinance.holds.title")} <span className="font-normal tabular-nums text-muted-foreground">({holds.length})</span></h4>
          <p className="text-xs text-muted-foreground">{t("settings.marketFinance.holds.note")}</p>
        </div>
        <Button size="sm" variant="ghost" className="min-h-11" isDisabled={busy} onPress={() => void load()}>
          <RefreshCw className="size-4" aria-hidden="true" />{t("common.reload")}
        </Button>
      </div>
      {loading ? <p className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" aria-hidden="true" />{t("common.loading")}</p> : null}
      {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
      <div aria-live="polite">{notice ? <Alert status="success">{notice}</Alert> : null}</div>
      {!loading && !holds.length ? <p className="text-sm text-muted-foreground">{t("settings.marketFinance.holds.empty")}</p> : null}
      {holds.length ? <ul className="grid gap-2">{holds.map((hold) => (
        <li key={`${hold.reasonKind}:${hold.reasonRef}:${hold.providerUserId}`} className="grid min-w-0 gap-2 rounded-xl border border-border bg-card p-3 text-sm shadow-sm sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
          <div className="grid min-w-0 gap-0.5">
            <p className="flex flex-wrap items-baseline gap-x-2">
              <span className="font-medium">{codeLabel(t, "settings.marketFinance.holds.reason.", hold.reasonKind)}</span>
              <span className="font-semibold tabular-nums">{formatWalletUnits(hold.heldUnits, "$", locale)}</span>
              <span className="text-xs text-muted-foreground">{t("settings.marketFinance.holds.lots", { count: hold.lots })} · {t("settings.marketFinance.holds.since", { time: ownerDateText(hold.createdAt, locale) })}</span>
            </p>
            <p className="break-all font-mono text-[11px] text-muted-foreground">{hold.reasonRef} · {hold.providerUserId}</p>
          </div>
          {RELEASABLE.has(hold.reasonKind) ? (
            <Button size="sm" variant="outline" className="min-h-11" isDisabled={busy} onPress={() => { setReleasing(hold); setReleaseReason(""); setDialogError(""); setNotice(""); }}>
              {t("settings.marketFinance.holds.release")}
            </Button>
          ) : null}
        </li>
      ))}</ul> : null}

      <form className="grid gap-2 rounded-xl border border-dashed border-border p-3" onSubmit={(event) => void place(event)}>
        <p className="text-xs font-semibold">{t("settings.marketFinance.holds.place")}</p>
        <p className="text-xs text-muted-foreground">{t("settings.marketFinance.holds.placeNote")}</p>
        <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] sm:items-end">
          <OwnerField label={t("settings.marketFinance.holds.contractId")} value={contractId} onChange={(event) => setContractId(event.target.value)} maxLength={128} inputClassName="font-mono" />
          <OwnerField label={t("settings.marketFinance.owner.reasonCode")} value={placeReason} onChange={(event) => setPlaceReason(event.target.value)} invalid={!!placeReason && !validMarketReasonCode(placeReason)} maxLength={64} placeholder="identity_review" inputClassName="font-mono" />
          <Button type="submit" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={busy || !placeValid}>{t("settings.marketFinance.holds.place")}</Button>
        </div>
        <p className="text-xs text-muted-foreground">{t("settings.marketFinance.owner.reasonCodeHelp")}</p>
      </form>

      <Modal.Backdrop isOpen={!!releasing} onOpenChange={(open) => { if (!open && !busy) setReleasing(null); }}>
        <Modal.Container placement="center">
          <Modal.Dialog className={OWNER_DIALOG_CLASS}>
            <Modal.CloseTrigger />
            <Modal.Header><Modal.Heading>{t("settings.marketFinance.holds.releaseTitle")}</Modal.Heading></Modal.Header>
            <form onSubmit={(event) => void release(event)}>
              <Modal.Body className="grid gap-3 text-sm">
                {releasing ? <p><span className="font-medium">{codeLabel(t, "settings.marketFinance.holds.reason.", releasing.reasonKind)}</span> · <span className="tabular-nums">{formatWalletUnits(releasing.heldUnits, "$", locale)}</span></p> : null}
                <p className="text-xs text-muted-foreground">{t("settings.marketFinance.holds.releaseNote")}</p>
                <OwnerField autoFocus label={t("settings.marketFinance.owner.reasonCode")} help={t("settings.marketFinance.owner.reasonCodeHelp")} value={releaseReason} onChange={(event) => setReleaseReason(event.target.value)} invalid={!!releaseReason && !validMarketReasonCode(releaseReason)} maxLength={64} inputClassName="font-mono" />
                {dialogError ? <Alert status="danger" role="alert">{dialogError}</Alert> : null}
              </Modal.Body>
              <Modal.Footer>
                <Button type="button" variant="outline" isDisabled={busy} onPress={() => setReleasing(null)}>{t("common.cancel")}</Button>
                <Button type="submit" isDisabled={busy || !validMarketReasonCode(releaseReason)}>
                  {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                  {t("settings.marketFinance.holds.release")}
                </Button>
              </Modal.Footer>
            </form>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
