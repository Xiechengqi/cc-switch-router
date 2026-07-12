"use client";

import { Alert } from "@heroui/react";
import { BoardDock } from "@/components/board/board-dock";
import { ClientBoard } from "@/components/dashboard/client-board";
import { MarketsTable, PresenceFooter } from "@/components/dashboard/data-tables";
import { LiveMap } from "@/components/dashboard/live-map";
import { DashboardFocusProvider } from "@/components/dashboard/dashboard-focus";
import { FocusBar } from "@/components/dashboard/focus-bar";
import { OperationVerificationProvider } from "@/components/dashboard/operation-verification";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import { StatsStrip } from "@/components/dashboard/stats-strip";
import { AttentionPanel } from "@/components/dashboard/attention-panel";
import { DashboardViewStateProvider } from "@/components/dashboard/dashboard-view-state";

export function DashboardPage() {
  const { data, error, refresh } = useDashboardData();

  return (
    <DashboardFocusProvider data={data}>
      <DashboardViewStateProvider>
        <OperationVerificationProvider data={data}>
          <main className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-6">
          {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
          <StatsStrip data={data} />
          <LiveMap data={data} />
          <FocusBar />
          <AttentionPanel data={data} />
          <ClientBoard
            clients={data?.clients || []}
            shares={data?.shares || []}
            markets={data?.markets || []}
            onChanged={refresh}
          />
          <MarketsTable markets={data?.markets || []} onChanged={refresh} />
          </main>
          <PresenceFooter />
          <BoardDock />
        </OperationVerificationProvider>
      </DashboardViewStateProvider>
    </DashboardFocusProvider>
  );
}
