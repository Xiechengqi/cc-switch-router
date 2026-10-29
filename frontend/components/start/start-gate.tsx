"use client";

import Image from "next/image";
import { usePathname, useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { StartPage } from "@/components/start/start-page";
import { homeEntryStatus } from "@/lib/auth";
import { resolveHomeGate, type HomeEntryStatus } from "@/lib/home-entry";

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
  React.useEffect(() => {
    setAddress({ pathname, search: window.location.search });
  }, [pathname]);
  const authed = !!session?.authenticated;
  const actor = session?.user?.id || "";
  const home = homeRead?.actor === actor ? homeRead.status : null;
  const decision = resolveHomeGate({ pathname, search: address?.pathname === pathname ? address.search : null, authed, loading, home });

  React.useEffect(() => {
    if (!decision.needsHome) return;
    let active = true;
    setHomeRead(null);
    homeEntryStatus()
      .then((status) => { if (active) setHomeRead({ actor, status }); })
      .catch(() => { if (active) setHomeRead({ actor, status: "failed" }); });
    return () => { active = false; };
  }, [actor, decision.needsHome]);

  React.useEffect(() => {
    if (decision.redirect) router.replace(decision.redirect);
  }, [decision.redirect, router]);

  if (decision.screen === "loading") return <LoadingScreen />;
  if (decision.screen === "start") return <StartPage />;
  return children;
}
