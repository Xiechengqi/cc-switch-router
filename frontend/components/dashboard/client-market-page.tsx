"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { OnlineClientMarket } from "@/components/dashboard/client-market/online-market";
import { ClientRentalAuditPanel } from "@/components/dashboard/client-market/rental-audit-panel";
import { ClientRentalRecords } from "@/components/dashboard/client-market/rental-records";
import { ClientTerminationDialog } from "@/components/market/client-termination-dialog";
import { useClientTerminationWorkspace } from "@/components/market/use-client-termination-workspace";
import { currentPageHref } from "@/lib/url-search";

/** Query parameters: `focus` (listing installation or lease id), `provider`,
 * `terminationId` (opens the termination dialog) and `rentalId` (personal record). */
export function ClientMarketPage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const searchParams = useSearchParams();
  const actor = session?.authenticated ? session.user?.id : undefined;
  const terminationWork = useClientTerminationWorkspace(actor);
  const [rentalRevision, setRentalRevision] = React.useState(0);
  const onRentalChanged = React.useCallback(() => setRentalRevision((n) => n + 1), []);
  const queryTerminationId = searchParams.get("terminationId");
  // Bound to the account that opened it, so a late account switch cannot show another actor's case.
  const [termination, setTermination] = React.useState<{ id: string; userId: string } | null>(null);
  const handledQuery = React.useRef<string | null>(null);
  const terminationId = termination && termination.userId === actor ? termination.id : null;
  const openTermination = React.useCallback((id: string) => {
    if (actor) setTermination({ id, userId: actor });
  }, [actor]);
  const providerId = searchParams.get("provider") || undefined;
  const terminations = React.useMemo(
    () => new Map(terminationWork.workspace?.hosts.map((host) => [host.hostId, host]) || []),
    [terminationWork.workspace],
  );
  const closeTermination = React.useCallback(() => {
    setTermination(null);
    if (new URLSearchParams(window.location.search).has("terminationId")) {
      // A null state lets Next resync useSearchParams, so a repeated deep link opens again.
      window.history.replaceState(null, "", currentPageHref(window.location.href, (params) => params.delete("terminationId")));
    }
  }, []);
  React.useEffect(() => {
    if (termination && termination.userId !== actor) closeTermination();
    else if (queryTerminationId && actor && handledQuery.current !== queryTerminationId) {
      handledQuery.current = queryTerminationId;
      openTermination(queryTerminationId);
    } else if (!queryTerminationId && handledQuery.current) {
      handledQuery.current = null;
      setTermination(null);
    }
  }, [queryTerminationId, actor, termination, openTermination, closeTermination]);
  const clearProvider = () => {
    window.history.replaceState(null, "", currentPageHref(window.location.href, (params) => params.delete("provider")));
  };
  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl grid-cols-[minmax(0,1fr)] gap-5 pb-10">
      <h1 className="sr-only">{t("clientMarket.title")}</h1>
      <OnlineClientMarket
        focusId={searchParams.get("focus")}
        providerId={providerId}
        rentalId={searchParams.get("rentalId")}
        tenantRecords={<ClientRentalRecords currentOnly focusKey={searchParams.get("rentalId")} onTerminationOpen={openTermination} revision={terminationWork.revision + rentalRevision} />}
        terminations={terminations}
        onChanged={onRentalChanged}
        onTerminationOpen={openTermination}
        providerNotice={providerId ? (
          <div className="flex min-h-11 min-w-0 flex-wrap items-center justify-between gap-2 rounded-lg bg-slate-100 px-3 py-2 text-sm">
            <span className="min-w-0 text-slate-700">{t("providers.marketFilter.client")}</span>
            <Button variant="ghost" className="min-h-11" onPress={clearProvider}>{t("providers.marketFilter.clear")}</Button>
          </div>
        ) : null}
      />
      {actor && terminationWork.error ? (
        <div className="flex flex-wrap items-center gap-2">
          <p role="alert" className="text-xs text-danger">{t("clientTermination.workspaceUnavailable")}</p>
          <Button
            size="sm"
            variant="ghost"
            className="min-h-11 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
            onPress={() => void terminationWork.refresh(true)}
          >
            {t("common.reload")}
          </Button>
        </div>
      ) : null}
      <ClientTerminationDialog terminationId={terminationId} onClose={closeTermination} onChanged={() => void terminationWork.refresh(true)} />
      <ClientRentalRecords historyOnly focusKey={searchParams.get("rentalId")} onTerminationOpen={openTermination} revision={terminationWork.revision + rentalRevision} />
      <ClientRentalAuditPanel onTerminationOpen={openTermination} revision={terminationWork.revision + rentalRevision} />
    </main>
  );
}
