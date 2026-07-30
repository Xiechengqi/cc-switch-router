"use client";

import * as React from "react";
import { Button, Checkbox, Chip, Modal, toast } from "@heroui/react";
import {
  AlertTriangle,
  Check,
  Loader2,
  Plus,
  RefreshCw,
  ShieldCheck,
  UserRoundCheck,
  UserRoundX,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
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
  MarketAccessProductKind,
  MarketCounterparty,
  MarketCreditKind,
  MarketCreditLine,
  MarketPublicCreditLine,
} from "@/lib/types";
import { cn } from "@/lib/utils";

type Currency = "CNY" | "USD";

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

function SegmentedControl<T extends string>({
  value,
  options,
  disabled,
  onChange,
}: {
  value: T;
  options: Array<{ value: T; label: string }>;
  disabled?: boolean;
  onChange: (value: T) => void;
}) {
  return (
    <div className="inline-grid min-h-9 grid-flow-col overflow-hidden rounded-md border border-border bg-slate-50 p-0.5">
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          disabled={disabled}
          onClick={() => onChange(option.value)}
          className={cn(
            "min-w-20 rounded px-2 py-1 text-xs font-medium transition-colors disabled:opacity-50",
            value === option.value
              ? "bg-white text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

function CreditEditor({
  counterpartyId,
  currency,
  line,
  disabled,
  onSaved,
}: {
  counterpartyId: string;
  currency: Currency;
  line?: MarketCreditLine;
  disabled?: boolean;
  onSaved: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const [kind, setKind] = React.useState<MarketCreditKind>(line?.kind || "none");
  const [limit, setLimit] = React.useState(line?.limitMinor ? (line.limitMinor / 100).toFixed(2) : "");
  const [unlimitedAcknowledged, setUnlimitedAcknowledged] = React.useState(false);
  const [busy, setBusy] = React.useState(false);

  React.useEffect(() => {
    setKind(line?.kind || "none");
    setLimit(line?.limitMinor ? (line.limitMinor / 100).toFixed(2) : "");
    setUnlimitedAcknowledged(false);
  }, [line?.kind, line?.limitMinor, line?.revision]);

  const save = async () => {
    const limitMinor = kind === "limited" ? parseLimitMinor(limit) : undefined;
    if (kind === "limited" && limitMinor == null) {
      toast.danger(t("marketAccess.invalidLimit"));
      return;
    }
    if (kind === "unlimited" && !unlimitedAcknowledged) {
      toast.danger(t("marketAccess.unlimitedConfirmRequired"));
      return;
    }
    setBusy(true);
    try {
      await updateMarketCounterpartyCredit(counterpartyId, currency, {
        kind,
        limitMinor: limitMinor ?? undefined,
        riskAcknowledged: kind === "unlimited" && unlimitedAcknowledged,
        expectedRevision: line?.revision || 0,
      });
      await onSaved();
      toast.success(t("marketAccess.creditSaved", { currency }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-2 border-t border-border pt-3 first:border-t-0 first:pt-0 sm:grid-cols-[4rem_8rem_minmax(7rem,1fr)_auto] sm:items-end">
      <strong className="text-sm">{currency}</strong>
      <label className="grid gap-1 text-xs text-muted-foreground">
        {t("marketAccess.creditType")}
        <select
          value={kind}
          disabled={disabled || busy}
          onChange={(event) => setKind(event.target.value as MarketCreditKind)}
          className="h-9 rounded-md border border-border bg-white px-2 text-sm text-foreground"
        >
          <option value="none">{t("marketAccess.credit.none")}</option>
          <option value="limited">{t("marketAccess.credit.limited")}</option>
          <option value="unlimited">{t("marketAccess.credit.unlimited")}</option>
        </select>
      </label>
      {kind === "limited" ? (
        <label className="grid gap-1 text-xs text-muted-foreground">
          {t("marketAccess.limitMajor")}
          <input
            value={limit}
            disabled={disabled || busy}
            inputMode="decimal"
            onChange={(event) => setLimit(event.target.value)}
            className="h-9 min-w-0 rounded-md border border-border bg-white px-3 text-sm text-foreground"
          />
        </label>
      ) : kind === "unlimited" ? (
        <Checkbox
          isSelected={unlimitedAcknowledged}
          onChange={setUnlimitedAcknowledged}
          isDisabled={disabled || busy}
          className="self-center text-xs"
        >
          <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
          {t("marketAccess.unlimitedConfirm")}
        </Checkbox>
      ) : <div />}
      <Button size="sm" variant="outline" isDisabled={disabled || busy} onClick={() => void save()}>
        {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Check className="h-4 w-4" />}
        {t("common.save")}
      </Button>
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
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [busy, setBusy] = React.useState("");
  const [email, setEmail] = React.useState("");
  const [allowShare, setAllowShare] = React.useState(true);
  const [allowClient, setAllowClient] = React.useState(true);
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
      setDashboard(await getMarketAccessDashboard());
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

  const policyFor = (kind: MarketAccessProductKind) => dashboard?.policies.find((item) => item.productKind === kind);
  const blackMode = dashboard?.policies.some((policy) => policy.mode === "blacklist") || false;

  const applyPolicy = async (policy: MarketAccessPolicy, mode: "whitelist" | "blacklist", acknowledged = false) => {
    setBusy(`policy:${policy.productKind}`);
    try {
      setDashboard(await updateMarketAccessPolicy(policy.productKind, {
        mode,
        riskAcknowledged: acknowledged,
        expectedRevision: policy.revision,
      }));
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
    if (!allowShare && !allowClient) {
      toast.danger(t("marketAccess.productRequired"));
      return;
    }
    setBusy("add");
    try {
      await upsertMarketCounterparty({
        email: normalizedEmail,
        accessRules: [
          ...(allowShare ? [{ productKind: "share" as const, decision: "allow" as const }] : []),
          ...(allowClient ? [{ productKind: "client_host" as const, decision: "allow" as const }] : []),
        ],
      });
      setEmail("");
      await load(true);
      toast.success(t("marketAccess.counterpartyAdded", { email: normalizedEmail }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  const updateDecision = async (
    counterparty: MarketCounterparty,
    productKind: MarketAccessProductKind,
    decision: MarketAccessDecision,
  ) => {
    setBusy(`access:${counterparty.id}:${productKind}`);
    try {
      await updateMarketCounterparty(counterparty.id, {
        accessRules: [{ productKind, decision }],
        status: counterparty.status === "revoked" ? "revoked" : "active",
        expectedRevision: counterparty.revision,
      });
      await load(true);
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  const toggleCounterparty = async (counterparty: MarketCounterparty) => {
    setBusy(`status:${counterparty.id}`);
    try {
      await updateMarketCounterparty(counterparty.id, {
        accessRules: [],
        status: counterparty.status === "active" ? "revoked" : "active",
        expectedRevision: counterparty.revision,
      });
      await load(true);
    } catch (error) {
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
        <Button isIconOnly variant="outline" aria-label={t("common.reload")} isDisabled={refreshing} onClick={() => void load(true)}>
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

      <section className="grid gap-4 border-y border-border py-5">
        <h3 className="text-sm font-semibold">{t("marketAccess.policyTitle")}</h3>
        <div className="grid gap-4 sm:grid-cols-2">
          {(["share", "client_host"] as const).map((kind) => {
            const policy = policyFor(kind) || { productKind: kind, mode: "whitelist", revision: 0, updatedAt: "" };
            return (
              <div key={kind} className="flex flex-wrap items-center justify-between gap-3 border-b border-border pb-4 sm:border-b-0 sm:pb-0">
                <strong className="text-sm">{kind === "share" ? t("marketAccess.product.share") : t("marketAccess.product.clientHost")}</strong>
                <SegmentedControl
                  value={policy.mode}
                  disabled={!!busy}
                  options={[
                    { value: "whitelist", label: t("marketAccess.mode.whitelist") },
                    { value: "blacklist", label: t("marketAccess.mode.blacklist") },
                  ]}
                  onChange={(mode) => changePolicy(policy, mode)}
                />
              </div>
            );
          })}
        </div>
      </section>

      {blackMode ? (
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
              onSaved={setDashboard}
            />
          ))}
        </section>
      ) : null}

      <section className="grid gap-4">
        <div>
          <h3 className="text-sm font-semibold">{t("marketAccess.addTitle")}</h3>
          <div className="mt-3 grid gap-3 sm:grid-cols-[minmax(12rem,1fr)_auto_auto_auto] sm:items-end">
            <label className="grid gap-1 text-xs text-muted-foreground">
              {t("marketAccess.email")}
              <input
                type="email"
                value={email}
                onChange={(event) => setEmail(event.target.value)}
                className="h-10 min-w-0 rounded-md border border-border bg-white px-3 text-sm text-foreground"
              />
            </label>
            <Checkbox isSelected={allowShare} onChange={setAllowShare} className="pb-2 text-sm">
              <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
              {t("marketAccess.product.share")}
            </Checkbox>
            <Checkbox isSelected={allowClient} onChange={setAllowClient} className="pb-2 text-sm">
              <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
              {t("marketAccess.product.clientHost")}
            </Checkbox>
            <Button variant="primary" isDisabled={busy === "add"} onClick={() => void addCounterparty()}>
              {busy === "add" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
              {t("common.add")}
            </Button>
          </div>
        </div>

        <div className="grid gap-3">
          {(dashboard?.counterparties || []).map((counterparty) => {
            const decisionFor = (kind: MarketAccessProductKind) =>
              counterparty.accessRules.find((rule) => rule.productKind === kind)?.decision || "inherit";
            return (
              <article key={counterparty.id} className="overflow-hidden rounded-lg border border-border bg-card">
                <div className="flex flex-wrap items-start justify-between gap-3 border-b border-border px-4 py-3">
                  <div className="min-w-0">
                    <div className="flex flex-wrap items-center gap-2">
                      <strong className="break-all text-sm">{counterparty.buyerEmail}</strong>
                      <Chip size="sm" variant="soft" className={counterparty.status === "active" ? "bg-emerald-100 text-emerald-700" : "bg-slate-100 text-slate-600"}>
                        {counterparty.status === "active" ? t("marketAccess.status.active") : t("marketAccess.status.revoked")}
                      </Chip>
                    </div>
                    {counterparty.exposures.length ? (
                      <p className="mt-1 text-xs text-muted-foreground">
                        {counterparty.exposures.map((exposure) => `${exposure.currency} ${formatMoney(exposure.balanceMinor, exposure.currency, locale)} · ${exposure.activeServiceCount}`).join(" | ")}
                      </p>
                    ) : null}
                  </div>
                  <Button
                    size="sm"
                    variant="ghost"
                    isDisabled={!!busy}
                    onClick={() => void toggleCounterparty(counterparty)}
                  >
                    {counterparty.status === "active" ? <UserRoundX className="h-4 w-4" /> : <UserRoundCheck className="h-4 w-4" />}
                    {counterparty.status === "active" ? t("marketAccess.revoke") : t("marketAccess.reactivate")}
                  </Button>
                </div>
                <div className="grid gap-4 px-4 py-4">
                  <div className="grid gap-3 sm:grid-cols-2">
                    {(["share", "client_host"] as const).map((kind) => (
                      <div key={kind} className="flex flex-wrap items-center justify-between gap-2">
                        <span className="text-sm">{kind === "share" ? t("marketAccess.product.share") : t("marketAccess.product.clientHost")}</span>
                        <SegmentedControl
                          value={decisionFor(kind)}
                          disabled={counterparty.status !== "active" || !!busy}
                          options={[
                            { value: "inherit", label: t("marketAccess.decision.inherit") },
                            { value: "allow", label: t("marketAccess.decision.allow") },
                            { value: "deny", label: t("marketAccess.decision.deny") },
                          ]}
                          onChange={(decision) => void updateDecision(counterparty, kind, decision)}
                        />
                      </div>
                    ))}
                  </div>
                  <div className="grid gap-3">
                    {(["CNY", "USD"] as const).map((currency) => (
                      <CreditEditor
                        key={currency}
                        counterpartyId={counterparty.id}
                        currency={currency}
                        line={counterparty.creditLines.find((line) => line.currency === currency)}
                        disabled={counterparty.status !== "active"}
                        onSaved={() => load(true)}
                      />
                    ))}
                  </div>
                </div>
              </article>
            );
          })}
          {dashboard?.counterparties.length === 0 ? (
            <p className="border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">{t("marketAccess.empty")}</p>
          ) : null}
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
