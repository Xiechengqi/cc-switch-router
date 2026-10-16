"use client";

/* Hallmark · component: service dispute resolution · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · pre-emit critique: P5 H5 E4 S5 R5 V4
 */

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAdminMarketServiceDisputes, resolveMarketServiceDispute, type MarketServiceDispute } from "@/lib/market-wallet-api";
import { formatWalletUnits, parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";

export function MarketFinanceDisputesPanel() {
  const { locale, t } = useLocaleText();
  const [disputes, setDisputes] = React.useState<MarketServiceDispute[]>([]);
  const [selected, setSelected] = React.useState<MarketServiceDispute | null>(null);
  const [amount, setAmount] = React.useState("0.00");
  const [reason, setReason] = React.useState("");
  const [confirmed, setConfirmed] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  const reload = React.useCallback(async () => setDisputes(await getAdminMarketServiceDisputes()), []);
  React.useEffect(() => {
    let active = true;
    getAdminMarketServiceDisputes()
      .then((items) => { if (active) setDisputes(items); })
      .catch((cause) => { if (active) setError(cause instanceof Error ? cause.message : String(cause)); });
    return () => { active = false; };
  }, []);

  const amountMinor = parseNonNegativeUsdMinor(amount);
  const validReason = /^[a-z0-9_]{1,80}$/.test(reason.trim());
  const withinRefundable = amountMinor != null
    && (!selected || BigInt(amountMinor) * 86_400n <= BigInt(selected.refundableUnits));
  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!selected || busy || amountMinor == null || !withinRefundable || !validReason || !confirmed) return;
    setBusy(true);
    setError("");
    try {
      await resolveMarketServiceDispute(selected.id, selected.version, reason.trim(), amountMinor);
      await reload();
      setSelected(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
      await reload().catch(() => {});
      setSelected(null);
    } finally {
      setBusy(false);
    }
  };

  return <section className="grid min-w-0 gap-3" aria-label={t("settings.marketFinance.serviceDisputes")}>
    <div className="grid gap-1">
      <h4 className="text-sm font-semibold">{t("settings.marketFinance.serviceDisputes", { count: disputes.length })}</h4>
      <p className="text-xs text-muted-foreground">{t("settings.marketFinance.serviceDisputesNote")}</p>
    </div>
    {error && !selected ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
    {disputes.length ? <ul className="grid gap-2">{disputes.map((item) => <li key={item.id} className="flex min-w-0 flex-wrap items-center justify-between gap-2 rounded-md border border-border px-3 py-2 text-sm">
      <div className="grid min-w-0 gap-1">
        <span className="break-all font-mono text-xs">{item.contractId}</span>
        <span className="break-all text-xs text-muted-foreground">{item.reasonCode} · {item.buyerUserId} → {item.providerUserId}</span>
      </div>
      <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={busy} onPress={() => {
        setSelected(item); setAmount("0.00"); setReason(""); setConfirmed(false); setError("");
      }}>{t("settings.marketFinance.resolveDispute")}</Button>
    </li>)}</ul> : <p className="text-sm text-muted-foreground">{t("settings.marketFinance.noServiceDisputes")}</p>}
    <Button size="sm" variant="ghost" className="min-h-11 justify-self-start whitespace-nowrap" isDisabled={busy} onPress={() => void reload().catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))}>{t("common.reload")}</Button>

    <Modal.Backdrop isOpen={!!selected} onOpenChange={(open) => { if (!open && !busy) setSelected(null); }}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(520px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.CloseTrigger />
          <Modal.Header><Modal.Heading>{t("settings.marketFinance.resolveDispute")}</Modal.Heading></Modal.Header>
          <form onSubmit={(event) => void submit(event)}>
            <Modal.Body className="grid max-h-[65vh] min-w-0 gap-3 overflow-y-auto text-sm">
              {selected ? <p className="break-all font-mono text-xs">{selected.contractId}</p> : null}
              <p className="text-xs text-muted-foreground">{t("settings.marketFinance.disputeRefundLimit")}</p>
              {selected ? <p className="text-sm font-medium tabular-nums">{t("settings.marketFinance.disputeRefundable", { amount: formatWalletUnits(selected.refundableUnits, "$", locale) })}</p> : null}
              <label className="grid gap-1">
                <span>{t("settings.marketFinance.disputeRefundAmount")}</span>
                <input inputMode="decimal" value={amount} onChange={(event) => setAmount(event.target.value)}
                  aria-invalid={!!amount && (amountMinor == null || !withinRefundable)} placeholder="0.00"
                  className="h-11 min-w-0 rounded-md border border-input bg-background px-3 focus-visible:outline-2 focus-visible:outline-ring aria-invalid:border-destructive" />
              </label>
              <label className="grid gap-1">
                <span>{t("settings.marketFinance.disputeResolutionCode")}</span>
                <input value={reason} onChange={(event) => setReason(event.target.value)}
                  aria-invalid={!!reason && !validReason} placeholder="service_refund" maxLength={80}
                  className="h-11 min-w-0 rounded-md border border-input bg-background px-3 font-mono text-xs focus-visible:outline-2 focus-visible:outline-ring aria-invalid:border-destructive" />
              </label>
              <label className="flex items-start gap-2 text-xs">
                <input type="checkbox" className="mt-0.5" checked={confirmed} onChange={(event) => setConfirmed(event.target.checked)} />
                {t("settings.marketFinance.disputeConfirm")}
              </label>
              {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button type="button" variant="outline" isDisabled={busy} onPress={() => setSelected(null)}>{t("common.cancel")}</Button>
              <Button type="submit" isDisabled={busy || !confirmed || !validReason || amountMinor == null || !withinRefundable} className="whitespace-nowrap">{busy ? t("common.loading") : t("settings.marketFinance.recordTransition")}</Button>
            </Modal.Footer>
          </form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  </section>;
}
