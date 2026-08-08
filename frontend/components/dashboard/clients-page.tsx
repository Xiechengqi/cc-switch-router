"use client";

import { Alert } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { ClientBoard } from "@/components/dashboard/client-board";
import { LiveMap } from "@/components/dashboard/live-map";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function ClientsPage() {
  const { data, error, loading, refreshing, refresh } = useDashboardData();
  const { t } = useLocaleText();

  if (loading && !data) {
    return (
      <main className="mx-auto w-[calc(100%-2rem)] max-w-7xl pb-6">
        <h1 className="sr-only">{t("nav.clientsTab")}</h1>
        <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-muted-foreground" role="status">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          {t("common.loading")}
        </div>
      </main>
    );
  }

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl grid-cols-[minmax(0,1fr)] gap-5 pb-6" aria-busy={refreshing}>
      <h1 className="sr-only">{t("nav.clientsTab")}</h1>
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
