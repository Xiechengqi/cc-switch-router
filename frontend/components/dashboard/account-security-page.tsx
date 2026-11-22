"use client";

import { Button } from "@heroui/react";
import { KeyRound, LockKeyhole, ShieldCheck, WalletCards } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { AccountSectionHeader } from "@/components/dashboard/account-section";
import { ApiTokenCard } from "@/components/dashboard/api-token-card";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getWalletSecurity, type WalletSecurity } from "@/lib/wallet-security-api";

/** Account & security: how scripts and people sign in. */
export function AccountSecurityPage() {
  const { t, locale } = useLocaleText();
  const { session, openPasswordSettings } = useAuth();
  const password = session?.password;
  const [wallet, setWallet] = React.useState<WalletSecurity | null>(null);
  // Re-read after a password change: it restarts the confirmation wait.
  const passwordVersion = password?.version;
  React.useEffect(() => {
    let active = true;
    getWalletSecurity().then((value) => { if (active) setWallet(value); }).catch(() => {});
    return () => { active = false; };
  }, [passwordVersion]);
  const walletStatus = !wallet ? null
    : !wallet.hasPassword ? t("account.security.walletNoPassword")
      : wallet.immediateUsableAt ? t("account.security.walletCooling", { time: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(wallet.immediateUsableAt)) })
        : t("account.security.walletReady");

  // Preserve ordinary section anchors without a separate home-preference gate.
  React.useEffect(() => {
    const id = window.location.hash.slice(1);
    if (id) document.getElementById(id)?.scrollIntoView({ block: "start" });
  }, []);

  return (
    <div className="grid min-w-0 grid-cols-1 gap-6 [&>*]:min-w-0">
      <AccountSectionHeader icon={ShieldCheck} title={t("account.nav.security")} hint={t("account.security.hint")} />
      <ApiTokenCard />
      {password ? (
        <section aria-labelledby="sign-in-password-title" className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-xl border border-border bg-card p-4 sm:p-5">
          <div className="min-w-0">
            <h3 id="sign-in-password-title" className="flex items-center gap-2 text-sm font-semibold text-foreground">
              <LockKeyhole className="h-4 w-4 text-muted-foreground" aria-hidden />
              {t("account.security.password")}
            </h3>
            <p className="mt-0.5 text-sm text-muted-foreground">
              {t(password.hasPassword ? "account.security.passwordSet" : "account.security.passwordUnset")}
            </p>
          </div>
          <Button variant="outline" onPress={openPasswordSettings}>
            <KeyRound className="h-4 w-4" aria-hidden />
            {t(password.hasPassword ? "auth.password.changeTitle" : "auth.password.setTitle")}
          </Button>
        </section>
      ) : null}
      {walletStatus ? (
        <section id="payment-confirmation" aria-labelledby="payment-confirmation-title" className="flex min-w-0 items-start gap-3 rounded-xl border border-border bg-card p-4 sm:p-5">
          <WalletCards className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" aria-hidden />
          <div className="min-w-0">
            <h3 id="payment-confirmation-title" className="text-sm font-semibold text-foreground">{t("account.security.walletTitle")}</h3>
            <p className="mt-0.5 text-sm text-muted-foreground">{walletStatus}</p>
          </div>
        </section>
      ) : null}
    </div>
  );
}
