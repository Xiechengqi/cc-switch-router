"use client";

import * as React from "react";
import { Switch } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useLoginContextKey } from "@/lib/use-login-context";
import { OPERATION_LABELS, type ShareOperation } from "@/lib/share-capabilities";
import type { GrantCapabilityRequest, GrantCapabilityPreview } from "@/lib/share-grant-capabilities";

export function GrantCapabilityFields({
  shareId, revision, email, request, selected, reconfirm, quota, zh, disabled, onChange, onReconfirm,
}: {
  shareId?: string; revision?: number; email: string; request: GrantCapabilityRequest;
  selected: readonly ShareOperation[]; reconfirm: readonly ShareOperation[];
  quota: "tokens" | "usd" | "unlimited"; zh: boolean; disabled?: boolean;
  onChange: (operations: ShareOperation[]) => void; onReconfirm: (operation: ShareOperation) => void;
}) {
  const { t } = useLocaleText();
  const login = useLoginContextKey();
  const key = shareId && revision != null && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email) ? JSON.stringify([login, shareId, revision, email]) : "";
  const [attempt, setAttempt] = React.useState(0);
  const [result, setResult] = React.useState<{ key: string; preview?: GrantCapabilityPreview }>();
  React.useEffect(() => {
    if (!key || !shareId || revision == null) return;
    const abort = new AbortController();
    const delay = setTimeout(() => {
      void request(shareId, { expectedConfigRevision: revision, subject: email, grantCapabilityEdits: {} }, abort.signal)
        .then(preview => { if (!abort.signal.aborted) setResult({ key, preview }); })
        .catch(() => { if (!abort.signal.aborted) setResult({ key }); });
    }, 180);
    return () => { clearTimeout(delay); abort.abort(); };
  }, [attempt, email, key, request, revision, shareId]);
  if (!key) return null;
  const current = result?.key === key ? result : undefined;
  const preview = current?.preview;
  const reportedRows = preview?.rows ?? [];
  const rows = [...reportedRows, ...selected.filter(operation => !reportedRows.some(row => row.operation === operation))
    .map(operation => ({ operation, selected: true, available: false, needsReconfirm: false }))];
  if (preview?.ready && !preview.unavailable && rows.length === 0 && !selected.length) return null;
  const unavailable = !!current && (!preview?.ready || preview.unavailable);
  return (
    <fieldset className="min-w-0 space-y-2 border-t border-border pt-3 sm:col-span-2">
      <legend className="pr-2 text-xs font-medium">{t("dashboard.grantMedia.title")}</legend>
      {!current && <p role="status" className="text-xs text-muted-foreground">{t("dashboard.grantMedia.loading")}</p>}
      {unavailable && <p role="status" className="text-xs">{t("dashboard.grantMedia.unavailable")}{" "}
        <button className="underline" type="button" onClick={() => { setResult(undefined); setAttempt(a => a + 1); }}>{t("common.retry")}</button>
      </p>}
      {rows.map(row => {
        const enabled = selected.includes(row.operation);
        const label = OPERATION_LABELS[row.operation][zh ? 0 : 1];
        return (
          <div key={row.operation} className="grid gap-1">
            <Switch aria-label={label} className="flex min-h-8 w-full items-center justify-between gap-4"
              isSelected={enabled} isDisabled={disabled || (!enabled && (!preview?.ready || unavailable || !row.available))}
              onChange={checked => onChange(checked ? [...selected, row.operation] : selected.filter(op => op !== row.operation))}>
              <Switch.Content><span className="text-sm">{label}</span></Switch.Content>
              <Switch.Control><Switch.Thumb /></Switch.Control>
            </Switch>
            {enabled && row.needsReconfirm && !unavailable && !reconfirm.includes(row.operation) && <button type="button"
              disabled={disabled} className="justify-self-start text-xs underline" onClick={() => onReconfirm(row.operation)}>
              {t("dashboard.grantMedia.reconfirm")}
            </button>}
            {current && enabled && !row.available && <p className="text-xs text-muted-foreground">{t("dashboard.grantMedia.remove")}</p>}
          </div>
        );
      })}
      {selected.length > 0 && quota !== "unlimited" && <p className="text-xs text-muted-foreground">
        {quota === "tokens" ? (t("dashboard.grantMedia.tokenHint"))
          : quota === "usd" ? (t("dashboard.grantMedia.usdHint")) : null}
      </p>}
    </fieldset>
  );
}
