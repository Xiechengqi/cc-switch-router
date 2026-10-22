"use client";
/* Hallmark · component: commission policy · genre: modern-minimal · theme: existing Router
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: pass (40–41), computed text and offset keyboard focus outline
 * pre-emit critique: P4 H4 E4 S5 R5 V3
 */
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import { getCommissionPolicy, saveCommissionPolicy, type CommissionPolicy } from "@/lib/wallet-policy-api";

function validRate(rate: string) {
  return /^\d{1,3}(\.\d{1,2})?$/.test(rate) && Number(rate) <= 100;
}

type FormProps = {
  policy: CommissionPolicy | null;
  rate: string;
  busy: boolean;
  error: string;
  saved: boolean;
  stale: boolean;
  onSave: (event: React.FormEvent) => void;
  onRateChange: (value: string) => void;
  onRetry: () => void;
  previewState?: "hover" | "focus" | "active";
};

/** Pure view, also used by the non-routed eight-state preview. */
export function CommissionPolicyForm({ policy, rate, busy, error, saved, stale, onSave, onRateChange, onRetry, previewState }: FormProps) {
  const { locale, t } = useLocaleText();
  const id = React.useId();
  const titleId = `${id}-title`;
  const errorId = `${id}-error`;
  const noticeId = `${id}-notice`;
  return (
    <section className="grid min-w-0 gap-4 rounded-xl border border-border bg-card p-5" aria-labelledby={titleId} aria-busy={busy}>
      <div className="grid gap-1">
        <h3 id={titleId} className="min-w-0 break-words text-base font-semibold">{t("commission.title")}</h3>
        <p className="text-sm leading-6 text-muted-foreground">{t("commission.help")}</p>
      </div>
      {!policy && !error ? <p role="status" className="text-sm text-muted-foreground">{t("common.loading")}</p> : null}
      <p id={errorId} role={error ? "alert" : undefined} className="min-h-6 text-sm text-destructive">{error}</p>
      {policy ? (
        <>
          <p className="text-sm tabular-nums">{t("commission.current", { rate: (policy.current.rateBps / 100).toString() })}</p>
          {policy.scheduled ? <p className="text-sm text-muted-foreground" role="status">{t("commission.scheduled", { rate: (policy.scheduled.rateBps / 100).toString(), time: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(policy.scheduled.effectiveAt)) })}</p> : null}
          {!policy.canEdit ? <p className="text-sm text-muted-foreground">{t("walletPolicy.readOnly")}</p> : null}
          <form onSubmit={onSave} className="grid min-w-0 gap-3">
            <label className="grid max-w-xs gap-1 text-sm">
              <span>{t("commission.rate")}</span>
              <input
                className={`${WALLET_INPUT} hover:bg-muted focus-visible:transition-none data-[preview=hover]:bg-muted data-[preview=focus]:outline-ring data-[preview=active]:outline-ring`}
                inputMode="decimal"
                value={rate}
                aria-invalid={!!error && !validRate(rate)}
                aria-describedby={`${noticeId} ${errorId}`}
                disabled={!policy.canEdit || busy || stale}
                data-preview={previewState}
                onChange={(event) => onRateChange(event.target.value)}
              />
            </label>
            <p id={noticeId} className="text-sm leading-6 text-muted-foreground">{t("commission.notice")}</p>
            <div className="flex flex-wrap items-center gap-3">
              <Button
                type="submit"
                className="min-h-11 whitespace-nowrap disabled:cursor-not-allowed data-[preview=hover]:brightness-95 data-[preview=focus]:outline-2 data-[preview=focus]:outline-offset-2 data-[preview=focus]:outline-ring data-[preview=active]:brightness-90"
                variant="primary"
                data-preview={previewState}
                isDisabled={!policy.canEdit || busy || stale}
                isPending={busy}
              >{t("common.save")}</Button>
              {saved ? <span role="status" className="text-sm">{t("commission.saved")}</span> : null}
              {stale ? <Button className="min-h-11 whitespace-nowrap" variant="ghost" onPress={onRetry}>{t("common.retry")}</Button> : null}
            </div>
          </form>
        </>
      ) : error ? <Button className="min-h-11 justify-self-start whitespace-nowrap" variant="ghost" onPress={onRetry}>{t("common.retry")}</Button> : null}
    </section>
  );
}

export function CommissionPolicySettings() {
  const { t } = useLocaleText();
  const [policy, setPolicy] = React.useState<CommissionPolicy | null>(null);
  const [rate, setRate] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const [stale, setStale] = React.useState(false);
  const [attempt, setAttempt] = React.useState(0);
  const load = (value: CommissionPolicy) => {
    setPolicy(value);
    setRate(((value.scheduled ?? value.current).rateBps / 100).toString());
  };
  React.useEffect(() => {
    let active = true;
    getCommissionPolicy(true).then((value) => {
      if (active) { load(value); setError(""); setStale(false); }
    }).catch((cause) => {
      if (active) setError(cause instanceof Error ? cause.message : String(cause));
    });
    return () => { active = false; };
  }, [attempt]);

  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!policy?.canEdit || busy || stale) return;
    if (!validRate(rate)) { setError(t("commission.invalid")); return; }
    const [whole, fraction = ""] = rate.split(".");
    const bps = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
    setBusy(true); setSaved(false); setError("");
    try {
      load(await saveCommissionPolicy(policy.version, bps));
      setSaved(true);
    } catch (cause) {
      const changed = cause instanceof ApiError && cause.status === 409;
      setStale(changed);
      setError(changed ? t("walletPolicy.changed") : cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
    }
  };
  return <CommissionPolicyForm policy={policy} rate={rate} busy={busy} error={error} saved={saved} stale={stale}
    onSave={(event) => void save(event)}
    onRateChange={(value) => { setRate(value); setSaved(false); setError(""); }}
    onRetry={() => setAttempt((n) => n + 1)} />;
}
