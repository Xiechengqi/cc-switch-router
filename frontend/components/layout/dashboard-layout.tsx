"use client";

import { ClientChatProvider } from "@/components/chat/client-chat";
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
        <ClientChatProvider>
          <OperationVerificationProvider data={data}>
            <ClientConsoleManagerProvider>
              <div className="flex flex-1 flex-col">
                <div className="flex-1">{children}</div>
                <PresenceFooter />
              </div>
              <ClientConsoleWindowLayer />
              <ClientConsoleDock />
            </ClientConsoleManagerProvider>
          </OperationVerificationProvider>
        </ClientChatProvider>
      </DashboardViewStateProvider>
    </DashboardFocusProvider>
  );
}
