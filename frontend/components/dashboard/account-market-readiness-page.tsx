"use client";

import Link from "next/link";
import * as React from "react";
import { Chip } from "@heroui/react";
import {
  ArrowRight,
  CircleAlert,
  ClipboardCheck,
  HandCoins,
  Loader2,
  RefreshCw,
  ShieldCheck,
  WalletCards,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getAccountPaymentProfile,
  getMarketAccessDashboard,
  getMarketBillingDashboard,
} from "@/lib/api";
import {
  DASHBOARD_ACCOUNT_BILLING_PATH,
  DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH,
  DASHBOARD_ACCOUNT_PAYMENTS_PATH,
} from "@/lib/dashboard-nav";
import type {
  AccountPaymentProfile,
  MarketAccessDashboard,
  MarketAccessPolicy,
  MarketBillingDashboard,
} from "@/lib/types";

const ACTION_LINK_CLASS = "inline-flex h-9 shrink-0 items-center gap-2 rounded-md border border-border bg-white px-3 text-sm font-medium text-foreground transition-colors hover:bg-slate-50";

function productLabel(policy: MarketAccessPolicy, t: ReturnType<typeof useLocaleText>["t"]) {
  return policy.productKind === "share"
    ? t("marketAccess.product.share")
    : t("marketAccess.product.clientHost");
}

function pricingLabel(policy: MarketAccessPolicy, t: ReturnType<typeof useLocaleText>["t"]) {
  return policy.pricingKind === "free"
    ? t("marketAccess.pricing.free")
    : t("marketAccess.pricing.paid");
}

export function AccountMarketReadinessPage() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;
  const [payment, setPayment] = React.useState<AccountPaymentProfile | null>(null);
  const [access, setAccess] = React.useState<MarketAccessDashboard | null>(null);
  const [billing, setBilling] = React.useState<MarketBillingDashboard | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [error, setError] = React.useState("");

  const load = React.useCallback(async (silent = false) => {
    if (!authed) return;
    if (silent) setRefreshing(true);
    else setLoading(true);
    try {
      const [nextPayment, nextAccess, nextBilling] = await Promise.all([
        getAccountPaymentProfile(),
        getMarketAccessDashboard(),
        getMarketBillingDashboard(),
      ]);
      setPayment(nextPayment);
      setAccess(nextAccess);
      setBilling(nextBilling);
      setError("");
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  }, [authed]);

  React.useEffect(() => {
    if (!authed) {
      setLoading(false);
      return;
    }
    void load();
  }, [authed, load]);

  if (authLoading || loading) {
    return <div className="flex items-center gap-2 py-8 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</div>;
  }
  if (!authed) return <p className="py-8 text-sm text-muted-foreground">{t("account.signInRequired")}</p>;

  const paymentReady = (payment?.methods.length || 0) > 0;
  const pendingRequests = access?.accessRequests.filter((request) => request.status === "requested").length || 0;
  const activeBuyers = access?.counterparties.filter((counterparty) => counterparty.status === "active").length || 0;
  const payableCount = billing?.accounts.filter((account) => account.isBuyer && ["open", "overdue"].includes(account.openInvoice?.status || "")).length || 0;
  const reviewCount = billing?.accounts.filter((account) => account.isSupplier && account.openInvoice?.status === "payment_declared").length || 0;
  const overdueCount = billing?.accounts.filter((account) => account.openInvoice?.status === "overdue").length || 0;
  const hasSnapshot = payment != null && access != null && billing != null;
  const policies = [...(access?.policies || [])].sort((left, right) => (
    `${left.productKind}:${left.pricingKind}`.localeCompare(`${right.productKind}:${right.pricingKind}`)
  ));

  return (
    <div className="grid min-w-0 gap-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <div className="flex items-center gap-2">
            <ClipboardCheck className="h-5 w-5 text-muted-foreground" />
            <h2 className="text-lg font-semibold text-foreground">{t("marketReadiness.title")}</h2>
          </div>
          <p className="mt-1 max-w-3xl text-sm text-muted-foreground">{t("marketReadiness.subtitle")}</p>
        </div>
        <button
          type="button"
          className="inline-flex h-9 w-9 items-center justify-center rounded-md border border-border bg-white hover:bg-slate-50 disabled:opacity-50"
          aria-label={t("common.reload")}
          disabled={refreshing}
          onClick={() => void load(true)}
        >
          <RefreshCw className={`h-4 w-4 ${refreshing ? "animate-spin" : ""}`} />
        </button>
      </div>

      {error ? (
        <div className="flex gap-3 rounded-md border border-rose-200 bg-rose-50 p-3 text-sm text-rose-800">
          <CircleAlert className="mt-0.5 h-4 w-4 shrink-0" />
          <span className="min-w-0 break-words">{error}</span>
        </div>
      ) : null}

      {hasSnapshot ? <><section className="grid grid-cols-2 gap-x-5 gap-y-4 border-y border-border py-4 sm:grid-cols-4">
        {[
          [t("marketReadiness.summary.paymentMethods"), payment?.methods.length || 0],
          [t("marketReadiness.summary.pendingAccess"), pendingRequests],
          [t("marketReadiness.summary.billingActions"), payableCount + reviewCount],
          [t("marketReadiness.summary.overdue"), overdueCount],
        ].map(([label, value]) => (
          <div key={String(label)} className="min-w-0">
            <span className="block truncate text-xs text-muted-foreground">{label}</span>
            <strong className="mt-0.5 block text-xl tabular-nums">{value}</strong>
          </div>
        ))}
      </section>

      <section className="divide-y divide-border border-y border-border">
        <div className="flex flex-wrap items-center gap-4 py-4">
          <WalletCards className="h-5 w-5 shrink-0 text-muted-foreground" />
          <div className="min-w-[12rem] flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <strong className="text-sm">{t("marketReadiness.payment.title")}</strong>
              <Chip size="sm" variant="soft" className={paymentReady ? "bg-emerald-100 text-emerald-700" : "bg-amber-100 text-amber-800"}>
                {paymentReady ? t("marketReadiness.ready") : t("marketReadiness.needsSetup")}
              </Chip>
            </div>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("marketReadiness.payment.detail", {
              methods: payment?.methods.length || 0,
              contacts: payment?.contacts?.length || 0,
            })}</p>
          </div>
          <Link href={DASHBOARD_ACCOUNT_PAYMENTS_PATH} className={ACTION_LINK_CLASS}>{t("marketReadiness.payment.action")}<ArrowRight className="h-4 w-4" /></Link>
        </div>

        <div className="flex flex-wrap items-center gap-4 py-4">
          <ShieldCheck className="h-5 w-5 shrink-0 text-muted-foreground" />
          <div className="min-w-[12rem] flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <strong className="text-sm">{t("marketReadiness.access.title")}</strong>
              {pendingRequests ? <Chip size="sm" variant="soft" className="bg-rose-100 text-rose-700">{t("marketReadiness.pending", { count: pendingRequests })}</Chip> : null}
            </div>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("marketReadiness.access.detail", { buyers: activeBuyers })}</p>
          </div>
          <Link href={DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH} className={ACTION_LINK_CLASS}>{t("marketReadiness.access.action")}<ArrowRight className="h-4 w-4" /></Link>
        </div>

        <div className="flex flex-wrap items-center gap-4 py-4">
          <HandCoins className="h-5 w-5 shrink-0 text-muted-foreground" />
          <div className="min-w-[12rem] flex-1">
            <strong className="text-sm">{t("marketReadiness.billing.title")}</strong>
            <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("marketReadiness.billing.detail", {
              payables: payableCount,
              reviews: reviewCount,
              overdue: overdueCount,
            })}</p>
          </div>
          <Link href={DASHBOARD_ACCOUNT_BILLING_PATH} className={ACTION_LINK_CLASS}>{t("marketReadiness.billing.action")}<ArrowRight className="h-4 w-4" /></Link>
        </div>
      </section>

      <section className="grid gap-3">
        <div>
          <h3 className="text-base font-semibold">{t("marketReadiness.policies.title")}</h3>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("marketReadiness.policies.hint")}</p>
        </div>
        <div className="overflow-x-auto rounded-lg border border-border">
          <table className="w-full min-w-[34rem] text-left text-sm">
            <thead className="bg-slate-50 text-xs text-muted-foreground">
              <tr>
                <th className="px-4 py-2.5 font-medium">{t("marketReadiness.policies.product")}</th>
                <th className="px-4 py-2.5 font-medium">{t("marketReadiness.policies.pricing")}</th>
                <th className="px-4 py-2.5 font-medium">{t("marketReadiness.policies.mode")}</th>
                <th className="px-4 py-2.5 text-right font-medium">{t("marketReadiness.policies.revision")}</th>
              </tr>
            </thead>
            <tbody>
              {policies.map((policy) => (
                <tr key={`${policy.productKind}:${policy.pricingKind}`} className="border-t border-border">
                  <td className="px-4 py-3 font-medium">{productLabel(policy, t)}</td>
                  <td className="px-4 py-3 text-muted-foreground">{pricingLabel(policy, t)}</td>
                  <td className="px-4 py-3">
                    <Chip size="sm" variant="soft" className={policy.mode === "whitelist" ? "bg-sky-100 text-sky-700" : "bg-amber-100 text-amber-800"}>
                      {t(policy.mode === "whitelist" ? "marketAccess.mode.whitelist" : "marketAccess.mode.blacklist")}
                    </Chip>
                  </td>
                  <td className="px-4 py-3 text-right font-mono text-xs text-muted-foreground">v{policy.revision}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section></> : null}
    </div>
  );
}
