"use client";

import Image from "next/image";
import { useRouter } from "next/navigation";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { SharePage } from "@/components/share/share-page";
import { buildDashboardHref, defaultDashboardRouteFromSearch } from "@/lib/dashboard-nav";
import { getShareContext } from "@/lib/share-api";

export function RootPage() {
  const router = useRouter();
  const { t } = useLocaleText();
  const [mode, setMode] = React.useState<"loading" | "dashboard" | "share">("loading");

  React.useEffect(() => {
    let active = true;
    getShareContext()
      .then(() => {
        if (active) setMode("share");
      })
      .catch(() => {
        if (active) setMode("dashboard");
      });
    return () => {
      active = false;
    };
  }, []);

  React.useEffect(() => {
    if (mode !== "dashboard") return;
    const search = window.location.search;
    router.replace(buildDashboardHref(defaultDashboardRouteFromSearch(search), search));
  }, [mode, router]);

  if (mode === "loading" || mode === "dashboard") {
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
  return <SharePage />;
}
