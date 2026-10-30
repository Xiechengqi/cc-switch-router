"use client";

import { Button } from "@heroui/react";
import { KeyRound, LockKeyhole, ShieldCheck } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { AccountSectionHeader } from "@/components/dashboard/account-section";
import { ApiTokenCard } from "@/components/dashboard/api-token-card";
import { HomeEntryCard } from "@/components/dashboard/home-entry-card";
import { useLocaleText } from "@/components/i18n/locale-provider";

/** Account & security: how scripts and people sign in, and where they land. */
export function AccountSecurityPage() {
  const { t } = useLocaleText();
  const { session, openPasswordSettings } = useAuth();
  const password = session?.password;

  // A link to the home entry card arrives with its anchor before the card exists.
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
      <HomeEntryCard />
    </div>
  );
}
