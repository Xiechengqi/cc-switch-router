"use client";

import * as React from "react";
import { Button, Checkbox, Chip, Modal, toast } from "@heroui/react";
import {
  AlertTriangle,
  Check,
  Loader2,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Search,
  ShieldCheck,
  X,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getMarketAccessDashboard,
  updateMarketAccessPolicy,
  updateMarketCounterparty,
  updateMarketCounterpartyCredit,
  updateMarketPublicCredit,
  upsertMarketCounterparty,
} from "@/lib/api";
import type {
  MarketAccessDashboard,
  MarketAccessDecision,
  MarketAccessPolicy,
  MarketAccessPricingKind,
  MarketAccessProductKind,
  MarketCounterparty,
  MarketCreditKind,
  MarketCreditLine,
  MarketPublicCreditLine,
} from "@/lib/types";
import { cn } from "@/lib/utils";

type Currency = "CNY" | "USD";
type ScopeKey = `${MarketAccessProductKind}:${MarketAccessPricingKind}`;

type CreditDraft = {
  kind: MarketCreditKind;
  limit: string;
  unlimitedAcknowledged: boolean;
};

type CounterpartyDraft = {
  status: "active" | "revoked";
  decisions: Record<ScopeKey, MarketAccessDecision>;
  credits: Record<Currency, CreditDraft>;
};

type CounterpartyChange = {
  statusChanged: boolean;
  accessRules: Array<{
    productKind: MarketAccessProductKind;
    pricingKind: MarketAccessPricingKind;
    decision: MarketAccessDecision;
  }>;
  creditCurrencies: Currency[];
};

const ACCESS_SCOPES: ReadonlyArray<{
  productKind: MarketAccessProductKind;
  pricingKind: MarketAccessPricingKind;
}> = [
  { productKind: "share", pricingKind: "free" },
  { productKind: "share", pricingKind: "paid" },
  { productKind: "client_host", pricingKind: "free" },
  { productKind: "client_host", pricingKind: "paid" },
];

const CURRENCIES: Currency[] = ["CNY", "USD"];

function parseLimitMinor(value: string) {
  if (!/^\d+(?:\.\d{1,2})?$/.test(value.trim())) return null;
  const minor = Math.round(Number(value) * 100);
  return Number.isSafeInteger(minor) && minor >= 1 && minor <= 100_000_000 ? minor : null;
}

function formatMoney(value: number, currency: string, locale: string) {
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: currency === "CNY" ? "CNY" : "USD",
  }).format(value / 100);
}

function scopeKey(
  productKind: MarketAccessProductKind,
  pricingKind: MarketAccessPricingKind,
): ScopeKey {
  return `${productKind}:${pricingKind}`;
}

function counterpartyDecision(
  counterparty: MarketCounterparty,
  productKind: MarketAccessProductKind,
  pricingKind: MarketAccessPricingKind,
) {
  return (
    counterparty.accessRules.find(
      (rule) => rule.productKind === productKind && rule.pricingKind === pricingKind,
    )?.decision || "inherit"
  );
}

function counterpartyCreditLine(counterparty: MarketCounterparty, currency: Currency) {
  return counterparty.creditLines.find((line) => line.currency === currency);
}

function creditDraft(line?: MarketCreditLine): CreditDraft {
  return {
    kind: line?.kind || "none",
    limit: line?.limitMinor != null ? (line.limitMinor / 100).toFixed(2) : "",
    unlimitedAcknowledged: false,
  };
}

function buildCounterpartyDraft(counterparty: MarketCounterparty): CounterpartyDraft {
  return {
    status: counterparty.status === "revoked" ? "revoked" : "active",
    decisions: {
      "share:free": counterpartyDecision(counterparty, "share", "free"),
      "share:paid": counterpartyDecision(counterparty, "share", "paid"),
      "client_host:free": counterpartyDecision(counterparty, "client_host", "free"),
      "client_host:paid": counterpartyDecision(counterparty, "client_host", "paid"),
    },
    credits: {
      CNY: creditDraft(counterpartyCreditLine(counterparty, "CNY")),
      USD: creditDraft(counterpartyCreditLine(counterparty, "USD")),
    },
  };
}

function buildCounterpartyDrafts(counterparties: MarketCounterparty[]) {
  return Object.fromEntries(
    counterparties.map((counterparty) => [counterparty.id, buildCounterpartyDraft(counterparty)]),
  ) as Record<string, CounterpartyDraft>;
}

function creditChanged(line: MarketCreditLine | undefined, draft: CreditDraft) {
  const currentKind = line?.kind || "none";
  if (draft.kind !== currentKind) return true;
  if (draft.kind !== "limited") return false;
  return parseLimitMinor(draft.limit) !== line?.limitMinor;
}

function counterpartyChange(
  counterparty: MarketCounterparty,
  draft?: CounterpartyDraft,
): CounterpartyChange {
  if (!draft) {
    return { statusChanged: false, accessRules: [], creditCurrencies: [] };
  }
  return {
    statusChanged:
      draft.status !== (counterparty.status === "revoked" ? "revoked" : "active"),
    accessRules: ACCESS_SCOPES.filter(
      ({ productKind, pricingKind }) =>
        draft.decisions[scopeKey(productKind, pricingKind)] !==
        counterpartyDecision(counterparty, productKind, pricingKind),
    ).map(({ productKind, pricingKind }) => ({
      productKind,
      pricingKind,
      decision: draft.decisions[scopeKey(productKind, pricingKind)],
    })),
    creditCurrencies: CURRENCIES.filter((currency) =>
      creditChanged(counterpartyCreditLine(counterparty, currency), draft.credits[currency]),
    ),
  };
}

function hasCounterpartyChange(change: CounterpartyChange) {
  return change.statusChanged || change.accessRules.length > 0 || change.creditCurrencies.length > 0;
}

function CounterpartyCreditCell({
  line,
  currency,
  draft,
  disabled,
  onChange,
}: {
  line?: MarketCreditLine;
  currency: Currency;
  draft: CreditDraft;
  disabled?: boolean;
  onChange: (draft: CreditDraft) => void;
}) {
  const { t } = useLocaleText();
  const needsUnlimitedAcknowledgement =
    draft.kind === "unlimited" && (line?.kind || "none") !== "unlimited";

  return (
    <div className="grid min-w-52 gap-2">
      <select
        value={draft.kind}
        disabled={disabled}
        aria-label={`${currency} ${t("marketAccess.creditType")}`}
        onChange={(event) =>
          onChange({
            ...draft,
            kind: event.target.value as MarketCreditKind,
            unlimitedAcknowledged: false,
          })
        }
        className="h-9 w-full rounded-md border border-border bg-white px-2 text-xs text-foreground disabled:bg-slate-50"
      >
        <option value="none">{t("marketAccess.credit.none")}</option>
        <option value="limited">{t("marketAccess.credit.limited")}</option>
        <option value="unlimited">{t("marketAccess.credit.unlimited")}</option>
      </select>
      {draft.kind === "limited" ? (
        <input
          value={draft.limit}
          disabled={disabled}
          inputMode="decimal"
          aria-label={`${currency} ${t("marketAccess.limitMajor")}`}
          placeholder={t("marketAccess.limitMajor")}
          onChange={(event) => onChange({ ...draft, limit: event.target.value })}
          className="h-9 w-full rounded-md border border-border bg-white px-2 text-xs text-foreground disabled:bg-slate-50"
        />
      ) : needsUnlimitedAcknowledgement ? (
        <Checkbox
          isSelected={draft.unlimitedAcknowledged}
          onChange={(selected) => onChange({ ...draft, unlimitedAcknowledged: selected })}
          isDisabled={disabled}
          className="text-[11px] leading-4"
        >
          <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
          {t("marketAccess.unlimitedConfirm")}
        </Checkbox>
      ) : null}
    </div>
  );
}

function PublicCreditEditor({
  currency,
  line,
  onSaved,
}: {
  currency: Currency;
  line?: MarketPublicCreditLine;
  onSaved: (dashboard: MarketAccessDashboard) => void;
}) {
  const { t } = useLocaleText();
  const [enabled, setEnabled] = React.useState(line?.enabled || false);
  const [limit, setLimit] = React.useState(line?.limitMinor ? (line.limitMinor / 100).toFixed(2) : "");
  const [acknowledged, setAcknowledged] = React.useState(false);
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    setEnabled(line?.enabled || false);
    setLimit(line?.limitMinor ? (line.limitMinor / 100).toFixed(2) : "");
    setAcknowledged(false);
  }, [line?.enabled, line?.limitMinor, line?.revision]);

  const save = async () => {
    const limitMinor = enabled ? parseLimitMinor(limit) : undefined;
    if (enabled && limitMinor == null) {
      toast.danger(t("marketAccess.invalidLimit"));
      return;
    }
    if (enabled && !acknowledged) {
      toast.danger(t("marketAccess.publicConfirmRequired"));
      return;
    }
    setBusy(true);
    try {
      onSaved(await updateMarketPublicCredit(currency, {
        enabled,
        limitMinor: limitMinor ?? undefined,
        riskAcknowledged: enabled && acknowledged,
        expectedRevision: line?.revision || 0,
      }));
      toast.success(t("marketAccess.publicSaved", { currency }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-2 sm:grid-cols-[4rem_7rem_minmax(8rem,1fr)_minmax(10rem,1fr)_auto] sm:items-end">
      <strong className="text-sm">{currency}</strong>
      <Checkbox isSelected={enabled} onChange={setEnabled} isDisabled={busy} className="pb-2 text-xs">
        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
        {t("common.enabled")}
      </Checkbox>
      <label className="grid gap-1 text-xs text-muted-foreground">
        {t("marketAccess.limitMajor")}
        <input
          value={limit}
          disabled={!enabled || busy}
          inputMode="decimal"
          onChange={(event) => setLimit(event.target.value)}
          className="h-9 min-w-0 rounded-md border border-border bg-white px-3 text-sm text-foreground disabled:bg-slate-50"
        />
      </label>
      <Checkbox isSelected={acknowledged} onChange={setAcknowledged} isDisabled={!enabled || busy} className="pb-2 text-xs">
        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
        {t("marketAccess.publicConfirm")}
      </Checkbox>
      <Button size="sm" variant="outline" isDisabled={busy} onClick={() => void save()}>
        {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Check className="h-4 w-4" />}
        {t("common.save")}
      </Button>
    </div>
  );
}

export function AccountMarketAccessPage() {
  const { locale, t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;
  const [dashboard, setDashboard] = React.useState<MarketAccessDashboard | null>(null);
  const [counterpartyDrafts, setCounterpartyDrafts] = React.useState<
    Record<string, CounterpartyDraft>
  >({});
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [busy, setBusy] = React.useState("");
  const [email, setEmail] = React.useState("");
  const [buyerQuery, setBuyerQuery] = React.useState("");
  const [allowedScopes, setAllowedScopes] = React.useState<Record<string, boolean>>({
    "share:free": true,
    "share:paid": true,
    "client_host:free": true,
    "client_host:paid": true,
  });
  const [blacklistPolicy, setBlacklistPolicy] = React.useState<MarketAccessPolicy | null>(null);
  const [riskAcknowledged, setRiskAcknowledged] = React.useState(false);

  const load = React.useCallback(async (silent = false) => {
    if (!authed) {
      setLoading(false);
      return;
    }
    if (silent) setRefreshing(true);
    else setLoading(true);
    try {
      const nextDashboard = await getMarketAccessDashboard();
      setDashboard(nextDashboard);
      setCounterpartyDrafts(buildCounterpartyDrafts(nextDashboard.counterparties));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, [authed]);

  React.useEffect(() => {
    void load();
  }, [load]);

  const policyFor = (kind: MarketAccessProductKind, pricingKind: MarketAccessPricingKind) =>
    dashboard?.policies.find(
      (item) => item.productKind === kind && item.pricingKind === pricingKind,
    );
  const blackMode = dashboard?.policies.some((policy) => policy.mode === "blacklist") || false;
  const paidBlackMode =
    dashboard?.policies.some(
      (policy) => policy.pricingKind === "paid" && policy.mode === "blacklist",
    ) || false;
  const counterpartyChanges = React.useMemo(
    () =>
      (dashboard?.counterparties || []).map((counterparty) => ({
        counterparty,
        draft: counterpartyDrafts[counterparty.id],
        change: counterpartyChange(counterparty, counterpartyDrafts[counterparty.id]),
      })),
    [counterpartyDrafts, dashboard?.counterparties],
  );
  const changedCounterparties = React.useMemo(
    () => counterpartyChanges.filter(({ change }) => hasCounterpartyChange(change)),
    [counterpartyChanges],
  );
  const normalizedBuyerQuery = buyerQuery.trim().toLowerCase();
  const visibleCounterparties = React.useMemo(() => {
    if (!normalizedBuyerQuery) return dashboard?.counterparties || [];
    return (dashboard?.counterparties || []).filter((counterparty) =>
      [counterparty.buyerEmail, counterparty.buyerUserId || "", counterparty.status]
        .join(" ")
        .toLowerCase()
        .includes(normalizedBuyerQuery),
    );
  }, [dashboard?.counterparties, normalizedBuyerQuery]);

  const updateCounterpartyDraft = React.useCallback(
    (id: string, update: (draft: CounterpartyDraft) => CounterpartyDraft) => {
      setCounterpartyDrafts((current) => {
        const draft = current[id];
        if (!draft) return current;
        return { ...current, [id]: update(draft) };
      });
    },
    [],
  );

  const updateDashboardPreservingDrafts = React.useCallback(
    (nextDashboard: MarketAccessDashboard) => {
      setDashboard(nextDashboard);
      setCounterpartyDrafts((current) =>
        Object.fromEntries(
          nextDashboard.counterparties.map((counterparty) => [
            counterparty.id,
            current[counterparty.id] || buildCounterpartyDraft(counterparty),
          ]),
        ) as Record<string, CounterpartyDraft>,
      );
    },
    [],
  );

  const applyPolicy = async (policy: MarketAccessPolicy, mode: "whitelist" | "blacklist", acknowledged = false) => {
    setBusy(`policy:${policy.productKind}:${policy.pricingKind}`);
    try {
      updateDashboardPreservingDrafts(
        await updateMarketAccessPolicy(policy.productKind, policy.pricingKind, {
          mode,
          riskAcknowledged: acknowledged,
          expectedRevision: policy.revision,
        }),
      );
      toast.success(t("marketAccess.policySaved"));
      return true;
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
      return false;
    } finally {
      setBusy("");
    }
  };

  const changePolicy = (policy: MarketAccessPolicy, mode: "whitelist" | "blacklist") => {
    if (policy.mode === mode) return;
    if (mode === "blacklist") {
      setRiskAcknowledged(false);
      setBlacklistPolicy(policy);
      return;
    }
    void applyPolicy(policy, mode);
  };

  const addCounterparty = async () => {
    const normalizedEmail = email.trim().toLowerCase();
    if (!normalizedEmail.includes("@")) {
      toast.danger(t("marketAccess.invalidEmail"));
      return;
    }
    if (!Object.values(allowedScopes).some(Boolean)) {
      toast.danger(t("marketAccess.productRequired"));
      return;
    }
    setBusy("add");
    try {
      const savedCounterparty = await upsertMarketCounterparty({
        email: normalizedEmail,
        accessRules: (["share", "client_host"] as const).flatMap((productKind) =>
          (["free", "paid"] as const)
            .filter((pricingKind) => allowedScopes[`${productKind}:${pricingKind}`])
            .map((pricingKind) => ({ productKind, pricingKind, decision: "allow" as const })),
        ),
      });
      setEmail("");
      setDashboard((current) =>
        current
          ? {
              ...current,
              counterparties: [
                ...current.counterparties.filter((item) => item.id !== savedCounterparty.id),
                savedCounterparty,
              ].sort((left, right) => left.buyerEmail.localeCompare(right.buyerEmail)),
            }
          : current,
      );
      setCounterpartyDrafts((current) => ({
        ...current,
        [savedCounterparty.id]: buildCounterpartyDraft(savedCounterparty),
      }));
      toast.success(t("marketAccess.counterpartyAdded", { email: normalizedEmail }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  const resetCounterpartyChanges = () => {
    setCounterpartyDrafts(buildCounterpartyDrafts(dashboard?.counterparties || []));
  };

  const saveCounterpartyChanges = async () => {
    if (!dashboard || changedCounterparties.length === 0) return;
    for (const { counterparty, draft, change } of changedCounterparties) {
      if (!draft) continue;
      for (const currency of change.creditCurrencies) {
        const credit = draft.credits[currency];
        if (credit.kind === "limited" && parseLimitMinor(credit.limit) == null) {
          toast.danger(
            t("marketAccess.buyerCreditError", {
              email: counterparty.buyerEmail,
              currency,
              message: t("marketAccess.invalidLimit"),
            }),
          );
          return;
        }
        if (credit.kind === "unlimited" && !credit.unlimitedAcknowledged) {
          toast.danger(
            t("marketAccess.buyerCreditError", {
              email: counterparty.buyerEmail,
              currency,
              message: t("marketAccess.unlimitedConfirmRequired"),
            }),
          );
          return;
        }
      }
    }

    setBusy("save-counterparties");
    try {
      for (const { counterparty, draft, change } of changedCounterparties) {
        if (!draft) continue;
        const saveRelationship = async () => {
          if (!change.statusChanged && change.accessRules.length === 0) return;
          await updateMarketCounterparty(counterparty.id, {
            accessRules: change.accessRules,
            status: draft.status,
            expectedRevision: counterparty.revision,
          });
        };
        const saveCredits = async () => {
          for (const currency of change.creditCurrencies) {
            const credit = draft.credits[currency];
            const line = counterpartyCreditLine(counterparty, currency);
            await updateMarketCounterpartyCredit(counterparty.id, currency, {
              kind: credit.kind,
              limitMinor:
                credit.kind === "limited" ? parseLimitMinor(credit.limit) ?? undefined : undefined,
              riskAcknowledged:
                credit.kind === "unlimited" && credit.unlimitedAcknowledged,
              expectedRevision: line?.revision || 0,
            });
          }
        };
        if (change.statusChanged && draft.status === "revoked") {
          await saveCredits();
          await saveRelationship();
        } else {
          await saveRelationship();
          await saveCredits();
        }
      }
      await load(true);
      toast.success(
        t("marketAccess.buyersSaved", { count: changedCounterparties.length }),
      );
    } catch (error) {
      await load(true);
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  if (authLoading || loading) {
    return <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  }
  if (!authed) return <p className="py-8 text-sm text-muted-foreground">{t("account.signInRequired")}</p>;

  return (
    <div className="grid min-w-0 gap-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex items-center gap-2">
          <ShieldCheck className="h-5 w-5 text-muted-foreground" />
          <h2 className="text-lg font-semibold">{t("marketAccess.title")}</h2>
        </div>
        <Button
          isIconOnly
          variant="outline"
          aria-label={t("common.reload")}
          isDisabled={refreshing || !!busy || changedCounterparties.length > 0}
          onClick={() => void load(true)}
        >
          <RefreshCw className={cn("h-4 w-4", refreshing && "animate-spin")} />
        </Button>
      </div>

      {blackMode ? (
        <section className="flex gap-3 border-y border-amber-300 bg-amber-50 px-4 py-3 text-amber-950">
          <AlertTriangle className="mt-0.5 h-5 w-5 shrink-0" />
          <div>
            <strong className="text-sm">{t("marketAccess.blacklistBannerTitle")}</strong>
            <p className="mt-1 text-sm leading-6">{t("marketAccess.blacklistBanner")}</p>
          </div>
        </section>
      ) : null}

      <section className="grid gap-4">
        <h3 className="text-sm font-semibold">{t("marketAccess.policyTitle")}</h3>
        <div className="grid gap-3 sm:grid-cols-2">
          {(["share", "client_host"] as const).flatMap((kind) =>
            (["free", "paid"] as const).map((pricingKind) => {
              const policy = policyFor(kind, pricingKind) || {
                productKind: kind,
                pricingKind,
                mode: pricingKind === "free" ? ("blacklist" as const) : ("whitelist" as const),
                revision: 0,
                updatedAt: "",
              };
              const productLabel =
                kind === "share"
                  ? t("marketAccess.product.share")
                  : t("marketAccess.product.clientHost");
              const pricingLabel =
                pricingKind === "free"
                  ? t("marketAccess.pricing.free")
                  : t("marketAccess.pricing.paid");
              const title = `${productLabel} · ${pricingLabel}`;
              const blacklisted = policy.mode === "blacklist";
              return (
                <div
                  key={`${kind}:${pricingKind}`}
                  className={cn(
                    "grid gap-3 rounded-lg border bg-card p-3",
                    blacklisted ? "border-amber-300 bg-amber-50/40" : "border-border",
                  )}
                >
                  <strong className="text-sm text-foreground">{title}</strong>
                  <SegmentedControl
                    value={policy.mode}
                    disabled={!!busy}
                    ariaLabel={title}
                    fullWidth
                    items={[
                      { id: "whitelist", label: t("marketAccess.mode.whitelist") },
                      { id: "blacklist", label: t("marketAccess.mode.blacklist") },
                    ]}
                    onChange={(mode) => changePolicy(policy, mode)}
                  />
                </div>
              );
            }),
          )}
        </div>
      </section>

      {paidBlackMode ? (
        <section className="grid gap-4 border-b border-border pb-5">
          <div>
            <h3 className="text-sm font-semibold">{t("marketAccess.publicCreditTitle")}</h3>
            <p className="mt-1 text-xs text-muted-foreground">{t("marketAccess.publicCreditHint")}</p>
          </div>
          {(["CNY", "USD"] as const).map((currency) => (
            <PublicCreditEditor
              key={currency}
              currency={currency}
              line={dashboard?.publicCreditLines.find((line) => line.currency === currency)}
              onSaved={updateDashboardPreservingDrafts}
            />
          ))}
        </section>
      ) : null}

      <section className="grid gap-4">
        <div>
          <h3 className="text-sm font-semibold">{t("marketAccess.addTitle")}</h3>
          <div className="mt-3 grid gap-3 sm:grid-cols-[minmax(12rem,1fr)_minmax(16rem,2fr)_auto] sm:items-end">
            <label className="grid gap-1 text-xs text-muted-foreground">
              {t("marketAccess.email")}
              <input
                type="email"
                value={email}
                onChange={(event) => setEmail(event.target.value)}
                className="h-10 min-w-0 rounded-md border border-border bg-white px-3 text-sm text-foreground"
              />
            </label>
            <div className="grid grid-cols-2 gap-x-4 gap-y-2 pb-1">
              {(["share", "client_host"] as const).flatMap((productKind) =>
                (["free", "paid"] as const).map((pricingKind) => {
                  const key = `${productKind}:${pricingKind}`;
                  return (
                    <Checkbox
                      key={key}
                      isSelected={allowedScopes[key] !== false}
                      onChange={(selected) =>
                        setAllowedScopes((current) => ({ ...current, [key]: selected }))
                      }
                      className="text-sm"
                    >
                      <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                      {productKind === "share"
                        ? t("marketAccess.product.share")
                        : t("marketAccess.product.clientHost")}
                      {" · "}
                      {pricingKind === "free"
                        ? t("marketAccess.pricing.free")
                        : t("marketAccess.pricing.paid")}
                    </Checkbox>
                  );
                }),
              )}
            </div>
            <Button variant="primary" isDisabled={!!busy} onClick={() => void addCounterparty()}>
              {busy === "add" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
              {t("common.add")}
            </Button>
          </div>
        </div>

        <div className="grid gap-3 border-t border-border pt-4">
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
            <h3 className="text-sm font-semibold">{t("marketAccess.buyersTitle")}</h3>
            <div className="flex w-full min-w-0 flex-wrap items-center gap-2 sm:w-auto">
              <label className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md border border-border bg-white px-3 text-sm focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/10 sm:min-w-64">
                <Search className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
                <input
                  type="search"
                  value={buyerQuery}
                  onChange={(event) => setBuyerQuery(event.target.value)}
                  className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
                  placeholder={t("marketAccess.searchBuyers")}
                  aria-label={t("marketAccess.searchBuyers")}
                />
                {buyerQuery ? (
                  <button
                    type="button"
                    className="rounded p-0.5 text-muted-foreground hover:bg-slate-100 hover:text-foreground"
                    aria-label={t("common.close")}
                    onClick={() => setBuyerQuery("")}
                  >
                    <X className="h-3.5 w-3.5" />
                  </button>
                ) : null}
              </label>
              <Button
                size="sm"
                variant="outline"
                className="h-9"
                isDisabled={changedCounterparties.length === 0 || !!busy}
                onClick={resetCounterpartyChanges}
              >
                <RotateCcw className="h-4 w-4" />
                {t("common.reset")}
              </Button>
              <Button
                size="sm"
                variant="primary"
                className="h-9"
                isDisabled={changedCounterparties.length === 0 || !!busy}
                onClick={() => void saveCounterpartyChanges()}
              >
                {busy === "save-counterparties" ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Save className="h-4 w-4" />
                )}
                {t("common.save")}
              </Button>
            </div>
          </div>

          <div className="overflow-hidden rounded-lg border border-border bg-white">
            <div className="overflow-x-auto">
              <table className="w-full min-w-[1460px] table-fixed border-collapse text-sm">
                <colgroup>
                  <col className="w-[230px]" />
                  <col className="w-[120px]" />
                  <col className="w-[230px]" />
                  <col className="w-[230px]" />
                  <col className="w-[230px]" />
                  <col className="w-[230px]" />
                  <col className="w-[190px]" />
                </colgroup>
                <thead className="bg-slate-50 text-left text-[11px] font-semibold uppercase text-muted-foreground">
                  <tr>
                    <th className="px-3 py-2.5">{t("marketAccess.table.buyer")}</th>
                    <th className="px-3 py-2.5">{t("marketAccess.table.status")}</th>
                    <th className="px-3 py-2.5">{t("marketAccess.product.share")}</th>
                    <th className="px-3 py-2.5">{t("marketAccess.product.clientHost")}</th>
                    <th className="px-3 py-2.5">CNY {t("marketAccess.creditType")}</th>
                    <th className="px-3 py-2.5">USD {t("marketAccess.creditType")}</th>
                    <th className="px-3 py-2.5">{t("marketAccess.table.exposure")}</th>
                  </tr>
                </thead>
                <tbody>
                  {visibleCounterparties.map((counterparty) => {
                    const draft =
                      counterpartyDrafts[counterparty.id] || buildCounterpartyDraft(counterparty);
                    const change = counterpartyChange(counterparty, draft);
                    const rowChanged = hasCounterpartyChange(change);
                    const editorDisabled = draft.status === "revoked" || !!busy;
                    return (
                      <tr
                        key={counterparty.id}
                        className={cn(
                          "border-t border-border align-top",
                          rowChanged && "bg-primary/[0.025] shadow-[inset_3px_0_0_var(--primary)]",
                        )}
                      >
                        <td className="px-3 py-3">
                          <strong className="block break-all text-sm font-medium text-foreground">
                            {counterparty.buyerEmail}
                          </strong>
                          {counterparty.buyerUserId ? (
                            <span className="mt-1 block truncate font-mono text-[10px] text-muted-foreground" title={counterparty.buyerUserId}>
                              {counterparty.buyerUserId}
                            </span>
                          ) : null}
                          {rowChanged ? (
                            <Chip size="sm" variant="soft" className="mt-2 bg-blue-50 text-blue-700">
                              {t("marketAccess.unsaved")}
                            </Chip>
                          ) : null}
                        </td>
                        <td className="px-3 py-3">
                          <div className="flex items-center gap-2">
                            <Checkbox
                              aria-label={t("marketAccess.status.active")}
                              isSelected={draft.status === "active"}
                              isDisabled={!!busy}
                              onChange={(selected) =>
                                updateCounterpartyDraft(counterparty.id, (current) => ({
                                  ...current,
                                  status: selected ? "active" : "revoked",
                                }))
                              }
                            >
                              <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                            </Checkbox>
                            <Chip
                              size="sm"
                              variant="soft"
                              className={
                                draft.status === "active"
                                  ? "bg-emerald-100 text-emerald-700"
                                  : "bg-slate-100 text-slate-600"
                              }
                            >
                              {draft.status === "active"
                                ? t("marketAccess.status.active")
                                : t("marketAccess.status.revoked")}
                            </Chip>
                          </div>
                        </td>
                        {(["share", "client_host"] as const).map((productKind) => (
                          <td key={productKind} className="px-3 py-3">
                            <div className="grid gap-2">
                              {(["free", "paid"] as const).map((pricingKind) => {
                                const key = scopeKey(productKind, pricingKind);
                                const label =
                                  pricingKind === "free"
                                    ? t("marketAccess.pricing.free")
                                    : t("marketAccess.pricing.paid");
                                return (
                                  <label key={pricingKind} className="grid grid-cols-[3rem_minmax(0,1fr)] items-center gap-2 text-xs text-muted-foreground">
                                    <span>{label}</span>
                                    <select
                                      value={draft.decisions[key]}
                                      disabled={editorDisabled}
                                      aria-label={`${productKind === "share" ? t("marketAccess.product.share") : t("marketAccess.product.clientHost")} · ${label}`}
                                      onChange={(event) =>
                                        updateCounterpartyDraft(counterparty.id, (current) => ({
                                          ...current,
                                          decisions: {
                                            ...current.decisions,
                                            [key]: event.target.value as MarketAccessDecision,
                                          },
                                        }))
                                      }
                                      className="h-9 min-w-0 rounded-md border border-border bg-white px-2 text-xs text-foreground disabled:bg-slate-50"
                                    >
                                      <option value="inherit">{t("marketAccess.decision.inherit")}</option>
                                      <option value="allow">{t("marketAccess.decision.allow")}</option>
                                      <option value="deny">{t("marketAccess.decision.deny")}</option>
                                    </select>
                                  </label>
                                );
                              })}
                            </div>
                          </td>
                        ))}
                        {CURRENCIES.map((currency) => (
                          <td key={currency} className="px-3 py-3">
                            <CounterpartyCreditCell
                              currency={currency}
                              line={counterpartyCreditLine(counterparty, currency)}
                              draft={draft.credits[currency]}
                              disabled={editorDisabled}
                              onChange={(credit) =>
                                updateCounterpartyDraft(counterparty.id, (current) => ({
                                  ...current,
                                  credits: { ...current.credits, [currency]: credit },
                                }))
                              }
                            />
                          </td>
                        ))}
                        <td className="px-3 py-3">
                          {counterparty.exposures.length ? (
                            <div className="grid gap-2">
                              {counterparty.exposures.map((exposure) => (
                                <div key={exposure.currency}>
                                  <strong className="block text-xs tabular-nums">
                                    {formatMoney(exposure.balanceMinor, exposure.currency, locale)}
                                  </strong>
                                  <span className="text-[11px] text-muted-foreground">
                                    {t("marketAccess.activeServices", {
                                      count: exposure.activeServiceCount,
                                    })}
                                  </span>
                                </div>
                              ))}
                            </div>
                          ) : (
                            <span className="text-muted-foreground">-</span>
                          )}
                        </td>
                      </tr>
                    );
                  })}
                  {visibleCounterparties.length === 0 ? (
                    <tr>
                      <td colSpan={7} className="px-4 py-12 text-center text-sm text-muted-foreground">
                        {dashboard?.counterparties.length
                          ? t("marketAccess.noMatches")
                          : t("marketAccess.empty")}
                      </td>
                    </tr>
                  ) : null}
                </tbody>
              </table>
            </div>
          </div>
        </div>
      </section>

      <Modal.Backdrop isOpen={!!blacklistPolicy} onOpenChange={(open) => !open && !busy && setBlacklistPolicy(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(520px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("marketAccess.blacklistConfirmTitle")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid gap-4">
              <div className="flex gap-3 rounded-md border border-amber-300 bg-amber-50 p-3 text-sm leading-6 text-amber-950">
                <AlertTriangle className="mt-1 h-4 w-4 shrink-0" />
                <span>{t("marketAccess.blacklistConfirmDescription")}</span>
              </div>
              <Checkbox isSelected={riskAcknowledged} onChange={setRiskAcknowledged}>
                <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                {t("marketAccess.blacklistConfirmCheckbox")}
              </Checkbox>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="outline" isDisabled={!!busy} onClick={() => setBlacklistPolicy(null)}>{t("common.cancel")}</Button>
              <Button
                variant="danger"
                isDisabled={!riskAcknowledged || !!busy}
                onClick={() => {
                  if (!blacklistPolicy) return;
                  void applyPolicy(blacklistPolicy, "blacklist", true).then((saved) => {
                    if (saved) setBlacklistPolicy(null);
                  });
                }}
              >
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <AlertTriangle className="h-4 w-4" />}
                {t("marketAccess.enableBlacklist")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </div>
  );
}
