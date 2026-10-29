"use client";

import { ClientChatProvider } from "@/components/chat/client-chat";
import { ClientConsoleDock, ClientConsoleManagerProvider, ClientConsoleWindowLayer } from "@/components/dashboard/client-console";
import { PresenceFooter } from "@/components/dashboard/data-tables";
import { DashboardFocusProvider } from "@/components/dashboard/dashboard-focus";
import { OperationVerificationProvider } from "@/components/dashboard/operation-verification";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import { DashboardViewStateProvider } from "@/components/dashboard/dashboard-view-state";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  WebTerminalDock,
  WebTerminalManagerProvider,
  WebTerminalWindowLayer,
} from "@/components/dashboard/web-terminal";

export function DashboardLayout({ children }: { children: React.ReactNode }) {
  const { data, stale, error, loading, refreshing } = useDashboardData();
  const { t } = useLocaleText();
  // Focus and verification decide that a target is gone when it is missing
  // from the snapshot, so they wait for a re-read rather than trust one kept
  // from an earlier visit.
  const verifiedData = stale ? null : data;

  return (
    <DashboardFocusProvider data={verifiedData}>
      <DashboardViewStateProvider>
        <OperationVerificationProvider data={verifiedData}>
          <ClientConsoleManagerProvider>
            <WebTerminalManagerProvider>
              <ClientChatProvider>
                <div className="flex flex-1 flex-col" aria-busy={loading || refreshing}>
                  <div className="sr-only" role="status" aria-live="polite">
                    {loading ? t("common.loading") : error}
                  </div>
                  <div className="flex-1">{children}</div>
                  <PresenceFooter />
                </div>
                <ClientConsoleWindowLayer />
                <ClientConsoleDock />
                <WebTerminalWindowLayer />
                <WebTerminalDock />
              </ClientChatProvider>
            </WebTerminalManagerProvider>
          </ClientConsoleManagerProvider>
        </OperationVerificationProvider>
      </DashboardViewStateProvider>
    </DashboardFocusProvider>
  );
}
