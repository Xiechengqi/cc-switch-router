"use client";

import { Alert } from "@heroui/react";
import { ClientBoard } from "@/components/dashboard/client-board";
import { LiveMap } from "@/components/dashboard/live-map";
import { useDashboardData } from "@/components/dashboard/dashboard-data";

export function ClientsPage() {
  const { data, error, refresh } = useDashboardData();

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-6">
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      <LiveMap data={data} />
      <ClientBoard
        clients={data?.clients || []}
        shares={data?.shares || []}
        markets={data?.markets || []}
        onChanged={refresh}
      />
    </main>
  );
}
