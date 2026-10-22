"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import {
  getWalletPolicy,
  saveWalletPolicy,
  type WalletPolicy,
} from "@/lib/wallet-policy-api";

function hundredths(value: string): number | null {
  if (!/^\d{1,6}(\.\d{1,2})?$/.test(value)) return null;
  const [whole, fraction = ""] = value.split(".");
  return Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
}
export function WalletPolicySettings() {
  const { t } = useLocaleText();
  const [policy, setPolicy] = React.useState<WalletPolicy | null>(null);
  const [fee, setFee] = React.useState("");
  const [single, setSingle] = React.useState("");
  const [daily, setDaily] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const [stale, setStale] = React.useState(false);
  const [attempt, setAttempt] = React.useState(0);
  const load = (value: WalletPolicy) => {
    setPolicy(value);
    setFee((value.maintenanceFeeBps / 100).toString());
    setSingle((value.transferMaxMinor / 100).toString());
    setDaily((value.transferDailyMaxMinor / 100).toString());
  };
  React.useEffect(() => {
    let active = true;
    getWalletPolicy(true)
      .then((value) => {
        if (active) {
          load(value);
          setError("");
        }
      })
      .catch((cause) => {
        if (active)
          setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      active = false;
    };
  }, [attempt]);
  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    if (busy || !policy || policy.canEdit === false) return;
    const rate = hundredths(fee),
      per = hundredths(single),
      day = hundredths(daily);
    if (
      rate == null ||
      rate > 10000 ||
      per == null ||
      day == null ||
      per < 1 ||
      per > 10000000 ||
      day < per ||
      day > 10000000
    ) {
      setError(t("walletPolicy.invalid"));
      return;
    }
    setBusy(true);
    setError("");
    setSaved(false);
    setStale(false);
    try {
      load({
        ...(await saveWalletPolicy({
          ...policy,
          maintenanceFeeBps: rate,
          transferMaxMinor: per,
          transferDailyMaxMinor: day,
        })),
        canEdit: true,
      });
      setSaved(true);
    } catch (cause) {
      const changed = cause instanceof ApiError && cause.status === 409;
      setStale(changed);
      setError(
        changed
          ? t("walletPolicy.changed")
          : cause instanceof Error
            ? cause.message
            : String(cause),
      );
    } finally {
      setBusy(false);
    }
  };
  return (
    <section className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5">
      <div>
        <h3 className="text-base font-semibold">{t("walletPolicy.title")}</h3>
        <p className="mt-1 text-sm leading-6 text-muted-foreground">
          {t("walletPolicy.help")}
        </p>
      </div>
      {error ? (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      {saved ? (
        <p role="status" className="text-sm">
          {t("walletPolicy.saved")}
        </p>
      ) : null}
      {policy?.canEdit === false ? (
        <p className="text-xs text-muted-foreground">
          {t("walletPolicy.readOnly")}
        </p>
      ) : null}
      {stale ? (
        <Button
          variant="outline"
          className="justify-self-start"
          isDisabled={busy}
          onPress={() => {
            setStale(false);
            setSaved(false);
            setAttempt((value) => value + 1);
          }}
        >
          {t("common.reload")}
        </Button>
      ) : null}
      {!policy ? (
        <Button
          variant="outline"
          className="justify-self-start"
          onPress={() => setAttempt((value) => value + 1)}
        >
          {t(error ? "common.retry" : "common.loading")}
        </Button>
      ) : (
        <form onSubmit={save} className="grid min-w-0 gap-4 sm:grid-cols-2">
          {(
            [
              { key: "walletPolicy.rate", value: fee, change: setFee },
              {
                key: "walletPolicy.singleLimit",
                value: single,
                change: setSingle,
              },
              {
                key: "walletPolicy.dailyLimit",
                value: daily,
                change: setDaily,
              },
            ] as const
          ).map((item) => (
            <label key={item.key} className="grid min-w-0 gap-1 text-sm">
              <span>{t(item.key)}</span>
              <input
                inputMode="decimal"
                required
                className={WALLET_INPUT}
                value={item.value}
                onChange={(event) => {
                  item.change(event.target.value);
                  setSaved(false);
                }}
                disabled={busy || policy.canEdit === false}
              />
            </label>
          ))}
          <div className="grid content-start gap-1">
            {(
              [
                { key: "transfersEnabled", label: "walletPolicy.enabled" },
                { key: "transfersPaused", label: "walletPolicy.paused" },
                { key: "earningsConversionsPaused", label: "earningConversion.pauseSetting" },
              ] as const
            ).map((item) => (
              <label
                key={item.key}
                className="flex min-h-11 items-center gap-2 text-sm"
              >
                <input
                  type="checkbox"
                  className="size-4 accent-primary"
                  checked={policy[item.key]}
                  onChange={(event) => {
                    setPolicy({ ...policy, [item.key]: event.target.checked });
                    setSaved(false);
                  }}
                  disabled={busy || policy.canEdit === false}
                />
                {t(item.label)}
              </label>
            ))}
          </div>
          <Button
            type="submit"
            variant="primary"
            className="min-h-11 justify-self-start whitespace-nowrap"
            isDisabled={busy || policy.canEdit === false}
          >
            {t(busy ? "common.loading" : "common.save")}
          </Button>
        </form>
      )}
    </section>
  );
}
