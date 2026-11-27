"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { useRouter, useSearchParams } from "next/navigation";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { OnlineClientMarket } from "@/components/dashboard/client-market/online-market";
import { clientMarketTenantHref } from "@/lib/dashboard-nav";
import { ClientTerminationDialog } from "@/components/market/client-termination-dialog";
import { useClientTerminationWorkspace } from "@/components/market/use-client-termination-workspace";
import { ClientConsoleIframe } from "./client-console/client-console-iframe";
import { authFetch } from "@/lib/auth";
import { onlineRequest, type OnlineLease } from "@/lib/online-client-market";
import { clientMarketTenantFromQuery, clientMarketWorkspaceFromQuery, ROUTER_OPEN_LOGIN_EVENT } from "./client-market/host-utils";
import { currentPageHref } from "@/lib/url-search";

/** Query parameters: `focus` (listing installation or lease id), `provider`,
 * `terminationId` (opens the termination dialog) and `rentalId` (personal record). */
export function ClientMarketPage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const searchParams = useSearchParams();
  const actor = session?.authenticated ? session.user?.id : undefined;
  const terminationWork = useClientTerminationWorkspace(actor);
  const router = useRouter();
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
  const openId = searchParams.get("openLease") || searchParams.get("embeddedLease");
  const rentalId = searchParams.get("rentalId");
  const legacyRentals = !openId && !queryTerminationId
    && clientMarketWorkspaceFromQuery(searchParams.get("view"), searchParams.get("tab")) === "catalog"
    && clientMarketTenantFromQuery(searchParams.get("view"), rentalId);
  const rentalFocus = rentalId || searchParams.get("focus") || undefined;
  React.useEffect(() => {
    if (legacyRentals) router.replace(clientMarketTenantHref(rentalFocus));
  }, [legacyRentals, rentalFocus, router]);
  if (legacyRentals) return <p role="status">{t("common.loading")}</p>;
  if (openId)
    return (
      <WorkspaceLaunch
        key={JSON.stringify([actor, openId, searchParams.has("embeddedLease")])}
        id={openId}
        embedded={searchParams.has("embeddedLease")}
      />
    );
  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl grid-cols-[minmax(0,1fr)] gap-5 pb-10">
      <h1 className="sr-only">{t("clientMarket.title")}</h1>
      <OnlineClientMarket
        focusId={searchParams.get("focus")}
        providerId={providerId}
        terminations={terminations}
        onTerminationOpen={openTermination}
        auditRevision={terminationWork.revision}
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
    </main>
  );
}

function WorkspaceLaunch({ id, embedded }: { id: string; embedded: boolean }) {
  const { session, loading } = useAuth();
  const { t } = useLocaleText();
  const actor = session?.authenticated ? session.user?.id : null;
  const [frame, setFrame] = React.useState<OnlineLease | null>(null);
  const [failed, setFailed] = React.useState(false);
  const [attempt, retry] = React.useReducer((n: number) => n + 1, 0);
  React.useEffect(() => {
    if (loading) return;
    if (!actor) {
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    const ctl = new AbortController();
    setFailed(false);
    async function open() {
      const { leases } = await onlineRequest<{ leases: OnlineLease[] }>(
        "leases?current=true",
        "GET",
        undefined,
        ctl.signal,
      );
      const lease = leases.find((item) => item.leaseId === id && item.state === "active");
      if (!lease) throw new Error("lease unavailable");
      const url = new URL(lease.clientUrl);
      if (
        !["https:", "http:"].includes(url.protocol) ||
        url.username ||
        url.password ||
        url.hostname.split(".")[0] !== lease.alias ||
        !/^rent-[0-9a-f]{20}$/.test(lease.alias)
      )
        throw new Error("invalid lease origin");
      if (ctl.signal.aborted) return;
      if (embedded) {
        setFrame(lease);
        return;
      }
      const nonce = crypto.randomUUID();
      const response = await authFetch("/v1/auth/client-web-tickets", {
        method: "POST",
        signal: ctl.signal,
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ installationId: id, kind: "console", nonce }),
      });
      if (!response.ok) throw new Error("workspace login unavailable");
      const { ticket, origin } = await response.json();
      if (origin !== url.origin || typeof ticket !== "string" || !/^[a-zA-Z0-9]{48}$/.test(ticket))
        throw new Error("invalid lease ticket");
      if (ctl.signal.aborted) return;
      const form = document.createElement("form");
      form.method = "post";
      form.action = new URL("/web-api/auth/router-launch", origin).href;
      for (const [name, value] of Object.entries({ ticket, nonce })) {
        const field = document.createElement("input");
        field.type = "hidden";
        field.name = name;
        field.value = value;
        form.appendChild(field);
      }
      document.body.appendChild(form);
      form.submit();
      form.remove();
    }
    void open().catch(() => {
      if (!ctl.signal.aborted) setFailed(true);
    });
    return () => ctl.abort();
  }, [actor, loading, embedded, id, attempt]);
  if (frame)
    return (
      <main className="h-[85dvh]">
        <ClientConsoleIframe
          window={{
            id,
            clientId: id,
            reloadKey: 0,
            title: frame.alias,
            url: frame.clientUrl,
            kind: "console",
          }}
        />
      </main>
    );
  return (
    <main className="mx-auto grid max-w-md gap-3 p-5">
      <h1>{t("online.console")}</h1>
      {!loading && !actor ? (
        <Button onPress={() => window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT))}>
          {t("nav.login")}
        </Button>
      ) : failed ? (
        <>
          <p role="alert">{t("lease.unavailable")}</p>
          <Button onPress={retry}>{t("common.retry")}</Button>
        </>
      ) : (
        <p role="status">{t("common.loading")}</p>
      )}
      <a href={clientMarketTenantHref(id)}>{t("online.myLeases")}</a>
    </main>
  );
}
