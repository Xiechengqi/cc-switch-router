"use client";

import * as React from "react";
import { Button, Checkbox, Chip, Modal, toast } from "@heroui/react";
import {
  AlertTriangle,
  Check,
  ChevronDown,
  Clock3,
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
  approveMarketAccessRequest,
  getMarketAccessDashboard,
  getMarketBillingConfig,
  rejectMarketAccessRequest,
  updateMarketAccessPolicy,
  updateMarketCounterpartiesBatch,
  updateMarketPublicCredit,
  upsertMarketCounterparty,
} from "@/lib/api";
import type {
  MarketAccessDashboard,
  MarketAccessRequest,
  MarketAccessDecision,
  MarketAccessPolicy,
  MarketAccessPricingKind,
  MarketAccessProductKind,
  MarketCounterparty,
  MarketCreditKind,
  MarketCreditLine,
  MarketPublicCreditLine,
} from "@/lib/types";
import {
  DEFAULT_USD_CNY_RATE_MICROS,
  formatUsdCnyMoney,
  MARKET_CURRENCY,
  usdMinorToCnyMinor,
} from "@/lib/market-money";
import { cn } from "@/lib/utils";

type Currency = typeof MARKET_CURRENCY;
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

type AccessRequestGroup = {
  key: string;
  buyerEmail: string;
  buyerUserId: string;
  requests: MarketAccessRequest[];
};

type RequestAction = {
  kind: "approve" | "reject";
  request: MarketAccessRequest;
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

const CURRENCIES: Currency[] = [MARKET_CURRENCY];

function parseLimitMinor(value: string) {
  if (!/^\d+(?:\.\d{1,2})?$/.test(value.trim())) return null;
  const minor = Math.round(Number(value) * 100);
  return Number.isSafeInteger(minor) && minor >= 1 && minor <= 100_000_000 ? minor : null;
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
      USD: creditDraft(counterpartyCreditLine(counterparty, "USD")),
    },
  };
}

function buildCounterpartyDrafts(counterparties: MarketCounterparty[]) {
  return Object.fromEntries(
    counterparties.map((counterparty) => [counterparty.id, buildCounterpartyDraft(counterparty)]),
  ) as Record<string, CounterpartyDraft>;
}

function sameBuyer(
  leftUserId: string | undefined,
  leftEmail: string,
  rightUserId: string | undefined,
  rightEmail: string,
) {
  return (!!leftUserId && !!rightUserId && leftUserId === rightUserId)
    || leftEmail.trim().toLowerCase() === rightEmail.trim().toLowerCase();
}

function groupAccessRequests(requests: MarketAccessRequest[]): AccessRequestGroup[] {
  const groups = new Map<string, AccessRequestGroup>();
  for (const request of requests) {
    const key = request.buyerUserId || request.buyerEmail.trim().toLowerCase();
    const group = groups.get(key) || {
      key,
      buyerEmail: request.buyerEmail,
      buyerUserId: request.buyerUserId,
      requests: [],
    };
    group.requests.push(request);
    groups.set(key, group);
  }
  return Array.from(groups.values()).sort((left, right) => {
    const leftTime = left.requests[0]?.requestedAt || "";
    const rightTime = right.requests[0]?.requestedAt || "";
    return rightTime.localeCompare(leftTime) || left.buyerEmail.localeCompare(right.buyerEmail);
  });
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

function formatRequestTime(value: string, locale: string) {
  const date = new Date(value);
  return Number.isFinite(date.getTime())
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date)
    : value;
}

function CounterpartyCreditCell({
  line,
  currency,
  draft,
  disabled,
  allowNone = true,
  onChange,
}: {
  line?: MarketCreditLine;
  currency: Currency;
  draft: CreditDraft;
  disabled?: boolean;
  allowNone?: boolean;
  onChange: (draft: CreditDraft) => void;
}) {
  const { t } = useLocaleText();
  const needsUnlimitedAcknowledgement =
    draft.kind === "unlimited" && (line?.kind || "none") !== "unlimited";

  return (
    <div className="grid min-w-0 gap-2">
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
        {allowNone ? <option value="none">{t("marketAccess.credit.none")}</option> : null}
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
  const [usdCnyRateMicros, setUsdCnyRateMicros] = React.useState(
    DEFAULT_USD_CNY_RATE_MICROS,
  );
  const [counterpartyDrafts, setCounterpartyDrafts] = React.useState<
    Record<string, CounterpartyDraft>
  >({});
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [busy, setBusy] = React.useState("");
  const [email, setEmail] = React.useState("");
  const [buyerQuery, setBuyerQuery] = React.useState("");
  const [allowedScopes, setAllowedScopes] = React.useState<Record<string, boolean>>({
    "share:free": false,
    "share:paid": false,
    "client_host:free": false,
    "client_host:paid": false,
  });
  const [newBuyerCredit, setNewBuyerCredit] = React.useState<CreditDraft>({
    kind: "limited",
    limit: "",
    unlimitedAcknowledged: false,
  });
  const [blacklistPolicy, setBlacklistPolicy] = React.useState<MarketAccessPolicy | null>(null);
  const [riskAcknowledged, setRiskAcknowledged] = React.useState(false);
  const [requestAction, setRequestAction] = React.useState<RequestAction | null>(null);
  const [approvalCredit, setApprovalCredit] = React.useState<CreditDraft>({
    kind: "limited",
    limit: "",
    unlimitedAcknowledged: false,
  });
  const [rejectionReason, setRejectionReason] = React.useState("");
  const [revokeConfirmationOpen, setRevokeConfirmationOpen] = React.useState(false);

  const load = React.useCallback(async (silent = false) => {
    if (!authed) {
      setLoading(false);
      return;
    }
    if (silent) setRefreshing(true);
    else setLoading(true);
    try {
      const [nextDashboard, billingConfig] = await Promise.all([
        getMarketAccessDashboard(),
        getMarketBillingConfig(),
      ]);
      setDashboard(nextDashboard);
      setUsdCnyRateMicros(billingConfig.usdCnyRateMicros);
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
  const paidBlackMode =
    dashboard?.policies.some(
      (policy) => policy.pricingKind === "paid" && policy.mode === "blacklist",
    ) || false;
  const publicCreditEnabled = dashboard?.publicCreditLines.some((line) => line.enabled) || false;
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
  const revokedCounterparties = React.useMemo(
    () => changedCounterparties.filter(({ draft, change }) =>
      change.statusChanged && draft?.status === "revoked"),
    [changedCounterparties],
  );
  const requestGroups = React.useMemo(
    () => groupAccessRequests(dashboard?.accessRequests || []),
    [dashboard?.accessRequests],
  );
  const requestsByCounterparty = React.useMemo(
    () => new Map((dashboard?.counterparties || []).map((counterparty) => [
      counterparty.id,
      (dashboard?.accessRequests || []).filter((request) => sameBuyer(
        counterparty.buyerUserId,
        counterparty.buyerEmail,
        request.buyerUserId,
        request.buyerEmail,
      )),
    ])),
    [dashboard?.accessRequests, dashboard?.counterparties],
  );
  const orphanRequestGroups = React.useMemo(
    () => requestGroups.filter((group) => !(dashboard?.counterparties || []).some(
      (counterparty) => sameBuyer(
        counterparty.buyerUserId,
        counterparty.buyerEmail,
        group.buyerUserId,
        group.buyerEmail,
      ),
    )),
    [dashboard?.counterparties, requestGroups],
  );
  const dirtyCounterpartyIds = React.useMemo(
    () => new Set(changedCounterparties.map(({ counterparty }) => counterparty.id)),
    [changedCounterparties],
  );
  const normalizedBuyerQuery = buyerQuery.trim().toLowerCase();
  const visibleCounterparties = React.useMemo(() => {
    if (!normalizedBuyerQuery) return dashboard?.counterparties || [];
    return (dashboard?.counterparties || []).filter((counterparty) => {
      const requests = requestsByCounterparty.get(counterparty.id) || [];
      return [
        counterparty.buyerEmail,
        counterparty.buyerUserId || "",
        counterparty.status,
        ...requests.flatMap((request) => [
          request.productKind,
          request.pricingKind,
          request.currency || "",
          "requested",
        ]),
      ]
        .join(" ")
        .toLowerCase()
        .includes(normalizedBuyerQuery);
    });
  }, [dashboard?.counterparties, normalizedBuyerQuery, requestsByCounterparty]);
  const visibleOrphanRequestGroups = React.useMemo(() => {
    if (!normalizedBuyerQuery) return orphanRequestGroups;
    return orphanRequestGroups.filter((group) =>
      [
        group.buyerEmail,
        group.buyerUserId,
        ...group.requests.flatMap((request) => [
          request.productKind,
          request.pricingKind,
          request.currency || "",
          "requested",
        ]),
      ]
        .join(" ")
        .toLowerCase()
        .includes(normalizedBuyerQuery),
    );
  }, [normalizedBuyerQuery, orphanRequestGroups]);

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
      const previousById = new Map(
        (dashboard?.counterparties || []).map((counterparty) => [counterparty.id, counterparty]),
      );
      setDashboard(nextDashboard);
      setCounterpartyDrafts((current) =>
        Object.fromEntries(
          nextDashboard.counterparties.map((counterparty) => {
            const previous = previousById.get(counterparty.id);
            const draft = current[counterparty.id];
            const preserve = !!previous && !!draft && hasCounterpartyChange(counterpartyChange(previous, draft));
            return [
              counterparty.id,
              preserve ? draft : buildCounterpartyDraft(counterparty),
            ];
          }),
        ) as Record<string, CounterpartyDraft>,
      );
    },
    [dashboard?.counterparties],
  );

  React.useEffect(() => {
    if (!authed) return;
    let active = true;
    const timer = window.setInterval(() => {
      if (busy || requestAction) return;
      void getMarketAccessDashboard()
        .then((nextDashboard) => {
          if (!active) return;
          if (changedCounterparties.length > 0) {
            setDashboard((current) => current
              ? { ...current, accessRequests: nextDashboard.accessRequests }
              : nextDashboard);
            return;
          }
          updateDashboardPreservingDrafts(nextDashboard);
        })
        .catch(() => undefined);
    }, 15_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [authed, busy, changedCounterparties.length, requestAction, updateDashboardPreservingDrafts]);

  React.useEffect(() => {
    if (changedCounterparties.length === 0) return;
    const message = t("marketAccess.leaveConfirm");
    const beforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = "";
    };
    const captureNavigation = (event: MouseEvent) => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const target = event.target instanceof Element ? event.target.closest("a[href]") : null;
      if (!(target instanceof HTMLAnchorElement) || target.target === "_blank" || target.href === window.location.href) return;
      if (!window.confirm(message)) {
        event.preventDefault();
        event.stopPropagation();
      }
    };
    window.addEventListener("beforeunload", beforeUnload);
    document.addEventListener("click", captureNavigation, true);
    return () => {
      window.removeEventListener("beforeunload", beforeUnload);
      document.removeEventListener("click", captureNavigation, true);
    };
  }, [changedCounterparties.length, t]);

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
    const paidScopeSelected = allowedScopes["share:paid"] || allowedScopes["client_host:paid"];
    if (!normalizedEmail.includes("@")) {
      toast.danger(t("marketAccess.invalidEmail"));
      return;
    }
    if (!Object.values(allowedScopes).some(Boolean)) {
      toast.danger(t("marketAccess.productRequired"));
      return;
    }
    const creditLimitMinor = newBuyerCredit.kind === "limited"
      ? parseLimitMinor(newBuyerCredit.limit)
      : undefined;
    if (paidScopeSelected && newBuyerCredit.kind === "none") {
      toast.danger(t("marketAccess.addCreditRequired"));
      return;
    }
    if (paidScopeSelected && newBuyerCredit.kind === "limited" && creditLimitMinor == null) {
      toast.danger(t("marketAccess.invalidLimit"));
      return;
    }
    if (paidScopeSelected && newBuyerCredit.kind === "unlimited" && !newBuyerCredit.unlimitedAcknowledged) {
      toast.danger(t("marketAccess.unlimitedConfirmRequired"));
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
        creditLines: paidScopeSelected ? [{
          currency: MARKET_CURRENCY,
          kind: newBuyerCredit.kind,
          limitMinor: creditLimitMinor ?? undefined,
          riskAcknowledged:
            newBuyerCredit.kind === "unlimited" && newBuyerCredit.unlimitedAcknowledged,
        }] : undefined,
      });
      setEmail("");
      setAllowedScopes({
        "share:free": false,
        "share:paid": false,
        "client_host:free": false,
        "client_host:paid": false,
      });
      setNewBuyerCredit({ kind: "limited", limit: "", unlimitedAcknowledged: false });
      setDashboard((current) =>
        current
          ? {
              ...current,
              accessRequests: current.accessRequests.filter(
                (request) =>
                  !sameBuyer(
                    savedCounterparty.buyerUserId,
                    savedCounterparty.buyerEmail,
                    request.buyerUserId,
                    request.buyerEmail,
                  ) || !allowedScopes[scopeKey(request.productKind, request.pricingKind)],
              ),
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

  const openRequestAction = (request: MarketAccessRequest, kind: "approve" | "reject") => {
    const buyerHasUnsavedChanges = changedCounterparties.some(({ counterparty }) => sameBuyer(
      counterparty.buyerUserId,
      counterparty.buyerEmail,
      request.buyerUserId,
      request.buyerEmail,
    ));
    if (buyerHasUnsavedChanges) {
      toast.info(t("marketAccess.requestSaveDraftFirst"));
      return;
    }
    const counterparty = dashboard?.counterparties.find((item) => sameBuyer(
      item.buyerUserId,
      item.buyerEmail,
      request.buyerUserId,
      request.buyerEmail,
    ));
    const existingCredit = counterparty && counterpartyCreditLine(counterparty, MARKET_CURRENCY);
    const suggestedMinor = request.dailyRateMinor ? request.dailyRateMinor * 7 : 5_000;
    setApprovalCredit({
      kind: existingCredit && existingCredit.kind !== "none" ? existingCredit.kind : "limited",
      limit: existingCredit?.limitMinor != null
        ? (existingCredit.limitMinor / 100).toFixed(2)
        : (suggestedMinor / 100).toFixed(2),
      unlimitedAcknowledged: false,
    });
    setRejectionReason("");
    setRequestAction({ kind, request });
  };

  const requestCounterparty = requestAction
    ? dashboard?.counterparties.find((counterparty) => sameBuyer(
        counterparty.buyerUserId,
        counterparty.buyerEmail,
        requestAction.request.buyerUserId,
        requestAction.request.buyerEmail,
      ))
    : undefined;
  const requestCreditLine = requestCounterparty
    ? counterpartyCreditLine(requestCounterparty, MARKET_CURRENCY)
    : undefined;

  const resolveAccessRequest = async () => {
    if (!requestAction) return;
    const { request, kind } = requestAction;
    let creditLine: {
      currency: Currency;
      kind: MarketCreditKind;
      limitMinor?: number;
      riskAcknowledged?: boolean;
      expectedRevision: number;
    } | undefined;
    if (kind === "approve" && request.pricingKind === "paid") {
      const limitMinor = approvalCredit.kind === "limited" ? parseLimitMinor(approvalCredit.limit) : undefined;
      if (approvalCredit.kind === "none" || (approvalCredit.kind === "limited" && limitMinor == null)) {
        toast.danger(t("marketAccess.invalidLimit"));
        return;
      }
      const existingUnlimitedCredit = requestCreditLine?.kind === "unlimited";
      if (
        approvalCredit.kind === "unlimited"
        && !existingUnlimitedCredit
        && !approvalCredit.unlimitedAcknowledged
      ) {
        toast.danger(t("marketAccess.unlimitedConfirmRequired"));
        return;
      }
      if (requestCounterparty?.status === "revoked" || creditChanged(requestCreditLine, approvalCredit)) {
        creditLine = {
          currency: MARKET_CURRENCY,
          kind: approvalCredit.kind,
          limitMinor: limitMinor ?? undefined,
          riskAcknowledged:
            approvalCredit.kind === "unlimited"
            && (existingUnlimitedCredit || approvalCredit.unlimitedAcknowledged),
          expectedRevision: requestCreditLine?.revision || 0,
        };
      }
    }
    if (kind === "reject" && !rejectionReason.trim()) {
      toast.danger(t("marketAccess.rejectionReasonRequired"));
      return;
    }
    setBusy(`request:${kind}:${request.id}`);
    try {
      const nextDashboard = kind === "approve"
        ? await approveMarketAccessRequest(request.id, {
            expectedRevision: request.revision,
            creditLine,
          })
        : await rejectMarketAccessRequest(request.id, request.revision, rejectionReason.trim());
      updateDashboardPreservingDrafts(nextDashboard);
      setRequestAction(null);
      toast.success(
        t(kind === "approve" ? "marketAccess.requestApproved" : "marketAccess.requestRejected", {
          email: request.buyerEmail,
        }),
      );
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  const renderAccessRequests = (requests: MarketAccessRequest[], buyerHasUnsavedChanges: boolean) => (
    <div className="grid gap-2">
      {requests.map((request) => {
        const disabled = !!busy || buyerHasUnsavedChanges;
        return (
          <div key={request.id} className="grid gap-1.5 rounded-md border border-sky-200 bg-sky-50 px-2.5 py-2">
            <div className="flex min-w-0 flex-wrap items-center gap-1.5 text-[11px] text-sky-900">
              <Chip size="sm" variant="soft" className="bg-sky-100 text-sky-800">
                {t("marketAccess.status.requested")}
              </Chip>
              <span>
                {t(request.productKind === "share" ? "marketAccess.product.share" : "marketAccess.product.clientHost")}
                {" · "}
                {t(request.pricingKind === "free" ? "marketAccess.pricing.free" : "marketAccess.pricing.paid")}
                {request.currency ? ` · ${request.currency}` : ""}
              </span>
            </div>
            <strong className="break-words text-xs font-medium text-sky-950">{request.targetLabel}</strong>
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] text-sky-800/80">
              <span className="inline-flex items-center gap-1"><Clock3 className="h-3 w-3" />{formatRequestTime(request.requestedAt, locale)}</span>
              {request.dailyRateMinor != null ? (
                <span>{formatUsdCnyMoney(
                  request.dailyRateMinor,
                  locale,
                  usdMinorToCnyMinor(request.dailyRateMinor, usdCnyRateMicros),
                )} / {t("marketBilling.day")}</span>
              ) : null}
            </div>
            <div className="flex flex-wrap gap-1.5">
              <Button
                size="sm"
                variant="primary"
                className="h-7 px-2 text-xs"
                isDisabled={disabled}
                onClick={() => openRequestAction(request, "approve")}
              >
                <Check className="h-3.5 w-3.5" />
                {t("marketAccess.approve")}
              </Button>
              <Button
                size="sm"
                variant="outline"
                className="h-7 px-2 text-xs"
                isDisabled={disabled}
                onClick={() => openRequestAction(request, "reject")}
              >
                <X className="h-3.5 w-3.5" />
                {t("marketAccess.reject")}
              </Button>
            </div>
          </div>
        );
      })}
    </div>
  );

  const resetCounterpartyChanges = () => {
    setCounterpartyDrafts(buildCounterpartyDrafts(dashboard?.counterparties || []));
  };

  const saveCounterpartyChanges = async (revokeConfirmed = false) => {
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

    if (revokedCounterparties.length > 0 && !revokeConfirmed) {
      setRevokeConfirmationOpen(true);
      return;
    }

    setBusy("save-counterparties");
    try {
      const nextDashboard = await updateMarketCounterpartiesBatch({
        updates: changedCounterparties.flatMap(({ counterparty, draft, change }) => draft ? [{
          id: counterparty.id,
          expectedRevision: counterparty.revision,
          accessRules: change.accessRules,
          status: change.statusChanged ? draft.status : undefined,
          creditLines: change.creditCurrencies.map((currency) => {
            const credit = draft.credits[currency];
            const line = counterpartyCreditLine(counterparty, currency);
            return {
              currency,
              kind: credit.kind,
              limitMinor: credit.kind === "limited" ? parseLimitMinor(credit.limit) ?? undefined : undefined,
              riskAcknowledged: credit.kind === "unlimited" && credit.unlimitedAcknowledged,
              expectedRevision: line?.revision || 0,
            };
          }),
        }] : []),
      });
      setDashboard(nextDashboard);
      setCounterpartyDrafts(buildCounterpartyDrafts(nextDashboard.counterparties));
      setRevokeConfirmationOpen(false);
      toast.success(
        t("marketAccess.buyersSaved", { count: changedCounterparties.length }),
      );
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

  const requestExposure = requestCounterparty?.exposures.find((exposure) => exposure.currency === MARKET_CURRENCY);
  const paidScopeSelected = allowedScopes["share:paid"] || allowedScopes["client_host:paid"];

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

      {paidBlackMode || publicCreditEnabled ? (
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
              const riskyOpenAccess = blacklisted && pricingKind === "paid";
              return (
                <div
                  key={`${kind}:${pricingKind}`}
                  className={cn(
                    "grid gap-3 rounded-lg border bg-card p-3",
                    riskyOpenAccess ? "border-amber-300 bg-amber-50/40" : "border-border",
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
                  <p className="text-xs leading-5 text-muted-foreground">
                    {t(blacklisted ? "marketAccess.mode.openHint" : "marketAccess.mode.trustedHint")}
                  </p>
                </div>
              );
            }),
          )}
        </div>
      </section>

      {paidBlackMode || publicCreditEnabled ? (
        <section className="grid gap-4 border-b border-border pb-5">
          <div>
            <h3 className="text-sm font-semibold">{t("marketAccess.publicCreditTitle")}</h3>
            <p className="mt-1 text-xs text-muted-foreground">{t("marketAccess.publicCreditHint")}</p>
          </div>
          {CURRENCIES.map((currency) => (
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
            {paidScopeSelected ? (
              <section className="grid gap-3 rounded-md border border-sky-200 bg-sky-50/40 p-3 sm:col-span-3 sm:grid-cols-[minmax(12rem,1fr)_minmax(14rem,2fr)] sm:items-start">
                <div>
                  <strong className="text-sm text-foreground">{t("marketAccess.addCreditTitle")}</strong>
                  <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("marketAccess.addCreditHint")}</p>
                </div>
                <CounterpartyCreditCell
                  currency={MARKET_CURRENCY}
                  draft={newBuyerCredit}
                  disabled={!!busy}
                  allowNone={false}
                  onChange={setNewBuyerCredit}
                />
              </section>
            ) : null}
          </div>
        </div>

        <div className="grid gap-3 border-t border-border pt-4">
          <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
            <h3 className="text-sm font-semibold">{t("marketAccess.buyersTitle")}</h3>
            <div className="flex w-full min-w-0 flex-wrap items-center gap-2 sm:w-auto">
              <label className="flex h-9 min-w-0 flex-1 items-center gap-2 rounded-md border border-border bg-white px-3 text-sm focus-within:border-primary/50 focus-within:ring-2 focus-within:ring-primary/10 sm:min-w-64">
                <Search className="h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
                <input
                  type="text"
                  value={buyerQuery}
                  onChange={(event) => setBuyerQuery(event.target.value)}
                  className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-muted-foreground"
                  placeholder={t("marketAccess.searchBuyers")}
                  aria-label={t("marketAccess.searchBuyers")}
                  autoComplete="off"
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

          <div className="grid gap-2">
            {visibleOrphanRequestGroups.map((group) => (
              <section key={`request:${group.key}`} className="grid gap-3 rounded-lg border border-sky-200 bg-sky-50/30 p-4">
                <div className="flex min-w-0 flex-wrap items-start justify-between gap-2">
                  <div className="min-w-0">
                    <strong className="block break-all text-sm font-medium text-foreground">{group.buyerEmail}</strong>
                    {group.buyerUserId ? <span className="mt-1 block truncate font-mono text-[10px] text-muted-foreground" title={group.buyerUserId}>{group.buyerUserId}</span> : null}
                  </div>
                  <Chip size="sm" variant="soft" className="bg-sky-100 text-sky-800">{t("marketAccess.status.requested")}</Chip>
                </div>
                {renderAccessRequests(group.requests, false)}
              </section>
            ))}
            {visibleCounterparties.map((counterparty) => {
              const draft = counterpartyDrafts[counterparty.id] || buildCounterpartyDraft(counterparty);
              const change = counterpartyChange(counterparty, draft);
              const rowChanged = hasCounterpartyChange(change);
              const editorDisabled = draft.status === "revoked" || !!busy;
              const accessRequests = requestsByCounterparty.get(counterparty.id) || [];
              const credit = counterpartyCreditLine(counterparty, MARKET_CURRENCY);
              return (
                <details
                  key={counterparty.id}
                  className={cn(
                    "group overflow-hidden rounded-lg border border-border bg-white",
                    rowChanged && "border-primary/40 shadow-[inset_3px_0_0_var(--primary)]",
                  )}
                >
                  <summary className="flex cursor-pointer list-none items-center gap-3 px-4 py-3 hover:bg-slate-50">
                    <div className="min-w-0 flex-1">
                      <div className="flex min-w-0 flex-wrap items-center gap-2">
                        <strong className="min-w-0 break-all text-sm font-medium text-foreground">{counterparty.buyerEmail}</strong>
                        <Chip size="sm" variant="soft" className={draft.status === "active" ? "bg-emerald-100 text-emerald-700" : "bg-slate-100 text-slate-600"}>
                          {draft.status === "active" ? t("marketAccess.status.active") : t("marketAccess.status.revoked")}
                        </Chip>
                        {accessRequests.length ? <Chip size="sm" variant="soft" className="bg-sky-100 text-sky-800">{t("marketAccess.requestCount", { count: accessRequests.length })}</Chip> : null}
                        {rowChanged ? <Chip size="sm" variant="soft" className="bg-blue-50 text-blue-700">{t("marketAccess.unsaved")}</Chip> : null}
                      </div>
                      <div className="mt-1 flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">
                        <span>{t("marketAccess.creditSummary", { value: credit?.kind === "limited" && credit.limitMinor != null ? formatUsdCnyMoney(credit.limitMinor, locale, usdMinorToCnyMinor(credit.limitMinor, usdCnyRateMicros)) : credit?.kind === "unlimited" ? t("marketBilling.creditUnlimited") : t("marketBilling.creditNone") })}</span>
                        {counterparty.exposures.map((exposure) => <span key={exposure.currency}>{t("marketAccess.exposureSummary", { amount: formatUsdCnyMoney(exposure.balanceMinor, locale, usdMinorToCnyMinor(exposure.balanceMinor, usdCnyRateMicros)), count: exposure.activeServiceCount })}</span>)}
                      </div>
                    </div>
                    <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180" />
                  </summary>
                  <div className="grid gap-5 border-t border-border p-4">
                    <div className="flex flex-wrap items-center justify-between gap-3">
                      <div>
                        <strong className="text-sm">{t("marketAccess.relationshipStatus")}</strong>
                        <p className="mt-0.5 text-xs text-muted-foreground">{t("marketAccess.relationshipStatusHint")}</p>
                      </div>
                      <Checkbox
                        aria-label={t("marketAccess.status.active")}
                        isSelected={draft.status === "active"}
                        isDisabled={!!busy}
                        onChange={(selected) => updateCounterpartyDraft(counterparty.id, (current) => ({ ...current, status: selected ? "active" : "revoked" }))}
                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        {draft.status === "active" ? t("marketAccess.status.active") : t("marketAccess.status.revoked")}
                      </Checkbox>
                    </div>

                    {accessRequests.length ? (
                      <section className="grid gap-2 border-t border-border pt-4">
                        <strong className="text-sm">{t("marketAccess.table.application")}</strong>
                        {renderAccessRequests(accessRequests, dirtyCounterpartyIds.has(counterparty.id))}
                      </section>
                    ) : null}

                    <div className="grid gap-4 border-t border-border pt-4 lg:grid-cols-2">
                      {(["share", "client_host"] as const).map((productKind) => (
                        <section key={productKind} className="grid gap-3">
                          <strong className="text-sm">{productKind === "share" ? t("marketAccess.product.share") : t("marketAccess.product.clientHost")}</strong>
                          {(["free", "paid"] as const).map((pricingKind) => {
                            const key = scopeKey(productKind, pricingKind);
                            const label = pricingKind === "free" ? t("marketAccess.pricing.free") : t("marketAccess.pricing.paid");
                            return (
                              <label key={pricingKind} className="grid gap-1 text-xs text-muted-foreground sm:grid-cols-[5rem_minmax(0,1fr)] sm:items-center">
                                <span>{label}</span>
                                <select
                                  value={draft.decisions[key]}
                                  disabled={editorDisabled}
                                  aria-label={`${productKind === "share" ? t("marketAccess.product.share") : t("marketAccess.product.clientHost")} · ${label}`}
                                  onChange={(event) => updateCounterpartyDraft(counterparty.id, (current) => ({ ...current, decisions: { ...current.decisions, [key]: event.target.value as MarketAccessDecision } }))}
                                  className="h-9 min-w-0 rounded-md border border-border bg-white px-2 text-xs text-foreground disabled:bg-slate-50"
                                >
                                  <option value="inherit">{t("marketAccess.decision.inherit")}</option>
                                  <option value="allow">{t("marketAccess.decision.allow")}</option>
                                  <option value="deny">{t("marketAccess.decision.deny")}</option>
                                </select>
                              </label>
                            );
                          })}
                        </section>
                      ))}
                    </div>

                    <div className="grid gap-4 border-t border-border pt-4 md:grid-cols-2">
                      <section className="grid gap-2">
                        <strong className="text-sm">USD {t("marketAccess.creditType")}</strong>
                        <CounterpartyCreditCell
                          currency={MARKET_CURRENCY}
                          line={credit}
                          draft={draft.credits[MARKET_CURRENCY]}
                          disabled={editorDisabled}
                          onChange={(nextCredit) => updateCounterpartyDraft(counterparty.id, (current) => ({ ...current, credits: { ...current.credits, [MARKET_CURRENCY]: nextCredit } }))}
                        />
                      </section>
                      <section className="grid content-start gap-2">
                        <strong className="text-sm">{t("marketAccess.table.exposure")}</strong>
                        {counterparty.exposures.length ? counterparty.exposures.map((exposure) => (
                          <div key={exposure.currency} className="text-sm">
                            <strong className="block tabular-nums">{formatUsdCnyMoney(exposure.balanceMinor, locale, usdMinorToCnyMinor(exposure.balanceMinor, usdCnyRateMicros))}</strong>
                            <span className="text-xs text-muted-foreground">{t("marketAccess.activeServices", { count: exposure.activeServiceCount })}</span>
                          </div>
                        )) : <span className="text-sm text-muted-foreground">-</span>}
                      </section>
                    </div>
                  </div>
                </details>
              );
            })}
            {visibleCounterparties.length === 0 && visibleOrphanRequestGroups.length === 0 ? (
              <p className="rounded-lg border border-dashed border-border px-4 py-12 text-center text-sm text-muted-foreground">
                {(dashboard?.counterparties.length || dashboard?.accessRequests.length) ? t("marketAccess.noMatches") : t("marketAccess.empty")}
              </p>
            ) : null}
          </div>
        </div>
      </section>

      <Modal.Backdrop isOpen={!!requestAction} onOpenChange={(open) => !open && !busy && setRequestAction(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(600px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t(requestAction?.kind === "reject" ? "marketAccess.rejectTitle" : "marketAccess.approveTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[70vh] gap-4 overflow-y-auto">
              {requestAction ? (
                <div className="grid gap-2 rounded-md border border-border bg-slate-50 p-3 text-sm">
                  <div className="flex flex-wrap items-start justify-between gap-2">
                    <strong className="break-all">{requestAction.request.buyerEmail}</strong>
                    <span className="text-xs text-muted-foreground">{formatRequestTime(requestAction.request.requestedAt, locale)}</span>
                  </div>
                  <span>{requestAction.request.targetLabel}</span>
                  <div className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
                    <span>{t(requestAction.request.productKind === "share" ? "marketAccess.product.share" : "marketAccess.product.clientHost")}</span>
                    <span>{t(requestAction.request.pricingKind === "free" ? "marketAccess.pricing.free" : "marketAccess.pricing.paid")}</span>
                    {requestAction.request.dailyRateMinor != null ? (
                      <span>{formatUsdCnyMoney(requestAction.request.dailyRateMinor, locale, usdMinorToCnyMinor(requestAction.request.dailyRateMinor, usdCnyRateMicros))} / {t("marketBilling.day")}</span>
                    ) : null}
                  </div>
                </div>
              ) : null}

              {requestAction?.kind === "approve" && requestAction.request.pricingKind === "paid" ? (
                <section className="grid gap-3">
                  <div>
                    <strong className="text-sm">{t("marketAccess.approvalCreditTitle")}</strong>
                    <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("marketAccess.approvalCreditHint")}</p>
                  </div>
                  {requestExposure ? (
                    <div className="grid grid-cols-2 gap-3 rounded-md border border-border px-3 py-2 text-xs">
                      <span className="text-muted-foreground">{t("marketAccess.currentExposure")}</span>
                      <strong className="text-right tabular-nums">{formatUsdCnyMoney(requestExposure.balanceMinor, locale, usdMinorToCnyMinor(requestExposure.balanceMinor, usdCnyRateMicros))}</strong>
                      <span className="text-muted-foreground">{t("marketAccess.activeServicesLabel")}</span>
                      <strong className="text-right tabular-nums">{requestExposure.activeServiceCount}</strong>
                    </div>
                  ) : null}
                  <CounterpartyCreditCell
                    currency={MARKET_CURRENCY}
                    line={requestCreditLine}
                    draft={approvalCredit}
                    disabled={!!busy}
                    allowNone={false}
                    onChange={setApprovalCredit}
                  />
                </section>
              ) : null}

              {requestAction?.kind === "approve" && requestAction.request.pricingKind === "free" ? (
                <p className="rounded-md border border-emerald-200 bg-emerald-50 px-3 py-2 text-sm leading-6 text-emerald-900">{t("marketAccess.approveFreeHint")}</p>
              ) : null}

              {requestAction?.kind === "reject" ? (
                <>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("marketAccess.rejectionReason")}</span>
                    <textarea value={rejectionReason} onChange={(event) => setRejectionReason(event.target.value)} maxLength={2000} rows={4} autoFocus className="rounded-md border border-border p-3" />
                  </label>
                  <p className="flex gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-950">
                    <Clock3 className="mt-0.5 h-4 w-4 shrink-0" />
                    {t("marketAccess.rejectionCooldown")}
                  </p>
                </>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!!busy} onClick={() => setRequestAction(null)}>{t("common.cancel")}</Button>
              <Button variant={requestAction?.kind === "reject" ? "danger" : "primary"} isDisabled={!!busy} onClick={() => void resolveAccessRequest()}>
                {busy.startsWith("request:") ? <Loader2 className="h-4 w-4 animate-spin" /> : requestAction?.kind === "reject" ? <X className="h-4 w-4" /> : <Check className="h-4 w-4" />}
                {t(requestAction?.kind === "reject" ? "marketAccess.rejectConfirm" : "marketAccess.approveConfirm")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <Modal.Backdrop isOpen={revokeConfirmationOpen} onOpenChange={(open) => !open && !busy && setRevokeConfirmationOpen(false)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("marketAccess.revokeConfirmTitle")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid gap-4">
              <div className="flex gap-3 rounded-md border border-rose-200 bg-rose-50 p-3 text-sm leading-6 text-rose-900">
                <AlertTriangle className="mt-1 h-4 w-4 shrink-0" />
                <span>{t("marketAccess.revokeConfirmDescription")}</span>
              </div>
              <div className="grid max-h-60 gap-2 overflow-y-auto">
                {revokedCounterparties.map(({ counterparty }) => {
                  const exposure = counterparty.exposures.find((item) => item.currency === MARKET_CURRENCY);
                  return (
                    <div key={counterparty.id} className="flex flex-wrap items-center justify-between gap-2 border-b border-border py-2 text-sm last:border-b-0">
                      <strong className="break-all">{counterparty.buyerEmail}</strong>
                      <span className="text-xs text-muted-foreground">{exposure ? t("marketAccess.revokeImpact", { amount: formatUsdCnyMoney(exposure.balanceMinor, locale, usdMinorToCnyMinor(exposure.balanceMinor, usdCnyRateMicros)), count: exposure.activeServiceCount }) : t("marketAccess.revokeNoExposure")}</span>
                    </div>
                  );
                })}
              </div>
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!!busy} onClick={() => setRevokeConfirmationOpen(false)}>{t("common.cancel")}</Button>
              <Button variant="danger" isDisabled={!!busy} onClick={() => void saveCounterpartyChanges(true)}>
                {busy === "save-counterparties" ? <Loader2 className="h-4 w-4 animate-spin" /> : <AlertTriangle className="h-4 w-4" />}
                {t("marketAccess.revokeConfirm")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

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
