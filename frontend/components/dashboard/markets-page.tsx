"use client";

import { Alert } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { MarketsTable } from "@/components/dashboard/markets-table";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function MarketsPage() {
  const { data, error, loading, refreshing, refresh } = useDashboardData();
  const { t } = useLocaleText();

  if (loading && !data) {
    return (
      <main className="mx-auto w-[calc(100%-2rem)] max-w-7xl pb-6">
        <h1 className="sr-only">{t("nav.marketsTab")}</h1>
        <div className="flex min-h-48 items-center justify-center gap-2 text-sm text-muted-foreground" role="status">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          {t("common.loading")}
        </div>
      </main>
    );
  }

  return (
    <main className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-6" aria-busy={refreshing}>
      <h1 className="sr-only">{t("nav.marketsTab")}</h1>
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      <MarketsTable markets={data?.markets || []} onChanged={refresh} />
    </main>
  );
}
