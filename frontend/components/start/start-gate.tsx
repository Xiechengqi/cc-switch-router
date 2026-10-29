"use client";

import Image from "next/image";
import { usePathname, useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { StartPage } from "@/components/start/start-page";
import { homeEntryStatus } from "@/lib/auth";
import { rememberHomeEntryStatus, resolveHomeGate, settledHomeEntryStatus, type HomeEntryStatus } from "@/lib/home-entry";

function LoadingScreen() {
  const { t } = useLocaleText();
  return (
    <main className="grid min-h-dvh place-items-center px-4" aria-busy="true">
      <div className="grid justify-items-center gap-3 text-center" role="status" aria-live="polite">
        <Image src="/router-logo.svg" alt="" width={40} height={40} priority />
        <div className="flex items-center gap-2 text-sm font-medium text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          {t("common.loading")}
        </div>
      </div>
    </main>
  );
}

export function StartGate({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "/";
  const router = useRouter();
  const { session, loading } = useAuth();
  const [homeRead, setHomeRead] = React.useState<{ actor: string; status: HomeEntryStatus | "failed" } | null>(null);
  const [address, setAddress] = React.useState<{ pathname: string; search: string } | null>(null);
  const [shellShown, setShellShown] = React.useState(false);
  React.useEffect(() => {
    setAddress({ pathname, search: window.location.search });
  }, [pathname]);
  const authed = !!session?.authenticated;
  const actor = session?.user?.id || "";
  const settled = settledHomeEntryStatus(actor);
  const home = settled ?? (homeRead?.actor === actor ? homeRead.status : null);
  const decision = resolveHomeGate({ pathname, search: address?.pathname === pathname ? address.search : null, authed, loading, home });

  React.useEffect(() => {
    // An unsettled answer (first use, or a failed read) belongs to one gated
    // visit: drop it on leaving, so the next visit asks again instead of
    // rendering an out-of-date start page. The shell stays up meanwhile.
    if (!decision.needsHome) {
      setHomeRead(null);
      return;
    }
    if (settled) return;
    let active = true;
    homeEntryStatus()
      .then((status) => {
        rememberHomeEntryStatus(actor, status);
        if (active) setHomeRead({ actor, status });
      })
      .catch(() => { if (active) setHomeRead({ actor, status: "failed" }); });
    return () => { active = false; };
  }, [actor, decision.needsHome, settled]);

  React.useEffect(() => {
    if (decision.redirect) router.replace(decision.redirect);
  }, [decision.redirect, router]);

  // This gate wraps the whole dashboard shell. Once the shell is up, a
  // pending answer (the address of a soft navigation is read one frame late,
  // or the home status is being re-read) keeps it mounted; replacing it with
  // a loading screen would remount every provider and refetch every panel.
  const showContent = decision.screen === "content"
    || (shellShown && decision.screen === "loading" && !decision.redirect);
  React.useEffect(() => {
    setShellShown(showContent);
  }, [showContent]);

  if (showContent) return children;
  if (decision.screen === "loading") return <LoadingScreen />;
  return <StartPage onSaved={(status) => setHomeRead({ actor, status })} />;
}
