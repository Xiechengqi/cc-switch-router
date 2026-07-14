"use client";

import { Terminal } from "lucide-react";
import * as React from "react";
import { useClientConsole } from "@/components/dashboard/client-console";
import { clientTunnelDisplayUrl } from "@/components/dashboard/share-dashboard-utils";
import type { TFn } from "@/components/dashboard/share-dashboard-utils";
import type { DashboardClient } from "@/lib/types";
export function ShareClientTag({
  client,
  t,
}: {
  client?: DashboardClient;
  t: TFn;
}) {
  const { openConsole } = useClientConsole();
  const url = clientTunnelDisplayUrl(client?.clientTunnel?.tunnelUrl);
  if (!url || !client) return null;
  const title = client.clientTunnel?.subdomain || url;
  const handle = (event: React.MouseEvent) => {
    event.stopPropagation();
    openConsole({
      clientId: client.installation.id,
      url,
      title,
    });
  };
  return (
    <button
      type="button"
      onClick={handle}
      data-no-row-drawer
      title={url}
      className="inline-flex h-[22px] items-center gap-1 rounded-full border border-sky-200 bg-sky-50 px-2.5 text-[11px] font-medium text-sky-700 transition-colors hover:border-sky-300 hover:bg-sky-100"
    >
      <Terminal className="h-3 w-3" />
      {t("dashboard.clientConsole")}
    </button>
  );
}
