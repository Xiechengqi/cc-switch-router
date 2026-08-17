"use client";

import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { Activity, BarChart3, Bell, ClipboardCheck, HandCoins, KeyRound, Receipt, Share2, ShieldCheck, WalletCards } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketAccessInboxSummary } from "@/lib/api";
import {
  accountNavHrefFromPathname,
  DASHBOARD_ACCOUNT_API_KEYS_PATH,
  DASHBOARD_ACCOUNT_CLIENT_PATH,
  DASHBOARD_ACCOUNT_BILLING_PATH,
  DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH,
  DASHBOARD_ACCOUNT_MARKET_READINESS_PATH,
  DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH,
  DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH,
  DASHBOARD_ACCOUNT_PATH,
  DASHBOARD_ACCOUNT_PAYMENTS_PATH,
  DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH,
  DASHBOARD_ACCOUNT_SHARE_PATH,
  resolveAccountEntryHref,
  writeStoredAccountNavHref,
  type AccountNavHref,
} from "@/lib/dashboard-nav";
import { cn } from "@/lib/utils";

const NAV_ITEMS: {
  id: string;
  href: AccountNavHref;
  groupLabelKey:
    | "account.nav.group.security"
    | "account.nav.group.usage"
    | "account.nav.group.market";
  labelKey:
    | "account.nav.apiKeys"
    | "account.nav.notifications"
    | "account.nav.providerUsage"
    | "account.nav.consumerUsage"
    | "account.nav.billing"
    | "account.nav.marketReadiness"
    | "account.nav.marketAccess"
    | "account.nav.payments"
    | "account.nav.share"
    | "account.nav.client";
  icon: typeof KeyRound;
  match: (pathname: string) => boolean;
}[] = [
  {
    id: "api-keys",
    href: DASHBOARD_ACCOUNT_API_KEYS_PATH,
    groupLabelKey: "account.nav.group.security",
    labelKey: "account.nav.apiKeys",
    icon: KeyRound,
    match: (pathname: string) =>
      pathname.startsWith("/account/api-keys") || pathname === "/account" || pathname === "/account/",
  },
  {
    id: "notifications",
    href: DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH,
    groupLabelKey: "account.nav.group.security",
    labelKey: "account.nav.notifications",
    icon: Bell,
    match: (pathname: string) => pathname.startsWith("/account/notifications"),
  },
  {
    id: "provider-usage",
    href: DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH,
    groupLabelKey: "account.nav.group.usage",
    labelKey: "account.nav.providerUsage",
    icon: BarChart3,
    match: (pathname: string) => pathname.startsWith("/account/provider-usage"),
  },
  {
    id: "consumer-usage",
    href: DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH,
    groupLabelKey: "account.nav.group.usage",
    labelKey: "account.nav.consumerUsage",
    icon: Activity,
    match: (pathname: string) => pathname.startsWith("/account/consumer-usage"),
  },
  {
    id: "market-readiness",
    href: DASHBOARD_ACCOUNT_MARKET_READINESS_PATH,
    groupLabelKey: "account.nav.group.market",
    labelKey: "account.nav.marketReadiness",
    icon: ClipboardCheck,
    match: (pathname: string) => pathname.startsWith("/account/market-readiness"),
  },
  {
    id: "market-access",
    href: DASHBOARD_ACCOUNT_MARKET_ACCESS_PATH,
    groupLabelKey: "account.nav.group.market",
    labelKey: "account.nav.marketAccess",
    icon: ShieldCheck,
    match: (pathname: string) => pathname.startsWith("/account/market-access"),
  },
  {
    id: "payments",
    href: DASHBOARD_ACCOUNT_PAYMENTS_PATH,
    groupLabelKey: "account.nav.group.market",
    labelKey: "account.nav.payments",
    icon: WalletCards,
    match: (pathname: string) => pathname.startsWith("/account/payments"),
  },
  {
    id: "billing",
    href: DASHBOARD_ACCOUNT_BILLING_PATH,
    groupLabelKey: "account.nav.group.market",
    labelKey: "account.nav.billing",
    icon: HandCoins,
    match: (pathname: string) => pathname.startsWith("/account/billing"),
  },
  {
    id: "share",
    href: DASHBOARD_ACCOUNT_SHARE_PATH,
    groupLabelKey: "account.nav.group.market",
    labelKey: "account.nav.share",
    icon: Share2,
    match: (pathname: string) => pathname.startsWith("/account/share"),
  },
  {
    id: "client",
    href: DASHBOARD_ACCOUNT_CLIENT_PATH,
    groupLabelKey: "account.nav.group.market",
    labelKey: "account.nav.client",
    icon: Receipt,
    match: (pathname: string) =>
      pathname.startsWith("/account/client") || pathname.startsWith("/account/rentals"),
  },
];

function isAccountIndexPath(pathname: string) {
  return pathname === "/account" || pathname === "/account/" || pathname === DASHBOARD_ACCOUNT_PATH;
}

export function AccountShell({ children }: { children: React.ReactNode }) {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const pathname = usePathname() || DASHBOARD_ACCOUNT_API_KEYS_PATH;
  const router = useRouter();
  const [pendingAccessRequests, setPendingAccessRequests] = React.useState(0);
  const authed = !!session?.authenticated;
  const activeItem = NAV_ITEMS.find((item) => item.match(pathname));

  React.useEffect(() => {
    if (isAccountIndexPath(pathname)) {
      router.replace(resolveAccountEntryHref());
      return;
    }
    const href = accountNavHrefFromPathname(pathname);
    if (href) writeStoredAccountNavHref(href);
  }, [pathname, router]);

  React.useEffect(() => {
    if (!authed) {
      setPendingAccessRequests(0);
      return;
    }
    let active = true;
    const loadPending = () => void getMarketAccessInboxSummary()
      .then((summary) => { if (active) setPendingAccessRequests(summary.pendingRequests); })
      .catch(() => undefined);
    loadPending();
    const timer = window.setInterval(loadPending, 30_000);
    return () => {
      active = false;
      window.clearInterval(timer);
    };
  }, [authed, pathname]);

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-6xl grid-cols-[minmax(0,1fr)] gap-6 pb-12 pt-2 md:grid-cols-[13rem_minmax(0,1fr)] md:gap-8">
      <nav
        aria-label={t("account.nav.sections")}
        className="min-w-0 md:sticky md:top-4 md:self-start"
      >
        <div className="flex gap-1 overflow-x-auto rounded-lg bg-slate-100/90 p-1 ring-1 ring-inset ring-slate-200/80 md:grid md:grid-cols-1 md:overflow-visible">
          {NAV_ITEMS.map((item, index) => {
            const active = item.match(pathname);
            const Icon = item.icon;
            return (
              <React.Fragment key={item.id}>
                {index === 0 || NAV_ITEMS[index - 1].groupLabelKey !== item.groupLabelKey ? (
                  <div className="hidden px-3 pb-1 pt-3 text-xs font-semibold text-slate-500 first:pt-1 md:block">
                    {t(item.groupLabelKey)}
                  </div>
                ) : null}
                <Link
                  href={item.href}
                  onClick={() => writeStoredAccountNavHref(item.href)}
                  className={cn(
                    "inline-flex h-9 shrink-0 items-center gap-2 rounded-md px-3 text-sm font-medium transition-colors",
                    active
                      ? "bg-white text-foreground shadow-sm"
                      : "text-muted-foreground hover:text-foreground",
                  )}
                  aria-current={active ? "page" : undefined}
                >
                  <Icon className="h-4 w-4 shrink-0" aria-hidden />
                  <span className="whitespace-nowrap">{t(item.labelKey)}</span>
                  {(item.id === "market-readiness" || item.id === "market-access") && pendingAccessRequests > 0 ? (
                    <span className="inline-flex min-w-5 items-center justify-center rounded-full bg-rose-100 px-1.5 text-[10px] font-semibold text-rose-700">
                      {pendingAccessRequests > 99 ? "99+" : pendingAccessRequests}
                    </span>
                  ) : null}
                </Link>
              </React.Fragment>
            );
          })}
        </div>
      </nav>

      <div className="min-w-0">
        <h1 className="sr-only">{activeItem ? t(activeItem.labelKey) : t("account.nav.sections")}</h1>
        {children}
      </div>
    </main>
  );
}
