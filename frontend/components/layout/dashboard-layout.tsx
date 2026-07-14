"use client";

import { BoardDock } from "@/components/board/board-dock";
import { ClientConsoleDock, ClientConsoleManagerProvider, ClientConsoleWindowLayer } from "@/components/dashboard/client-console";
import { PresenceFooter } from "@/components/dashboard/data-tables";
import { DashboardFocusProvider } from "@/components/dashboard/dashboard-focus";
import { OperationVerificationProvider } from "@/components/dashboard/operation-verification";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import { DashboardViewStateProvider } from "@/components/dashboard/dashboard-view-state";

export function DashboardLayout({ children }: { children: React.ReactNode }) {
  const { data } = useDashboardData();

  return (
    <DashboardFocusProvider data={data}>
      <DashboardViewStateProvider>
        <OperationVerificationProvider data={data}>
          <ClientConsoleManagerProvider>
            {children}
            <PresenceFooter />
            <ClientConsoleWindowLayer />
            <ClientConsoleDock />
            <BoardDock />
          </ClientConsoleManagerProvider>
        </OperationVerificationProvider>
      </DashboardViewStateProvider>
    </DashboardFocusProvider>
  );
}
