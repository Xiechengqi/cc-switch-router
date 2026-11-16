"use client";

import * as React from "react";
import { Globe2, Loader2, Server, Sparkles } from "lucide-react";
import { Button, Input, Modal } from "@heroui/react";
import { ChoiceCard } from "@/components/common/choice-card";
import { FIELD_LABEL_CLASS } from "@/components/dashboard/share-edit/share-edit-shared";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  deleteShareModelPrice,
  getShareModelPriceSuggestions,
  getShareModelPrices,
  upsertAdminModelPrice,
  upsertShareModelPrice,
} from "@/lib/api";
import {
  EMPTY_PRICE_DRAFT,
  PRICE_RATE_FIELDS,
  modelPriceErrorKey,
  modelPriceErrorMessage,
  priceDraftFromRates,
  validatePriceDraft,
  type PriceDraft,
  type PriceRateField,
  type PriceScope,
  type SharePriceAccess,
} from "@/lib/share-model-prices";
import type { OwnerModelPrice, ShareModelPriceSuggestion } from "@/lib/types";
import { cn, formatDateTime } from "@/lib/utils";

/** `modelKey` is the key the price is stored under: an owner price's own key when editing one. */
export type ModelPriceTarget = { modelKey: string; displayName?: string };
export type ModelPriceSaved = { scope: PriceScope; effectiveFromMs: number; removed?: boolean };

type Loadable<T> = { state: "idle" | "loading" | "failed" } | { state: "ready"; value: T };

/**
 * Prices one model from a Share's usage breakdown. "This client" writes an
 * owner price (every Share on the Share's installation, optimistic version
 * check); "Router-wide" is the admin catalog price and only replaces a
 * missing one. Both re-price this model's $0 usage in windows that have not
 * ended once the new tariff activates.
 */
export function ShareModelPriceDialog({
  target,
  shareId,
  access,
  onClose,
  onSaved,
}: {
  target: ModelPriceTarget | null;
  shareId?: string;
  access: SharePriceAccess;
  onClose: () => void;
  onSaved: (result: ModelPriceSaved) => void;
}) {
  const { t, locale } = useLocaleText();
  const baseId = React.useId();
  const modelKey = target?.modelKey ?? "";
  const clientAllowed = access.client && !!shareId;
  const [scope, setScope] = React.useState<PriceScope>("client");
  const [draft, setDraft] = React.useState<PriceDraft>(EMPTY_PRICE_DRAFT);
  const [touched, setTouched] = React.useState<Partial<Record<PriceRateField, true>>>({});
  const [submitted, setSubmitted] = React.useState(false);
  const [existing, setExisting] = React.useState<Loadable<OwnerModelPrice | null>>({ state: "idle" });
  const [suggestions, setSuggestions] = React.useState<Loadable<ShareModelPriceSuggestion[]>>({ state: "idle" });
  const [busy, setBusy] = React.useState(false);
  const [confirmRemove, setConfirmRemove] = React.useState(false);
  const [error, setError] = React.useState<{ message: string; detail?: string } | null>(null);
  const dirty = React.useRef(false);

  const loadExisting = React.useCallback(async (signal?: AbortSignal, prefill = true) => {
    if (!shareId || !modelKey) return undefined;
    setExisting({ state: "loading" });
    try {
      const response = await getShareModelPrices(shareId, signal);
      // The Router stores keys trimmed and lowercased.
      const wanted = modelKey.trim().toLowerCase();
      const price = (response.prices || []).find((item) => item.modelKey === wanted) ?? null;
      if (signal?.aborted) return undefined;
      setExisting({ state: "ready", value: price });
      // Never overwrite what the person already typed.
      if (prefill && price && !dirty.current) setDraft(priceDraftFromRates(price.rates));
      return price;
    } catch {
      if (!signal?.aborted) setExisting({ state: "failed" });
      return undefined;
    }
  }, [modelKey, shareId]);

  React.useEffect(() => {
    if (!target) return;
    setScope(clientAllowed ? "client" : "global");
    setDraft(EMPTY_PRICE_DRAFT);
    setTouched({});
    setSubmitted(false);
    setError(null);
    setConfirmRemove(false);
    setExisting({ state: "idle" });
    setSuggestions({ state: "idle" });
    dirty.current = false;
    if (!shareId) return;
    const controller = new AbortController();
    if (clientAllowed) void loadExisting(controller.signal);
    setSuggestions({ state: "loading" });
    getShareModelPriceSuggestions(shareId, target.modelKey, controller.signal)
      .then((response) => {
        if (!controller.signal.aborted) setSuggestions({ state: "ready", value: (response.suggestions || []).slice(0, 5) });
      })
      .catch(() => {
        if (!controller.signal.aborted) setSuggestions({ state: "failed" });
      });
    return () => controller.abort();
    // `target` identity marks one opening of the dialog.
  }, [target, shareId, clientAllowed, loadExisting]);

  const validation = validatePriceDraft(draft);
  const ownerPrice = existing.state === "ready" ? existing.value : null;
  const showError = (field: PriceRateField) => (submitted || touched[field]) && validation.errors[field];

  const updateField = (field: PriceRateField, value: string) => {
    dirty.current = true;
    setConfirmRemove(false);
    setDraft((current) => ({ ...current, [field]: value }));
  };

  const fail = (cause: unknown, failedScope: PriceScope) => {
    const { key, params, field, detail } = modelPriceErrorMessage(cause, failedScope);
    const values = field ? { ...params, field: t(`dashboard.modelPrice.rate.${field}`) } : params;
    setError({ message: t(key, values), detail });
  };

  const save = async () => {
    setSubmitted(true);
    setError(null);
    if (!target || !validation.rates || busy) return;
    const displayName = target.displayName?.trim() || target.modelKey;
    setBusy(true);
    try {
      if (scope === "client" && shareId) {
        try {
          const result = await upsertShareModelPrice(shareId, target.modelKey, {
            displayName,
            rates: validation.rates,
            expectedVersion: ownerPrice?.version ?? null,
          });
          onSaved({ scope, effectiveFromMs: result.effectiveFromMs });
        } catch (cause) {
          fail(cause, scope);
          // Load the newer version so the next save replaces it; keep the entries.
          if (modelPriceErrorKey(cause, scope) === "dashboard.modelPrice.conflict.client") await loadExisting(undefined, false);
        }
      } else {
        try {
          const result = await upsertAdminModelPrice(target.modelKey, { displayName, expectUnpriced: true, rates: validation.rates });
          onSaved({ scope: "global", effectiveFromMs: result.effectiveFrom });
        } catch (cause) {
          fail(cause, "global");
        }
      }
    } finally {
      setBusy(false);
    }
  };

  const remove = async () => {
    if (!target || !shareId || !ownerPrice || busy) return;
    if (!confirmRemove) { setConfirmRemove(true); return; }
    setBusy(true);
    setError(null);
    try {
      const result = await deleteShareModelPrice(shareId, target.modelKey, ownerPrice.version);
      onSaved({ scope: "client", effectiveFromMs: result.effectiveFromMs, removed: true });
    } catch (cause) {
      fail(cause, "client");
      setConfirmRemove(false);
      if (modelPriceErrorKey(cause, "client") === "dashboard.modelPrice.conflict.client") await loadExisting(undefined, false);
    } finally {
      setBusy(false);
    }
  };

  const scopeName = `${baseId}-scope`;
  const scopeHeadingId = `${baseId}-scope-heading`;
  const ratesHeadingId = `${baseId}-rates-heading`;
  const effectId = `${baseId}-effect`;
  const positiveId = `${baseId}-positive`;
  const suggestionList = suggestions.state === "ready" ? suggestions.value : [];

  return (
    <Modal.Backdrop isOpen={!!target} onOpenChange={(open) => { if (!open && !busy) onClose(); }} isDismissable={!busy}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(600px,calc(100vw-2rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
          <Modal.CloseTrigger />
          <Modal.Header className="grid gap-1.5">
            <Modal.Heading>{t(ownerPrice ? "dashboard.userLimit.price.editTitle" : "dashboard.userLimit.price.title")}</Modal.Heading>
            <div className="flex min-w-0 flex-wrap items-center gap-2 text-xs">
              <span className="sr-only">{t("dashboard.modelPrice.model")}</span>
              <span className="min-w-0 break-all rounded-md border border-border bg-muted/60 px-2 py-0.5 font-mono text-foreground">{modelKey}</span>
              {target?.displayName && target.displayName !== modelKey ? (
                <span className="min-w-0 break-words text-muted-foreground">{target.displayName}</span>
              ) : null}
            </div>
          </Modal.Header>
          <Modal.Body className="grid max-h-[min(72vh,640px)] gap-5 overflow-y-auto">
            {clientAllowed && access.global ? (
              <section className="grid gap-2">
                <span id={scopeHeadingId} className={FIELD_LABEL_CLASS}>{t("dashboard.modelPrice.scope")}</span>
                <div role="radiogroup" aria-labelledby={scopeHeadingId} className="grid gap-2 sm:grid-cols-2">
                  <ChoiceCard
                    name={scopeName}
                    value="client"
                    checked={scope === "client"}
                    disabled={busy}
                    onChange={() => { setScope("client"); setError(null); }}
                    icon={Server}
                    title={t("dashboard.modelPrice.scope.client")}
                    description={t("dashboard.modelPrice.scope.clientHint")}
                    selectedLabel={t("dashboard.modelPrice.scope.selected")}
                  />
                  <ChoiceCard
                    name={scopeName}
                    value="global"
                    checked={scope === "global"}
                    disabled={busy}
                    onChange={() => { setScope("global"); setError(null); setConfirmRemove(false); }}
                    icon={Globe2}
                    title={t("dashboard.modelPrice.scope.global")}
                    description={t("dashboard.modelPrice.scope.globalHint")}
                    selectedLabel={t("dashboard.modelPrice.scope.selected")}
                  />
                </div>
              </section>
            ) : (
              <p className="text-xs leading-relaxed text-muted-foreground">
                <span className="font-medium text-foreground">
                  {t("dashboard.modelPrice.scope")}: {t(scope === "client" ? "dashboard.modelPrice.scope.client" : "dashboard.modelPrice.scope.global")}
                </span>
                {" · "}
                {t(scope === "client" ? "dashboard.modelPrice.scope.clientHint" : "dashboard.modelPrice.scope.globalHint")}
              </p>
            )}

            {scope === "client" ? (
              existing.state === "loading" ? (
                <p role="status" className="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Loader2 className="h-3.5 w-3.5 animate-spin motion-reduce:animate-none" aria-hidden />
                  {t("dashboard.modelPrice.existingLoading")}
                </p>
              ) : existing.state === "failed" ? (
                <p role="status" className="text-xs text-amber-800">{t("dashboard.modelPrice.existingFailed")}</p>
              ) : ownerPrice?.shadowed ? (
                <p role="status" className="flex gap-2 rounded-lg border border-amber-300/60 bg-amber-50 px-3 py-2 text-xs leading-relaxed text-amber-900">
                  <span aria-hidden className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-amber-500" />
                  <span>{t("dashboard.modelPrice.shadowed")}</span>
                </p>
              ) : ownerPrice ? (
                <p role="status" className="text-xs text-muted-foreground">
                  {t("dashboard.modelPrice.existing", { time: formatDateTime(ownerPrice.updatedAtMs, locale) })}
                </p>
              ) : null
            ) : null}

            {suggestions.state === "loading" || suggestionList.length ? (
              <section className="grid gap-2" aria-busy={suggestions.state === "loading"}>
                <div className="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-0.5">
                  <span className={cn(FIELD_LABEL_CLASS, "inline-flex items-center gap-1.5")}>
                    <Sparkles className="h-3.5 w-3.5 text-accent" aria-hidden />
                    {t("dashboard.modelPrice.suggestions")}
                  </span>
                  <span className="text-[11px] text-muted-foreground">{t("dashboard.modelPrice.suggestionsHint")}</span>
                </div>
                {suggestions.state === "loading" ? (
                  <p role="status" className="text-xs text-muted-foreground">{t("dashboard.modelPrice.suggestionsLoading")}</p>
                ) : (
                  <ul className="grid gap-1.5 sm:grid-cols-2">
                    {suggestionList.map((suggestion) => (
                      <li key={suggestion.modelKey} className="min-w-0">
                        <button
                          type="button"
                          disabled={busy}
                          aria-label={t("dashboard.modelPrice.suggestionUse", { name: suggestion.displayName || suggestion.modelKey })}
                          onClick={() => {
                            dirty.current = true;
                            setConfirmRemove(false);
                            setDraft(priceDraftFromRates(suggestion.rates));
                          }}
                          className="group grid min-h-11 w-full min-w-0 gap-0.5 rounded-lg border border-border bg-card px-3 py-2 text-left transition-all duration-200 hover:-translate-y-0.5 hover:border-accent/30 hover:shadow-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2 active:scale-[0.98] disabled:pointer-events-none disabled:opacity-60 motion-reduce:transition-none motion-reduce:hover:translate-y-0"
                        >
                          <span className="truncate text-xs font-medium text-foreground group-hover:text-accent">{suggestion.displayName || suggestion.modelKey}</span>
                          <span className="truncate font-mono text-[11px] text-muted-foreground">
                            {t("dashboard.modelPrice.suggestionRates", { input: suggestion.rates.inputUsdPer1m, output: suggestion.rates.outputUsdPer1m })}
                          </span>
                        </button>
                      </li>
                    ))}
                  </ul>
                )}
              </section>
            ) : null}

            <fieldset className="grid gap-3" aria-describedby={submitted && validation.allZero ? positiveId : undefined}>
              <legend id={ratesHeadingId} className={cn(FIELD_LABEL_CLASS, "mb-2")}>{t("dashboard.modelPrice.rates")}</legend>
              <div className="grid gap-3 sm:grid-cols-2">
                {PRICE_RATE_FIELDS.map((field) => {
                  const inputId = `${baseId}-${field}`;
                  const errorId = `${inputId}-error`;
                  const fieldError = showError(field);
                  return (
                    <div key={field} className="grid gap-1">
                      <label htmlFor={inputId} className="text-xs font-medium text-foreground">{t(`dashboard.modelPrice.rate.${field}`)}</label>
                      <div className="relative">
                        <span aria-hidden="true" className="pointer-events-none absolute left-3 top-1/2 z-10 -translate-y-1/2 text-sm text-muted-foreground">$</span>
                        <Input
                          id={inputId}
                          type="text"
                          inputMode="decimal"
                          autoComplete="off"
                          className="w-full pl-6 font-mono"
                          placeholder="0"
                          disabled={busy}
                          value={draft[field]}
                          aria-invalid={fieldError ? true : undefined}
                          aria-describedby={fieldError ? errorId : undefined}
                          onChange={(event) => updateField(field, event.target.value)}
                          onBlur={() => setTouched((current) => ({ ...current, [field]: true }))}
                        />
                      </div>
                      {fieldError ? (
                        <p id={errorId} className="text-xs text-danger">{t(`dashboard.modelPrice.error.${fieldError}`)}</p>
                      ) : null}
                    </div>
                  );
                })}
              </div>
              {submitted && validation.allZero ? (
                <p id={positiveId} role="alert" className="text-xs text-danger">{t("dashboard.modelPrice.error.positive")}</p>
              ) : null}
            </fieldset>

            <p id={effectId} className="flex gap-2 rounded-lg border border-accent/20 bg-accent/5 px-3 py-2 text-xs leading-relaxed text-foreground/80">
              <span aria-hidden className="mt-1.5 h-1.5 w-1.5 shrink-0 rounded-full bg-accent" />
              <span>{t("dashboard.modelPrice.effect")}</span>
            </p>

            {error ? (
              <div role="alert" className="grid gap-1 rounded-lg border border-danger/30 bg-danger/5 px-3 py-2 text-xs text-danger">
                <span>{error.message}</span>
                {error.detail ? <span className="break-words font-mono text-[11px] text-danger/80">{error.detail}</span> : null}
              </div>
            ) : null}
          </Modal.Body>
          <Modal.Footer className="flex flex-wrap items-center gap-2">
            {scope === "client" && ownerPrice ? (
              <Button variant="ghost" className="mr-auto text-danger" isDisabled={busy} onPress={() => void remove()}>
                {t(confirmRemove ? "dashboard.modelPrice.removeConfirm" : "dashboard.modelPrice.remove")}
              </Button>
            ) : null}
            <Button variant="outline" isDisabled={busy} onPress={onClose}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy} aria-describedby={effectId} onPress={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
