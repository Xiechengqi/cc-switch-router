"use client";

import { Alert } from "@heroui/react";
import { MarketsTable } from "@/components/dashboard/markets-table";
import { useDashboardData } from "@/components/dashboard/dashboard-data";

export function MarketsPage() {
  const { data, error, refresh } = useDashboardData();

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-6">
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      <MarketsTable markets={data?.markets || []} onChanged={refresh} />
    </main>
  );
}
