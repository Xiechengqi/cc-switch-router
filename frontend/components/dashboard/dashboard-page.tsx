"use client";

import * as React from "react";
import { Alert } from "@heroui/react";
import { BoardDock } from "@/components/board/board-dock";
import { useAuth } from "@/components/auth/auth-provider";
import { ClientsTable, MarketsTable, PresenceFooter } from "@/components/dashboard/data-tables";
import { LiveMap } from "@/components/dashboard/live-map";
import { getDashboard } from "@/lib/api";
import type { DashboardResponse } from "@/lib/types";

export function DashboardPage() {
  const [data, setData] = React.useState<DashboardResponse | null>(null);
  const [error, setError] = React.useState("");
  const { loading: authLoading, session } = useAuth();
  const requestSeq = React.useRef(0);

  const load = React.useCallback(async () => {
    const seq = ++requestSeq.current;
    try {
      const next = await getDashboard();
      if (seq === requestSeq.current) {
        setData(next);
        setError("");
      }
    } catch (err) {
      if (seq === requestSeq.current) {
        setError(err instanceof Error ? err.message : String(err));
      }
    }
  }, []);

  React.useEffect(() => {
    if (authLoading) return;
    load().catch(console.error);
    const id = window.setInterval(() => load().catch(console.error), 5000);
    return () => window.clearInterval(id);
  }, [authLoading, load, session?.authenticated, session?.user?.email]);

  return (
    <>
      <main className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-6 pb-6">
        {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
        <LiveMap data={data} />
        <ClientsTable clients={data?.clients || []} markets={data?.markets || []} onChanged={load} />
        <MarketsTable markets={data?.markets || []} onChanged={load} />
      </main>
      <PresenceFooter />
      <BoardDock />
    </>
  );
}
