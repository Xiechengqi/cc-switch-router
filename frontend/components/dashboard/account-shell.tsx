"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { Activity, BarChart3, KeyRound, Receipt, WalletCards } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  DASHBOARD_ACCOUNT_API_KEYS_PATH,
  DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH,
  DASHBOARD_ACCOUNT_PAYMENTS_PATH,
  DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH,
  DASHBOARD_ACCOUNT_RENTALS_PATH,
} from "@/lib/dashboard-nav";
import { cn } from "@/lib/utils";

const NAV_ITEMS = [
  {
    id: "api-keys",
    href: DASHBOARD_ACCOUNT_API_KEYS_PATH,
    labelKey: "account.nav.apiKeys" as const,
    icon: KeyRound,
    match: (pathname: string) =>
      pathname.startsWith("/account/api-keys") || pathname === "/account" || pathname === "/account/",
  },
  {
    id: "provider-usage",
    href: DASHBOARD_ACCOUNT_PROVIDER_USAGE_PATH,
    labelKey: "account.nav.providerUsage" as const,
    icon: BarChart3,
    match: (pathname: string) => pathname.startsWith("/account/provider-usage"),
  },
  {
    id: "consumer-usage",
    href: DASHBOARD_ACCOUNT_CONSUMER_USAGE_PATH,
    labelKey: "account.nav.consumerUsage" as const,
    icon: Activity,
    match: (pathname: string) => pathname.startsWith("/account/consumer-usage"),
  },
  {
    id: "payments",
    href: DASHBOARD_ACCOUNT_PAYMENTS_PATH,
    labelKey: "account.nav.payments" as const,
    icon: WalletCards,
    match: (pathname: string) => pathname.startsWith("/account/payments"),
  },
  {
    id: "rentals",
    href: DASHBOARD_ACCOUNT_RENTALS_PATH,
    labelKey: "account.nav.rentals" as const,
    icon: Receipt,
    match: (pathname: string) => pathname.startsWith("/account/rentals"),
  },
];

export function AccountShell({ children }: { children: React.ReactNode }) {
  const { t } = useLocaleText();
  const pathname = usePathname() || DASHBOARD_ACCOUNT_API_KEYS_PATH;

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-6xl grid-cols-[minmax(0,1fr)] gap-6 pb-12 pt-2 md:grid-cols-[13rem_minmax(0,1fr)] md:gap-8">
      <nav
        aria-label={t("account.nav.sections")}
        className="min-w-0 md:sticky md:top-4 md:self-start"
      >
        <div className="flex gap-1 overflow-x-auto rounded-lg bg-slate-100/90 p-1 ring-1 ring-inset ring-slate-200/80 md:grid md:grid-cols-1 md:overflow-visible">
          {NAV_ITEMS.map((item) => {
            const active = item.match(pathname);
            const Icon = item.icon;
            return (
              <Link
                key={item.id}
                href={item.href}
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
              </Link>
            );
          })}
        </div>
      </nav>

      <div className="min-w-0">{children}</div>
    </main>
  );
}
