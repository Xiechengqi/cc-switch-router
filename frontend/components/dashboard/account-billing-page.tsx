"use client";

import * as React from "react";
import { Button, Chip, Modal, toast } from "@heroui/react";
import {
  AlertTriangle,
  Ban,
  Check,
  ChevronDown,
  CircleDollarSign,
  Clock3,
  ExternalLink,
  HandCoins,
  Loader2,
  RefreshCw,
  ReceiptText,
  Scale,
  Server,
  Settings2,
  ShieldAlert,
  Users,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import {
  ProviderContactsList,
  ProviderPaymentMethodsList,
} from "@/components/common/provider-contacts";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  closeMarketBillingAccount,
  confirmMarketBillingPayment,
  declareMarketBillingPayment,
  disputeMarketBillingInvoice,
  getAdminMarketBillingDisputes,
  getMarketBillingDashboard,
  getMarketBillingInvoiceHistory,
  rejectMarketBillingPayment,
  requestMarketBillingSettlement,
  resolveAdminMarketBillingDispute,
  settleMarketBillingAccount,
  updateMarketBillingSupplierProfile,
  voidAdminMarketBillingInvoice,
} from "@/lib/api";
import type {
  AdminMarketBillingDispute,
  MarketBillingDashboard,
  MarketBillingInvoice,
  MarketBillingPaymentDeclaration,
  MarketCreditAccount,
} from "@/lib/types";

type Currency = "CNY" | "USD";
type ProfileDraft = { graceHours: string };
type Action =
  | { kind: "settle" | "request-settlement" | "close"; account: MarketCreditAccount }
  | { kind: "declare" | "confirm" | "reject" | "dispute"; account: MarketCreditAccount; invoice: MarketBillingInvoice }
  | { kind: "admin-uphold" | "admin-void"; dispute: AdminMarketBillingDispute }
  | { kind: "admin-invoice-void"; dispute: AdminMarketBillingDispute };

const EMPTY_PROFILE: ProfileDraft = { graceHours: "24" };

function formatMoney(value: number, currency: string, locale: string) {
  return new Intl.NumberFormat(locale, {
    style: "currency",
    currency: currency === "CNY" ? "CNY" : "USD",
  }).format(value / 100);
}

function formatDate(value: string | undefined, locale: string) {
  if (!value) return "-";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(date);
}

function formatDuration(seconds: number) {
  const safeSeconds = Math.max(0, Math.floor(seconds));
  if (safeSeconds < 60) return `${safeSeconds}s`;
  if (safeSeconds < 3600) return `${(safeSeconds / 60).toFixed(safeSeconds % 60 === 0 ? 0 : 1)}m`;
  return `${(safeSeconds / 3600).toFixed(safeSeconds % 3600 === 0 ? 0 : 2)}h`;
}

function statusTone(status: string) {
  if (status === "overdue") return "bg-rose-100 text-rose-700";
  if (status === "settlement_due" || status === "near_credit_limit") return "bg-amber-100 text-amber-800";
  if (status === "payment_declared" || status === "disputed") return "bg-sky-100 text-sky-700";
  if (status === "closed" || status === "void") return "bg-slate-100 text-slate-600";
  return "bg-emerald-100 text-emerald-700";
}

function productLabel(kind: string, t: ReturnType<typeof useLocaleText>["t"]) {
  return kind === "share" ? t("marketBilling.product.share") : t("marketBilling.product.clientHost");
}

function accountStatusLabel(status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  switch (status) {
    case "active": return t("marketBilling.status.active");
    case "near_credit_limit": return t("marketBilling.status.nearCreditLimit");
    case "settlement_due": return t("marketBilling.status.settlementDue");
    case "payment_declared": return t("marketBilling.status.paymentDeclared");
    case "overdue": return t("marketBilling.status.overdue");
    case "disputed": return t("marketBilling.status.disputed");
    case "paid": return t("marketBilling.status.paid");
    case "void": return t("marketBilling.status.void");
    case "closed": return t("marketBilling.status.closed");
    default: return status.replaceAll("_", " ");
  }
}

function serviceStatusLabel(status: string, t: ReturnType<typeof useLocaleText>["t"]) {
  switch (status) {
    case "trial": return t("marketBilling.service.trial");
    case "active": return t("marketBilling.service.active");
    case "billing_suspended": return t("marketBilling.service.suspended");
    case "terminated": return t("marketBilling.service.terminated");
    default: return status.replaceAll("_", " ");
  }
}

function trialTime(seconds: number, t: ReturnType<typeof useLocaleText>["t"]) {
  const hours = Math.floor(Math.max(0, seconds) / 3600);
  const minutes = Math.floor((Math.max(0, seconds) % 3600) / 60);
  return t("marketBilling.service.trialRemaining", { hours, minutes });
}

function InvoiceLines({ invoice, locale }: { invoice: MarketBillingInvoice; locale: string }) {
  const { t } = useLocaleText();
  return (
    <div className="overflow-x-auto border-t border-border">
      <table className="w-full min-w-[38rem] text-left text-xs">
        <thead className="text-muted-foreground">
          <tr>
            <th className="px-3 py-2 font-medium">{t("marketBilling.service")}</th>
            <th className="px-3 py-2 font-medium">{t("marketBilling.period")}</th>
            <th className="px-3 py-2 text-right font-medium">{t("marketBilling.billableTime")}</th>
            <th className="px-3 py-2 text-right font-medium">{t("marketBilling.amount")}</th>
          </tr>
        </thead>
        <tbody>
          {invoice.lines.map((line) => (
            <tr key={line.id} className="border-t border-border/70">
              <td className="px-3 py-2.5">
                <span className="block font-medium text-foreground">{line.serviceLabel}</span>
                <span className="text-muted-foreground">{productLabel(line.productKind, t)}</span>
              </td>
              <td className="px-3 py-2.5 text-muted-foreground">
                {formatDate(line.serviceStartedAt, locale)} - {formatDate(line.serviceEndedAt, locale)}
              </td>
              <td className="px-3 py-2.5 text-right tabular-nums">
                {formatDuration(line.billableSeconds)}
              </td>
              <td className="px-3 py-2.5 text-right font-medium tabular-nums">
                {formatMoney(line.amountMinor, invoice.currency, locale)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function safeEvidenceHref(value: string | undefined) {
  if (!value) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === "http:" || url.protocol === "https:" ? url.toString() : undefined;
  } catch {
    return undefined;
  }
}

function PaymentDeclarationDetails({
  declaration,
  locale,
}: {
  declaration: MarketBillingPaymentDeclaration;
  locale: string;
}) {
  const { t } = useLocaleText();
  const evidenceHref = safeEvidenceHref(declaration.evidenceUrl);
  return (
    <div className="grid gap-1.5 text-xs text-muted-foreground">
      <p>{t("marketBilling.declaration.summary", {
        date: formatDate(declaration.declaredAt, locale),
        reference: declaration.paymentReference || "-",
      })}</p>
      {declaration.paymentMethodKind ? (
        <p><span className="font-medium text-foreground">{t("marketBilling.declare.method")}:</span> {declaration.paymentMethodKind}</p>
      ) : null}
      {declaration.note ? (
        <p className="whitespace-pre-wrap break-words"><span className="font-medium text-foreground">{t("marketBilling.note")}:</span> {declaration.note}</p>
      ) : null}
      {declaration.evidenceUrl ? (
        <p className="break-all">
          <span className="font-medium text-foreground">{t("marketBilling.declare.evidence")}:</span>{" "}
          {evidenceHref ? (
            <a href={evidenceHref} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-primary hover:underline">
              {t("marketBilling.declare.openEvidence")}<ExternalLink className="h-3 w-3 shrink-0" aria-hidden />
            </a>
          ) : declaration.evidenceUrl}
        </p>
      ) : null}
      {declaration.rejectionReason ? (
        <p className="whitespace-pre-wrap break-words text-rose-700">{declaration.rejectionReason}</p>
      ) : null}
    </div>
  );
}

function CreditAccountPanel({
  account,
  perspective,
  onAction,
}: {
  account: MarketCreditAccount;
  perspective: "buyer" | "supplier";
  onAction: (action: Action) => void;
}) {
  const { locale, t } = useLocaleText();
  const invoice = account.openInvoice;
  const counterparty = perspective === "buyer" ? account.supplierEmail : account.buyerEmail;
  const utilization = account.utilizationBps == null
    ? null
    : Math.min(100, Math.max(0, account.utilizationBps / 100));
  const invoiceCanBePaid = perspective === "buyer" && invoice && ["open", "overdue"].includes(invoice.status);
  const invoiceCanBeReviewed = perspective === "supplier" && invoice?.status === "payment_declared";
  const [historyOpen, setHistoryOpen] = React.useState(false);
  const [history, setHistory] = React.useState<MarketBillingInvoice[] | null>(null);
  const [historyCursor, setHistoryCursor] = React.useState<number | undefined>();
  const [historyLoading, setHistoryLoading] = React.useState(false);

  React.useEffect(() => {
    setHistoryOpen(false);
    setHistory(null);
    setHistoryCursor(undefined);
  }, [account.openInvoice?.id]);

  async function loadHistory(beforeSequence?: number) {
    setHistoryLoading(true);
    try {
      const page = await getMarketBillingInvoiceHistory(account.id, beforeSequence);
      setHistory((current) => beforeSequence == null ? page.invoices : [...(current || []), ...page.invoices]);
      setHistoryCursor(page.nextBeforeSequence ?? undefined);
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setHistoryLoading(false);
    }
  }

  function toggleHistory() {
    const nextOpen = !historyOpen;
    setHistoryOpen(nextOpen);
    if (nextOpen && history == null) void loadHistory();
  }

  return (
    <section className="overflow-hidden rounded-lg border border-border bg-card">
      <div className="grid gap-4 p-4 sm:p-5">
        <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
          <div className="min-w-0">
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <Users className="h-4 w-4 shrink-0 text-muted-foreground" />
              <strong className="min-w-0 break-all text-sm">{counterparty}</strong>
              <Chip size="sm" variant="soft" className={statusTone(account.status)}>
                {accountStatusLabel(account.status, t)}
              </Chip>
            </div>
            <p className="mt-1 text-xs text-muted-foreground">
              {perspective === "buyer" ? t("marketBilling.supplier") : t("marketBilling.buyer")} · {account.currency}
            </p>
          </div>
          <div className="text-right">
            <strong className="block text-lg tabular-nums text-foreground">
              {formatMoney(invoice?.amountMinor ?? account.balanceMinor, account.currency, locale)}
            </strong>
            <span className="text-xs text-muted-foreground">
              {invoice ? t("marketBilling.invoice.total") : t("marketBilling.unbilledBalance")}
            </span>
          </div>
        </div>

        <div className="grid gap-3 sm:grid-cols-3">
          <div>
            <span className="text-xs text-muted-foreground">{t("marketBilling.creditLimit")}</span>
            <strong className="mt-0.5 block text-sm tabular-nums">
              {account.creditKind === "unlimited"
                ? t("marketBilling.creditUnlimited")
                : account.creditLimitMinor != null
                  ? formatMoney(account.creditLimitMinor, account.currency, locale)
                  : t("marketBilling.creditNone")}
            </strong>
          </div>
          <div>
            <span className="text-xs text-muted-foreground">{t("marketBilling.dailyExposure")}</span>
            <strong className="mt-0.5 block text-sm tabular-nums">
              {formatMoney(account.dailyRateMinor, account.currency, locale)}
            </strong>
          </div>
          <div>
            <span className="text-xs text-muted-foreground">{t("marketBilling.estimatedSettlement")}</span>
            <strong className="mt-0.5 block text-sm">
              {account.estimatedSettlementAt ? formatDate(account.estimatedSettlementAt, locale) : "-"}
            </strong>
          </div>
        </div>

        {utilization != null ? <div>
          <div className="mb-1 flex items-center justify-between text-xs text-muted-foreground">
            <span>{t("marketBilling.creditUsed")}</span>
            <span className="tabular-nums">{utilization.toFixed(1)}%</span>
          </div>
          <div className="h-2 overflow-hidden rounded-full bg-slate-100">
            <div
              className={`h-full rounded-full ${utilization >= 100 ? "bg-rose-500" : utilization >= 80 ? "bg-amber-500" : "bg-emerald-500"}`}
              style={{ width: `${utilization}%` }}
            />
          </div>
        </div> : null}

        {account.closeRequested ? (
          <div className="flex gap-2 rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-rose-900">
            <Ban className="mt-0.5 h-4 w-4 shrink-0" />
            <div>
              <strong className="text-xs">{t("marketBilling.closing.title")}</strong>
              <p className="mt-0.5 text-xs leading-5">{t("marketBilling.closing.description")}</p>
            </div>
          </div>
        ) : null}

        <div className="grid gap-2 border-t border-border pt-3">
          <div className="flex items-center justify-between gap-3">
            <strong className="text-xs uppercase text-muted-foreground">{t("marketBilling.services")}</strong>
            <span className="text-xs text-muted-foreground">{account.services.length}</span>
          </div>
          {account.services.length ? account.services.map((service) => (
            <div key={service.id} className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 py-1 text-sm">
              <Server className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
              <span className="min-w-0 flex-1 truncate font-medium" title={service.serviceLabel}>{service.serviceLabel}</span>
              <span className="text-xs text-muted-foreground">{productLabel(service.productKind, t)}</span>
              <span className="text-xs tabular-nums">{formatMoney(service.dailyRateMinor, account.currency, locale)} / {t("marketBilling.day")}</span>
              <Chip size="sm" variant="soft">{serviceStatusLabel(service.status, t)}</Chip>
              {service.trialSecondsRemaining > 0 ? (
                <span className="w-full pl-5 text-xs text-muted-foreground">
                  {trialTime(service.trialSecondsRemaining, t)} · {t("marketBilling.healthOnly")}
                </span>
              ) : null}
            </div>
          )) : <p className="text-sm text-muted-foreground">{t("marketBilling.noServices")}</p>}
        </div>
      </div>

      {invoice ? (
        <div className="border-t border-border bg-slate-50/70">
          <div className="flex flex-wrap items-center justify-between gap-3 px-4 py-3 sm:px-5">
            <div>
              <div className="flex flex-wrap items-center gap-2">
                <CircleDollarSign className="h-4 w-4 text-muted-foreground" />
                <strong className="text-sm">{t("marketBilling.invoice.number", { sequence: invoice.sequence })}</strong>
                <Chip size="sm" variant="soft" className={statusTone(invoice.status)}>
                  {accountStatusLabel(invoice.status, t)}
                </Chip>
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                {t("marketBilling.invoice.deadline", { date: formatDate(invoice.deadlineAt, locale) })}
              </p>
            </div>
            <div className="flex flex-wrap gap-2">
              {invoiceCanBePaid ? (
                <Button size="sm" variant="primary" onClick={() => onAction({ kind: "declare", account, invoice })}>
                  <HandCoins className="h-4 w-4" />
                  {t("marketBilling.action.declare")}
                </Button>
              ) : null}
              {perspective === "buyer" && ["open", "overdue", "payment_declared"].includes(invoice.status) && !invoice.dispute ? (
                <Button size="sm" variant="outline" onClick={() => onAction({ kind: "dispute", account, invoice })}>
                  <Scale className="h-4 w-4" />
                  {t("marketBilling.action.dispute")}
                </Button>
              ) : null}
              {invoiceCanBeReviewed ? (
                <>
                  <Button size="sm" variant="primary" onClick={() => onAction({ kind: "confirm", account, invoice })}>
                    <Check className="h-4 w-4" />
                    {t("marketBilling.action.confirm")}
                  </Button>
                  <Button size="sm" variant="outline" onClick={() => onAction({ kind: "reject", account, invoice })}>
                    <Ban className="h-4 w-4" />
                    {t("marketBilling.action.reject")}
                  </Button>
                </>
              ) : null}
            </div>
          </div>
          <InvoiceLines invoice={invoice} locale={locale} />
          {invoice.declaration ? (
            <div className="border-t border-border px-4 py-3 sm:px-5">
              <PaymentDeclarationDetails declaration={invoice.declaration} locale={locale} />
            </div>
          ) : null}
          {invoice.dispute ? (
            <div className="border-t border-sky-200 bg-sky-50 px-4 py-3 text-xs text-sky-800 sm:px-5">
              {t("marketBilling.dispute.summary", { reason: invoice.dispute.reason })}
            </div>
          ) : null}
        </div>
      ) : null}

      <div className="border-t border-border">
        <button
          type="button"
          className="flex w-full items-center justify-between gap-3 px-4 py-3 text-left text-sm font-medium hover:bg-slate-50 sm:px-5"
          aria-expanded={historyOpen}
          onClick={toggleHistory}
        >
          <span className="flex items-center gap-2">
            <ReceiptText className="h-4 w-4 text-muted-foreground" />
            {t("marketBilling.history.title")}
          </span>
          <ChevronDown className={`h-4 w-4 text-muted-foreground transition-transform ${historyOpen ? "rotate-180" : ""}`} />
        </button>
        {historyOpen ? (
          <div className="border-t border-border">
            {historyLoading && history == null ? (
              <p className="px-4 py-5 text-sm text-muted-foreground sm:px-5">{t("marketBilling.history.loading")}</p>
            ) : history?.length ? (
              <>
                {history.map((item) => (
                  <details key={item.id} className="group border-b border-border last:border-b-0">
                    <summary className="flex cursor-pointer list-none flex-wrap items-center justify-between gap-3 px-4 py-3 hover:bg-slate-50 sm:px-5">
                      <span className="flex min-w-0 items-center gap-2">
                        <ChevronDown className="h-4 w-4 shrink-0 -rotate-90 text-muted-foreground transition-transform group-open:rotate-0" />
                        <span className="text-sm font-medium">{t("marketBilling.invoice.number", { sequence: item.sequence })}</span>
                        <Chip size="sm" variant="soft" className={statusTone(item.status)}>
                          {accountStatusLabel(item.status, t)}
                        </Chip>
                      </span>
                      <span className="text-right">
                        <strong className="block text-sm tabular-nums">{formatMoney(item.amountMinor, item.currency, locale)}</strong>
                        <span className="text-xs text-muted-foreground">{formatDate(item.paidAt || item.openedAt, locale)}</span>
                      </span>
                    </summary>
                    <InvoiceLines invoice={item} locale={locale} />
                    {item.declaration ? (
                      <div className="border-t border-border px-4 py-3 sm:px-5">
                        <PaymentDeclarationDetails declaration={item.declaration} locale={locale} />
                      </div>
                    ) : null}
                    {item.dispute ? (
                      <p className="border-t border-border px-4 py-3 text-xs text-muted-foreground sm:px-5">
                        {t("marketBilling.dispute.summary", { reason: item.dispute.reason })}
                      </p>
                    ) : null}
                  </details>
                ))}
                {historyCursor != null ? (
                  <div className="flex justify-center border-t border-border px-4 py-3">
                    <Button size="sm" variant="ghost" isDisabled={historyLoading} onClick={() => void loadHistory(historyCursor)}>
                      {historyLoading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                      {t("marketBilling.history.loadMore")}
                    </Button>
                  </div>
                ) : null}
              </>
            ) : (
              <p className="px-4 py-5 text-sm text-muted-foreground sm:px-5">{t("marketBilling.history.empty")}</p>
            )}
          </div>
        ) : null}
      </div>

      {(!invoice && account.canSettle) || (!invoice && perspective === "supplier" && account.balanceMinor > 0 && ["active", "near_credit_limit"].includes(account.status)) || account.canClose ? (
        <div className="flex flex-wrap justify-end gap-2 border-t border-border px-4 py-3 sm:px-5">
          {!invoice && account.canSettle ? (
            <Button size="sm" variant="outline" onClick={() => onAction({ kind: "settle", account })}>
              <HandCoins className="h-4 w-4" />
              {t("marketBilling.action.settle")}
            </Button>
          ) : null}
          {!invoice && perspective === "supplier" && account.balanceMinor > 0 && ["active", "near_credit_limit"].includes(account.status) ? (
            <Button size="sm" variant="outline" onClick={() => onAction({ kind: "request-settlement", account })}>
              <ReceiptText className="h-4 w-4" />
              {t("marketBilling.action.requestSettlement")}
            </Button>
          ) : null}
          {account.canClose ? (
            <Button size="sm" variant="outline" className="text-rose-700" onClick={() => onAction({ kind: "close", account })}>
              <Ban className="h-4 w-4" />
              {t("marketBilling.action.closeAccount")}
            </Button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

export function AccountBillingPage() {
  const { locale, t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;
  const isAdmin = !!session?.isAdmin;
  const [dashboard, setDashboard] = React.useState<MarketBillingDashboard | null>(null);
  const [adminDisputes, setAdminDisputes] = React.useState<AdminMarketBillingDispute[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [busy, setBusy] = React.useState("");
  const [action, setAction] = React.useState<Action | null>(null);
  const [reason, setReason] = React.useState("");
  const [reference, setReference] = React.useState("");
  const [note, setNote] = React.useState("");
  const [evidenceUrl, setEvidenceUrl] = React.useState("");
  const [paymentKind, setPaymentKind] = React.useState("");
  const [profiles, setProfiles] = React.useState<Record<Currency, ProfileDraft>>({
    CNY: { ...EMPTY_PROFILE },
    USD: { ...EMPTY_PROFILE },
  });
  const [profileDirty, setProfileDirty] = React.useState<Record<Currency, boolean>>({
    CNY: false,
    USD: false,
  });

  const load = React.useCallback(async (silent = false) => {
    if (!authed) return;
    if (silent) setRefreshing(true);
    else setLoading(true);
    try {
      const [nextDashboard, nextDisputes] = await Promise.all([
        getMarketBillingDashboard(),
        isAdmin ? getAdminMarketBillingDisputes() : Promise.resolve([]),
      ]);
      setDashboard(nextDashboard);
      setAdminDisputes(nextDisputes);
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, [authed, isAdmin]);

  React.useEffect(() => {
    if (!authed) {
      setLoading(false);
      return;
    }
    void load();
    const timer = window.setInterval(() => void load(true), 20_000);
    return () => window.clearInterval(timer);
  }, [authed, load]);

  React.useEffect(() => {
    if (!dashboard) return;
    setProfiles((current) => {
      const next = { CNY: { ...current.CNY }, USD: { ...current.USD } };
      for (const currency of ["CNY", "USD"] as const) {
        if (profileDirty[currency]) continue;
        const profile = dashboard.supplierProfiles.find((item) => item.currency === currency);
        if (profile) {
          next[currency] = {
            graceHours: String(profile.settlementGraceHours),
          };
        }
      }
      return next;
    });
  }, [dashboard, profileDirty]);

  const openAction = (next: Action) => {
    setReason("");
    setReference("");
    setNote("");
    setEvidenceUrl("");
    setPaymentKind(next.kind === "declare" ? next.invoice.paymentMethods[0]?.kind || "" : "");
    setAction(next);
  };

  const saveProfile = async (currency: Currency) => {
    const draft = profiles[currency];
    const graceHours = Number(draft.graceHours);
    if (!Number.isSafeInteger(graceHours) || graceHours < 1 || graceHours > 720) {
      toast.danger(t("marketBilling.profile.invalidGrace"));
      return;
    }
    setBusy(`profile:${currency}`);
    try {
      setDashboard(await updateMarketBillingSupplierProfile(currency, graceHours));
      setProfileDirty((current) => ({ ...current, [currency]: false }));
      toast.success(t("marketBilling.profile.saved", { currency }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  const submitAction = async () => {
    if (!action || busy) return;
    const actionKey = action.kind;
    setBusy(actionKey);
    try {
      let nextDashboard: MarketBillingDashboard | null = null;
      switch (action.kind) {
        case "settle":
          nextDashboard = await settleMarketBillingAccount(action.account.id);
          break;
        case "request-settlement":
          await requestMarketBillingSettlement(action.account.id);
          break;
        case "close":
          nextDashboard = await closeMarketBillingAccount(action.account.id);
          break;
        case "declare":
          nextDashboard = await declareMarketBillingPayment(action.invoice.id, {
            paymentMethodKind: paymentKind || undefined,
            paymentReference: reference || undefined,
            note: note || undefined,
            evidenceUrl: evidenceUrl || undefined,
          });
          break;
        case "confirm":
          nextDashboard = await confirmMarketBillingPayment(action.invoice.id);
          break;
        case "reject":
          if (!reason.trim()) throw new Error(t("marketBilling.reasonRequired"));
          nextDashboard = await rejectMarketBillingPayment(action.invoice.id, reason.trim());
          break;
        case "dispute":
          if (!reason.trim()) throw new Error(t("marketBilling.reasonRequired"));
          nextDashboard = await disputeMarketBillingInvoice(action.invoice.id, reason.trim());
          break;
        case "admin-uphold":
          await resolveAdminMarketBillingDispute(action.dispute.dispute.id, "uphold", note || undefined);
          break;
        case "admin-void":
          await resolveAdminMarketBillingDispute(action.dispute.dispute.id, "void", note || undefined);
          break;
        case "admin-invoice-void":
          if (!reason.trim()) throw new Error(t("marketBilling.reasonRequired"));
          await voidAdminMarketBillingInvoice(action.dispute.invoice.id, reason.trim());
          break;
      }
      if (nextDashboard) setDashboard(nextDashboard);
      setAction(null);
      toast.success(t("marketBilling.action.completed"));
      if (action.kind.startsWith("admin-") || action.kind === "request-settlement") await load(true);
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy("");
    }
  };

  if (authLoading || loading) {
    return <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  }
  if (!authed) {
    return <p className="py-8 text-sm text-muted-foreground">{t("account.signInRequired")}</p>;
  }

  const buyerAccounts = dashboard?.accounts.filter((account) => account.isBuyer) || [];
  const supplierAccounts = dashboard?.accounts.filter((account) => account.isSupplier) || [];
  const restrictions = dashboard?.restrictions || [];

  const actionInvoice = action && "invoice" in action ? action.invoice : action && "dispute" in action ? action.dispute.invoice : undefined;
  const needsReason = action && ["reject", "dispute", "admin-invoice-void"].includes(action.kind);
  const needsAdminNote = action && ["admin-uphold", "admin-void"].includes(action.kind);

  return (
    <div className="grid min-w-0 gap-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <HandCoins className="h-5 w-5 text-muted-foreground" />
            <h2 className="text-lg font-semibold text-foreground">{t("marketBilling.title")}</h2>
          </div>
          <p className="mt-1 max-w-3xl text-sm text-muted-foreground">{t("marketBilling.subtitle", { hours: dashboard?.trialHours || 12 })}</p>
        </div>
        <Button isIconOnly variant="outline" aria-label={t("common.reload")} isDisabled={refreshing} onClick={() => void load(true)}>
          <RefreshCw className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`} />
        </Button>
      </div>

      {restrictions.length ? (
        <section className="flex gap-3 rounded-lg border border-rose-200 bg-rose-50 p-4 text-rose-900">
          <ShieldAlert className="mt-0.5 h-5 w-5 shrink-0" />
          <div>
            <strong className="text-sm">{t("marketBilling.restricted.title")}</strong>
            <p className="mt-1 text-sm leading-6">{t("marketBilling.restricted.description")}</p>
          </div>
        </section>
      ) : null}

      <section className="grid gap-4 border-y border-border py-5">
        <div className="flex items-start gap-2">
          <Settings2 className="mt-0.5 h-4 w-4 text-muted-foreground" />
          <div>
            <h3 className="text-sm font-semibold">{t("marketBilling.profile.title")}</h3>
            <p className="mt-0.5 text-xs text-muted-foreground">{t("marketBilling.profile.hint")}</p>
          </div>
        </div>
        <div className="grid gap-3 xl:grid-cols-2">
          {(["CNY", "USD"] as const).map((currency) => {
            const profile = dashboard?.supplierProfiles.find((item) => item.currency === currency);
            const draft = profiles[currency];
            return (
              <div key={currency} className="grid gap-3 rounded-lg border border-border bg-card p-4 sm:grid-cols-[4rem_minmax(0,1fr)_auto] sm:items-end">
                <div>
                  <strong className="text-sm">{currency}</strong>
                  <span className="mt-1 block text-xs text-muted-foreground">{profile ? `v${profile.revision}` : t("marketBilling.profile.notSet")}</span>
                </div>
                <label className="grid min-w-0 gap-1 text-xs leading-4 text-muted-foreground">
                  {t("marketBilling.profile.grace")}
                  <input
                    value={draft.graceHours}
                    onChange={(event) => {
                      setProfiles((current) => ({ ...current, [currency]: { ...current[currency], graceHours: event.target.value } }));
                      setProfileDirty((current) => ({ ...current, [currency]: true }));
                    }}
                    inputMode="numeric"
                    className="h-9 w-full min-w-0 rounded-md border border-border bg-white px-3 text-sm text-foreground outline-none focus:ring-2 focus:ring-primary/20"
                  />
                </label>
                <Button size="sm" variant="primary" isDisabled={!!busy} onClick={() => void saveProfile(currency)}>
                  {busy === `profile:${currency}` ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("common.save")}
                </Button>
              </div>
            );
          })}
        </div>
      </section>

      <section className="grid gap-3">
        <div>
          <h3 className="text-base font-semibold">{t("marketBilling.payables.title")}</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("marketBilling.payables.hint")}</p>
        </div>
        {buyerAccounts.length ? buyerAccounts.map((account) => (
          <CreditAccountPanel key={`buyer:${account.id}`} account={account} perspective="buyer" onAction={openAction} />
        )) : <p className="rounded-lg border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">{t("marketBilling.payables.empty")}</p>}
      </section>

      <section className="grid gap-3">
        <div>
          <h3 className="text-base font-semibold">{t("marketBilling.receivables.title")}</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("marketBilling.receivables.hint")}</p>
        </div>
        {supplierAccounts.length ? supplierAccounts.map((account) => (
          <CreditAccountPanel key={`supplier:${account.id}`} account={account} perspective="supplier" onAction={openAction} />
        )) : <p className="rounded-lg border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">{t("marketBilling.receivables.empty")}</p>}
      </section>

      {isAdmin ? (
        <section className="grid gap-3 border-t border-border pt-6">
          <div className="flex items-start gap-2">
            <Scale className="mt-0.5 h-4 w-4 text-muted-foreground" />
            <div>
              <h3 className="text-base font-semibold">{t("marketBilling.admin.title")}</h3>
              <p className="mt-0.5 text-sm text-muted-foreground">{t("marketBilling.admin.hint")}</p>
            </div>
          </div>
          {adminDisputes.length ? adminDisputes.map((item) => (
            <div key={item.dispute.id} className="grid gap-3 rounded-lg border border-sky-200 bg-sky-50/60 p-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <strong className="text-sm">{formatMoney(item.invoice.amountMinor, item.invoice.currency, locale)}</strong>
                  <p className="mt-1 break-all text-xs text-muted-foreground">{item.buyerEmail} → {item.supplierEmail}</p>
                </div>
                <span className="text-xs text-muted-foreground">{formatDate(item.dispute.createdAt, locale)}</span>
              </div>
              <p className="text-sm leading-6 text-sky-950">{item.dispute.reason}</p>
              <div className="flex flex-wrap justify-end gap-2">
                <Button size="sm" variant="outline" onClick={() => openAction({ kind: "admin-uphold", dispute: item })}>
                  <Check className="h-4 w-4" />{t("marketBilling.admin.uphold")}
                </Button>
                <Button size="sm" variant="outline" onClick={() => openAction({ kind: "admin-void", dispute: item })}>
                  <Scale className="h-4 w-4" />{t("marketBilling.admin.voidDispute")}
                </Button>
                <Button size="sm" variant="outline" className="text-rose-700" onClick={() => openAction({ kind: "admin-invoice-void", dispute: item })}>
                  <Ban className="h-4 w-4" />{t("marketBilling.admin.voidInvoice")}
                </Button>
              </div>
            </div>
          )) : <p className="rounded-lg border border-dashed border-border px-4 py-8 text-center text-sm text-muted-foreground">{t("marketBilling.admin.empty")}</p>}
        </section>
      ) : null}

      <Modal.Backdrop isOpen={!!action} onOpenChange={(open) => !open && !busy && setAction(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{action ? t(`marketBilling.dialog.${action.kind}` as Parameters<typeof t>[0]) : ""}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[70vh] gap-4 overflow-y-auto">
              {action ? (
                <p className="rounded-md border border-sky-200 bg-sky-50 px-3 py-2 text-xs leading-5 text-sky-900">
                  {t("chat.marketPublicNotice")}
                </p>
              ) : null}
              {actionInvoice ? (
                <div className="flex items-center justify-between gap-3 rounded-md border border-border bg-slate-50 p-3 text-sm">
                  <span className="text-muted-foreground">{t("marketBilling.invoice.total")}</span>
                  <strong>{formatMoney(actionInvoice.amountMinor, actionInvoice.currency, locale)}</strong>
                </div>
              ) : null}
              {actionInvoice?.declaration ? (
                <div className="rounded-md border border-border p-3">
                  <PaymentDeclarationDetails declaration={actionInvoice.declaration} locale={locale} />
                </div>
              ) : null}
              {action?.kind === "declare" ? (
                <>
                  <ProviderContactsList contacts={action.invoice.contacts} />
                  <ProviderPaymentMethodsList paymentMethods={action.invoice.paymentMethods} />
                  <p className="text-xs text-muted-foreground">
                    {t("marketBilling.paymentSnapshot", { date: formatDate(action.invoice.paymentProfileUpdatedAt, locale) })}
                  </p>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("marketBilling.declare.method")}</span>
                    <select value={paymentKind} onChange={(event) => setPaymentKind(event.target.value)} className="h-10 rounded-md border border-border bg-white px-3">
                      <option value="">{t("marketBilling.declare.methodOther")}</option>
                      {action.invoice.paymentMethods.map((method, index) => <option key={`${method.kind}:${index}`} value={method.kind}>{method.kind}</option>)}
                    </select>
                  </label>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("marketBilling.declare.reference")}</span>
                    <input value={reference} onChange={(event) => setReference(event.target.value)} maxLength={200} className="h-10 rounded-md border border-border px-3" />
                  </label>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("marketBilling.declare.evidence")}</span>
                    <input value={evidenceUrl} onChange={(event) => setEvidenceUrl(event.target.value)} maxLength={2000} placeholder="https://" className="h-10 rounded-md border border-border px-3" />
                  </label>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("marketBilling.note")}</span>
                    <textarea value={note} onChange={(event) => setNote(event.target.value)} maxLength={2000} rows={3} className="rounded-md border border-border p-3" />
                  </label>
                  <p className="text-xs leading-5 text-muted-foreground">{t("marketBilling.declare.notice")}</p>
                </>
              ) : null}
              {needsReason ? (
                <label className="grid gap-1 text-sm">
                  <span className="text-muted-foreground">{t("marketBilling.reason")}</span>
                  <textarea value={reason} onChange={(event) => setReason(event.target.value)} maxLength={2000} rows={4} className="rounded-md border border-border p-3" autoFocus />
                </label>
              ) : null}
              {needsAdminNote ? (
                <label className="grid gap-1 text-sm">
                  <span className="text-muted-foreground">{t("marketBilling.admin.note")}</span>
                  <textarea value={note} onChange={(event) => setNote(event.target.value)} maxLength={2000} rows={4} className="rounded-md border border-border p-3" />
                </label>
              ) : null}
              {action && ["settle", "request-settlement", "close", "confirm"].includes(action.kind) ? (
                <div className={`flex gap-3 rounded-md border p-3 text-sm leading-6 ${action.kind === "close" ? "border-rose-200 bg-rose-50 text-rose-900" : "border-amber-200 bg-amber-50 text-amber-950"}`}>
                  {action.kind === "close" ? <AlertTriangle className="mt-1 h-4 w-4 shrink-0" /> : <Clock3 className="mt-1 h-4 w-4 shrink-0" />}
                  <span>{t(`marketBilling.dialog.${action.kind}.description` as Parameters<typeof t>[0])}</span>
                </div>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!!busy} onClick={() => setAction(null)}>{t("common.cancel")}</Button>
              <Button variant="primary" isDisabled={!!busy} onClick={() => void submitAction()}>
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("common.verify")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </div>
  );
}
