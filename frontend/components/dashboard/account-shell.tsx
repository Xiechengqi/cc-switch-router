"use client";

import Link from "next/link";
import { usePathname, useRouter } from "next/navigation";
import { BarChart3, Bell, Receipt, ShieldCheck, Store, WalletCards } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  accountNavHrefFromPathname,
  DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH,
  DASHBOARD_ACCOUNT_PATH,
  DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH,
  DASHBOARD_ACCOUNT_RENTALS_PATH,
  DASHBOARD_ACCOUNT_SECURITY_PATH,
  DASHBOARD_ACCOUNT_USAGE_PATH,
  DASHBOARD_ACCOUNT_WALLET_PATH,
  legacyAccountHref,
  resolveAccountEntryHref,
  writeStoredAccountNavHref,
  type AccountNavHref,
} from "@/lib/dashboard-nav";
import { useMarketAccessInboxSummary } from "@/lib/use-market-access-inbox-summary";
import { cn } from "@/lib/utils";

/**
 * Money and trades first, where most visits go; account housekeeping last.
 * Retired addresses map onto these through `accountNavHrefFromPathname`.
 */
const NAV_ITEMS: {
  href: AccountNavHref;
  groupLabelKey: "account.nav.group.trading" | "account.nav.group.account";
  labelKey:
    | "account.nav.wallet"
    | "account.nav.myRentals"
    | "account.nav.providerSettings"
    | "account.nav.usage"
    | "account.nav.security"
    | "account.nav.notifications";
  icon: typeof Bell;
}[] = [
  { href: DASHBOARD_ACCOUNT_WALLET_PATH, groupLabelKey: "account.nav.group.trading", labelKey: "account.nav.wallet", icon: WalletCards },
  { href: DASHBOARD_ACCOUNT_RENTALS_PATH, groupLabelKey: "account.nav.group.trading", labelKey: "account.nav.myRentals", icon: Receipt },
  { href: DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH, groupLabelKey: "account.nav.group.trading", labelKey: "account.nav.providerSettings", icon: Store },
  { href: DASHBOARD_ACCOUNT_USAGE_PATH, groupLabelKey: "account.nav.group.account", labelKey: "account.nav.usage", icon: BarChart3 },
  { href: DASHBOARD_ACCOUNT_SECURITY_PATH, groupLabelKey: "account.nav.group.account", labelKey: "account.nav.security", icon: ShieldCheck },
  { href: DASHBOARD_ACCOUNT_NOTIFICATIONS_PATH, groupLabelKey: "account.nav.group.account", labelKey: "account.nav.notifications", icon: Bell },
];

function isAccountIndexPath(pathname: string) {
  return pathname === "/account" || pathname === "/account/" || pathname === DASHBOARD_ACCOUNT_PATH;
}

export function AccountShell({ children }: { children: React.ReactNode }) {
  const { t } = useLocaleText();
  const { session, loading } = useAuth();
  const pathname = usePathname() || DASHBOARD_ACCOUNT_PATH;
  const router = useRouter();
  const authed = !!session?.authenticated;
  const { pendingRequests: pendingAccessRequests, refresh: refreshPendingAccessRequests } = useMarketAccessInboxSummary(
    authed ? session?.user?.id || session?.user?.email : null,
  );
  const activeHref = accountNavHrefFromPathname(pathname);
  const activeItem = NAV_ITEMS.find((item) => item.href === activeHref);

  React.useEffect(() => {
    if (isAccountIndexPath(pathname)) {
      router.replace(resolveAccountEntryHref());
      return;
    }
    if (activeHref) writeStoredAccountNavHref(activeHref);
  }, [activeHref, pathname, router]);

  // Each account section re-reads the badge; the request is shared.
  React.useEffect(() => {
    void refreshPendingAccessRequests();
  }, [pathname, refreshPendingAccessRequests]);

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-6xl grid-cols-[minmax(0,1fr)] gap-6 pb-12 pt-2 md:grid-cols-[13rem_minmax(0,1fr)] md:gap-8">
      <nav
        aria-label={t("account.nav.sections")}
        className="min-w-0 md:sticky md:top-4 md:self-start"
      >
        <div className="flex gap-1 overflow-x-auto rounded-lg bg-slate-100/90 p-1 ring-1 ring-inset ring-slate-200/80 md:grid md:grid-cols-1 md:overflow-visible">
          {NAV_ITEMS.map((item, index) => {
            const active = item.href === activeHref;
            const Icon = item.icon;
            return (
              <React.Fragment key={item.href}>
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
                  {item.href === DASHBOARD_ACCOUNT_PROVIDER_SETTINGS_PATH && pendingAccessRequests > 0 ? (
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
        {/* Every section needs an account; say so once instead of in each page.
            Retired addresses still redirect first, keeping role, market and focus. */}
        {authed || legacyAccountHref(pathname) ? children : loading ? <PageSkeleton inset /> : (
          <p className="py-6 text-sm text-muted-foreground">{t("account.signInRequiredAll")}</p>
        )}
      </div>
    </main>
  );
}
