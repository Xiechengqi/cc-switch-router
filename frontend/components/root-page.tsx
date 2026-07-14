"use client";

import { useRouter } from "next/navigation";
import * as React from "react";
import { SharePage } from "@/components/share/share-page";
import { buildDashboardHref, defaultDashboardRouteFromSearch } from "@/lib/dashboard-nav";
import { getShareContext } from "@/lib/share-api";

export function RootPage() {
  const router = useRouter();
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

  if (mode === "loading" || mode === "dashboard") return null;
  return <SharePage />;
}
