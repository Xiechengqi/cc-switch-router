"use client";

/* Owner editor for a channel's payment settings: order mode, allowed rails,
 * per-order limits and the fee used for gateway cost estimates. */

import * as React from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  MARKET_PAYMENT_TOKEN_IDS,
  updateMarketPaymentChannelSettings,
  type MarketPaymentChannel,
  type MarketTopupOrderMode,
} from "@/lib/market-wallet-api";
import { formatMinorUsd, parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";
import { paymentTokenLabel } from "@/lib/market-payment-tokens";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { OWNER_DIALOG_CLASS, OwnerField, ownerErrorText } from "@/components/settings/market-finance-shared";

function minorToInput(minor: number | null | undefined) {
  return minor == null ? "" : (minor / 100).toFixed(2);
}

/** One-line summary used on the channel card. */
export function channelSettingsSummary(channel: MarketPaymentChannel, t: ReturnType<typeof useLocaleText>["t"]) {
  const settings = channel.settings;
  if (!settings) return null;
  const methods = settings.allowedMethods.length
    ? t("settings.marketFinance.channel.methodsCount", { count: settings.allowedMethods.length })
    : t("settings.marketFinance.channel.methodsAll");
  const range = settings.minTopupMinor == null && settings.maxTopupMinor == null
    ? t("settings.marketFinance.channel.rangeAny")
    : `${settings.minTopupMinor == null ? "…" : formatMinorUsd(settings.minTopupMinor)}–${settings.maxTopupMinor == null ? "…" : formatMinorUsd(settings.maxTopupMinor)}`;
  const fee = t("settings.marketFinance.channel.feeValue", { bps: settings.feeRateBps, fixed: formatMinorUsd(settings.fixedFeeMinor) });
  return [t(`settings.marketFinance.channel.orderMode.${settings.orderMode}`), methods, range, fee].join(" · ");
}

export function MarketChannelSettingsDialog({ channel, onClose, onSaved }: {
  channel: MarketPaymentChannel | null;
  onClose: () => void;
  onSaved: () => Promise<unknown>;
}) {
  const { t } = useLocaleText();
  const [orderMode, setOrderMode] = React.useState<MarketTopupOrderMode>("cashier");
  const [methods, setMethods] = React.useState<string[]>([]);
  const [min, setMin] = React.useState("");
  const [max, setMax] = React.useState("");
  const [feeBps, setFeeBps] = React.useState("0");
  const [fixedFee, setFixedFee] = React.useState("0.00");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    const settings = channel?.settings;
    if (!channel) return;
    setOrderMode(settings?.orderMode ?? "cashier");
    setMethods(settings?.allowedMethods ?? []);
    setMin(minorToInput(settings?.minTopupMinor));
    setMax(minorToInput(settings?.maxTopupMinor));
    setFeeBps(String(settings?.feeRateBps ?? 0));
    setFixedFee(minorToInput(settings?.fixedFeeMinor ?? 0));
    setError("");
  }, [channel]);

  const minMinor = min.trim() ? parseNonNegativeUsdMinor(min) : null;
  const maxMinor = max.trim() ? parseNonNegativeUsdMinor(max) : null;
  const minInvalid = !!min.trim() && (minMinor == null || minMinor < 100);
  const maxInvalid = !!max.trim() && (maxMinor == null || maxMinor < 100);
  const rangeInvalid = minMinor != null && maxMinor != null && minMinor > maxMinor;
  const bps = /^\d{1,5}$/.test(feeBps.trim()) ? Number(feeBps.trim()) : null;
  const fixedMinor = parseNonNegativeUsdMinor(fixedFee || "0");
  const feeInvalid = bps == null || bps > 10_000 || fixedMinor == null;
  const methodsInvalid = orderMode === "direct" && methods.length === 0;
  const valid = !!channel?.settings && !minInvalid && !maxInvalid && !rangeInvalid && !feeInvalid && !methodsInvalid;

  const toggle = (tokenId: string, checked: boolean) =>
    setMethods((current) => checked ? [...current, tokenId] : current.filter((item) => item !== tokenId));

  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!channel?.settings || !valid || busy || bps == null || fixedMinor == null) return;
    setBusy(true);
    setError("");
    try {
      await updateMarketPaymentChannelSettings(channel.id, {
        orderMode,
        allowedMethods: MARKET_PAYMENT_TOKEN_IDS.filter((id) => methods.includes(id)),
        minTopupMinor: minMinor,
        maxTopupMinor: maxMinor,
        feeRateBps: bps,
        fixedFeeMinor: fixedMinor,
        expectedVersion: channel.settings.version,
      });
      await onSaved();
      onClose();
    } catch (cause) {
      setError(ownerErrorText(t, cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal.Backdrop isOpen={!!channel} onOpenChange={(open) => { if (!open && !busy) onClose(); }}>
      <Modal.Container placement="center">
        <Modal.Dialog className={OWNER_DIALOG_CLASS}>
          <Modal.CloseTrigger />
          <Modal.Header><Modal.Heading>{t("settings.marketFinance.channel.settingsTitle")}</Modal.Heading></Modal.Header>
          <form onSubmit={(event) => void submit(event)}>
            <Modal.Body className="grid max-h-[65vh] gap-4 overflow-y-auto text-sm">
              {channel ? <p className="font-medium">{channel.displayName}</p> : null}
              <label className="grid gap-1">
                <span className="font-medium">{t("settings.marketFinance.channel.orderMode")}</span>
                <select autoFocus value={orderMode} onChange={(event) => setOrderMode(event.target.value as MarketTopupOrderMode)} className={WALLET_INPUT}>
                  <option value="cashier">{t("settings.marketFinance.channel.orderMode.cashier")}</option>
                  <option value="direct">{t("settings.marketFinance.channel.orderMode.direct")}</option>
                </select>
              </label>
              <fieldset className="grid gap-2" aria-describedby={methodsInvalid ? "channel-methods-error" : undefined}>
                <legend className="mb-1 font-medium">{t("settings.marketFinance.channel.methods")}</legend>
                <div className="grid grid-cols-1 gap-1 sm:grid-cols-2">
                  {MARKET_PAYMENT_TOKEN_IDS.map((tokenId) => (
                    <label key={tokenId} className="flex min-h-11 items-center gap-2 rounded-lg border border-border px-3 has-[:checked]:border-accent/40 has-[:checked]:bg-accent/5">
                      <input type="checkbox" checked={methods.includes(tokenId)} onChange={(event) => toggle(tokenId, event.target.checked)} />
                      <span>{paymentTokenLabel(tokenId).label}</span>
                    </label>
                  ))}
                </div>
                {methodsInvalid ? <p id="channel-methods-error" className="text-xs text-destructive">{t("settings.marketFinance.channel.methodsRequired")}</p> : null}
              </fieldset>
              <div className="grid gap-3 sm:grid-cols-2">
                <OwnerField label={t("settings.marketFinance.channel.minTopup")} inputMode="decimal" value={min} onChange={(event) => setMin(event.target.value)} invalid={minInvalid || rangeInvalid} placeholder="1.00" />
                <OwnerField label={t("settings.marketFinance.channel.maxTopup")} inputMode="decimal" value={max} onChange={(event) => setMax(event.target.value)} invalid={maxInvalid || rangeInvalid} placeholder="1000.00" />
              </div>
              {rangeInvalid || minInvalid || maxInvalid ? <p className="text-xs text-destructive">{t("settings.marketFinance.channel.invalidRange")}</p> : null}
              <div className="grid gap-3 sm:grid-cols-2">
                <OwnerField label={t("settings.marketFinance.channel.feeRateBps")} inputMode="numeric" value={feeBps} onChange={(event) => setFeeBps(event.target.value)} invalid={bps == null || bps > 10_000} />
                <OwnerField label={t("settings.marketFinance.channel.fixedFee")} inputMode="decimal" value={fixedFee} onChange={(event) => setFixedFee(event.target.value)} invalid={fixedMinor == null} />
              </div>
              <p className={feeInvalid ? "text-xs text-destructive" : "text-xs text-muted-foreground"}>
                {t(feeInvalid ? "settings.marketFinance.channel.invalidFee" : "settings.marketFinance.channel.feeHelp")}
              </p>
              {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
            </Modal.Body>
            <Modal.Footer>
              <Button type="button" variant="outline" isDisabled={busy} onPress={onClose}>{t("common.cancel")}</Button>
              <Button type="submit" isDisabled={busy || !valid}>
                {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                {t("common.save")}
              </Button>
            </Modal.Footer>
          </form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
