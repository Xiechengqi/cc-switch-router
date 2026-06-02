"use client";

import * as React from "react";
import { DashboardPage } from "@/components/dashboard/dashboard-page";
import { AppShell } from "@/components/layout/app-shell";
import { SharePage } from "@/components/share/share-page";
import { getShareContext } from "@/lib/share-api";

export function RootPage() {
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

  if (mode === "loading") return null;
  if (mode === "share") return <SharePage />;
  return (
    <AppShell active="dashboard">
      <DashboardPage />
    </AppShell>
  );
}
