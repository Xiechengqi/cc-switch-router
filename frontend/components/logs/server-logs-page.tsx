"use client";

import { Alert, Button, Chip, Drawer } from "@heroui/react";
import { Download, Loader2, RefreshCw, Search } from "lucide-react";
import * as React from "react";

import { useAuth } from "@/components/auth/auth-provider";
import { CompactSelect } from "@/components/common/compact-select";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  exportServerLogs,
  getLiveClientLogTail,
  getServerLogMeta,
  getServerLogs,
} from "@/lib/api";
import type {
  LiveClientLogTail,
  ServerLogClient,
  ServerLogEvent,
  ServerLogMeta,
  ServerLogScope,
} from "@/lib/types";
import { cn, formatDateTime } from "@/lib/utils";

const EMPTY_EVENTS: ServerLogEvent[] = [];

export function ServerLogsPage() {
  const { session } = useAuth();
  const { locale, t } = useLocaleText();
  const [meta, setMeta] = React.useState<ServerLogMeta | null>(null);
  const [scope, setScope] = React.useState<ServerLogScope>("public");
  const [clientFilter, setClientFilter] = React.useState("");
  const [search, setSearch] = React.useState("");
  const [events, setEvents] = React.useState<ServerLogEvent[]>(EMPTY_EVENTS);
  const [nextCursor, setNextCursor] = React.useState<string>();
  const [pageCursors, setPageCursors] = React.useState<(string | undefined)[]>([undefined]);
  const [pageIndex, setPageIndex] = React.useState(0);
  const [selectedClient, setSelectedClient] = React.useState<ServerLogClient | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const requestRef = React.useRef(0);
  const scopeInitializedRef = React.useRef(false);

  const loadMeta = React.useCallback(async () => {
    const next = await getServerLogMeta();
    setMeta(next);
    const preferred: ServerLogScope = next.isRouterOwner
      ? "all"
      : next.authenticated
        ? "mine"
        : "public";
    setScope((current) => {
      if (!scopeInitializedRef.current) {
        scopeInitializedRef.current = true;
        return next.scopes.includes(preferred) ? preferred : next.scopes[0] || "public";
      }
      return next.scopes.includes(current) ? current : next.scopes[0] || "public";
    });
    return next;
  }, []);

  const clients = React.useMemo(() => {
    if (!meta) return [];
    if (scope === "public") return meta.clients;
    return meta.clients.filter((client) => !!client.installationId);
  }, [meta, scope]);

  const load = React.useCallback(
    async (cursor?: string, background = false) => {
      if (!meta?.scopes.includes(scope)) {
        setEvents(EMPTY_EVENTS);
        setNextCursor(undefined);
        setLoading(false);
        return;
      }
      const requestId = ++requestRef.current;
      if (!background) setLoading(true);
      setError("");
      const selected = clients.find((client) => clientFilterValue(client, scope) === clientFilter);
      try {
        const response = await getServerLogs({
          scope,
          installationId: scope === "public" ? undefined : selected?.installationId,
          clientAlias: scope === "public" ? selected?.clientAlias : undefined,
          search: search.trim() || undefined,
          cursor,
          limit: 200,
        });
        if (requestId !== requestRef.current) return;
        setEvents(
          retainVisibleEvents(
            response.events,
            scope,
            response.publicWindowSeconds,
            response.serverTimeMs,
          ),
        );
        setNextCursor(response.nextCursor);
      } catch (cause) {
        if (requestId !== requestRef.current) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        if (!background) setEvents(EMPTY_EVENTS);
      } finally {
        if (requestId === requestRef.current) {
          setLoading(false);
        }
      }
    },
    [clientFilter, clients, meta, scope, search],
  );

  React.useEffect(() => {
    setLoading(true);
    loadMeta()
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setLoading(false));
  }, [loadMeta, session?.isAdmin, session?.user?.email]);

  React.useEffect(() => {
    setClientFilter("");
  }, [scope]);

  React.useEffect(() => {
    setPageCursors([undefined]);
    setPageIndex(0);
  }, [scope, clientFilter, search]);

  React.useEffect(() => {
    if (!meta) return;
    const timer = window.setTimeout(() => void load(pageCursors[pageIndex]), 250);
    return () => window.clearTimeout(timer);
  }, [load, meta, pageCursors, pageIndex]);

  React.useEffect(() => {
    const timer = window.setInterval(() => {
      void loadMeta().catch((cause) =>
        setError(cause instanceof Error ? cause.message : String(cause)),
      );
    }, 15_000);
    return () => window.clearInterval(timer);
  }, [loadMeta]);

  React.useEffect(() => {
    if (!meta?.scopes.includes(scope) || pageIndex !== 0) return;
    const timer = window.setInterval(() => void load(undefined, true), 5_000);
    return () => window.clearInterval(timer);
  }, [load, meta, pageIndex, scope]);

  const scopeItems = (meta?.scopes || []).map((item) => ({
    id: item,
    label: t(scopeMessageKey(item)),
  }));
  const clientOptions = [
    { value: "", label: t("serverLogs.allClients") },
    ...clients.map((client) => ({
      value: clientFilterValue(client, scope),
      label: clientDisplayName(client),
    })),
  ];

  const download = async () => {
    if (scope === "public") return;
    setError("");
    const selected = clients.find((client) => clientFilterValue(client, scope) === clientFilter);
    try {
      const blob = await exportServerLogs({
        scope,
        installationId: selected?.installationId,
        search: search.trim() || undefined,
      });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `cc-switch-server-logs-${new Date().toISOString().slice(0, 10)}.jsonl`;
      anchor.click();
      URL.revokeObjectURL(url);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  };

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl gap-4 pb-8 text-foreground">
      <h1 className="sr-only">{t("nav.logsTab")}</h1>

      <section className="flex flex-wrap items-center gap-2">
        {scopeItems.length > 1 ? (
          <SegmentedControl
            value={scope}
            onChange={setScope}
            items={scopeItems}
            ariaLabel={t("serverLogs.scopeAria")}
            size="md"
          />
        ) : null}
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <Chip size="sm" variant="soft">
            {scope === "public"
              ? t("serverLogs.lastFiveMinutes")
              : t("serverLogs.retentionDays", { days: meta?.retentionDays || 0 })}
          </Chip>
          {scope !== "public" ? (
            <Button
              isIconOnly
              variant="outline"
              aria-label={t("serverLogs.export")}
              onClick={() => void download()}
            >
              <Download className="h-4 w-4" />
            </Button>
          ) : null}
          <Button
            isIconOnly
            variant="outline"
            aria-label={t("common.reload")}
            isDisabled={loading}
            onClick={() => void load(pageCursors[pageIndex])}
          >
            <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
          </Button>
        </div>
      </section>

      <section className="flex flex-wrap items-center gap-2">
        <label className="relative min-w-[15rem] flex-1">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <input
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("serverLogs.search")}
            aria-label={t("serverLogs.search")}
            className="h-10 w-full rounded-md border bg-white pl-9 pr-3 text-sm outline-none transition-colors focus:border-primary"
          />
        </label>
        {clientOptions.length > 1 ? (
          <CompactSelect
            value={clientFilter}
            onChange={setClientFilter}
            ariaLabel={t("serverLogs.client")}
            className="w-56 max-w-full"
            options={clientOptions}
          />
        ) : null}
      </section>

      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {!loading && meta && !meta.scopes.length ? (
        <Alert status="default" className="!text-slate-900">
          {t("serverLogs.publicDisabled")}
        </Alert>
      ) : null}

      <section className="min-w-0 overflow-hidden rounded-md border bg-white">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[760px] table-fixed text-left text-sm">
            <thead className="bg-slate-50 text-xs text-muted-foreground">
              <tr>
                <th className="w-44 px-3 py-2 font-medium">{t("serverLogs.time")}</th>
                <th className="w-44 px-3 py-2 font-medium">{t("serverLogs.client")}</th>
                <th className="w-64 px-3 py-2 font-medium">{t("serverLogs.event")}</th>
                <th className="px-3 py-2 font-medium">{t("serverLogs.details")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {events.map((event) => (
                <LogRow
                  key={event.eventId}
                  event={event}
                  locale={locale}
                  client={findEventClient(meta?.clients || [], event)}
                  onOpenClient={setSelectedClient}
                />
              ))}
            </tbody>
          </table>
        </div>
        {loading && events.length === 0 ? (
          <div className="flex h-48 items-center justify-center text-muted-foreground">
            <Loader2 className="h-5 w-5 animate-spin" />
          </div>
        ) : !loading && events.length === 0 ? (
          <div className="flex h-48 items-center justify-center text-sm text-muted-foreground">
            {t("serverLogs.empty")}
          </div>
        ) : null}
      </section>

      {pageIndex > 0 || nextCursor ? (
        <div className="flex items-center justify-center gap-2">
          <Button
            variant="outline"
            isDisabled={loading || pageIndex === 0}
            onClick={() => {
              setEvents(EMPTY_EVENTS);
              setPageIndex((current) => Math.max(0, current - 1));
            }}
          >
            {t("serverLogs.loadNewer")}
          </Button>
          <span className="min-w-16 text-center text-xs text-muted-foreground">
            {t("serverLogs.page", { page: pageIndex + 1 })}
          </span>
          <Button
            variant="outline"
            isDisabled={loading || !nextCursor}
            onClick={() => {
              if (!nextCursor) return;
              setEvents(EMPTY_EVENTS);
              setPageCursors((current) => [
                ...current.slice(0, pageIndex + 1),
                nextCursor,
              ]);
              setPageIndex((current) => current + 1);
            }}
          >
            {t("serverLogs.loadOlder")}
          </Button>
        </div>
      ) : null}

      <ClientDetailsDrawer
        client={selectedClient}
        canUseLiveDiagnostics={!!meta?.isRouterOwner}
        onOpenChange={(open) => !open && setSelectedClient(null)}
      />
    </main>
  );
}

function LogRow({
  event,
  locale,
  client,
  onOpenClient,
}: {
  event: ServerLogEvent;
  locale: string;
  client?: ServerLogClient;
  onOpenClient: (client: ServerLogClient) => void;
}) {
  const { t } = useLocaleText();
  const details = {
    ...(event.fields || {}),
    ...(event.serverVersion ? { serverVersion: event.serverVersion } : {}),
    ...(event.commitId ? { commitId: event.commitId } : {}),
  };
  const statusCode = numberField(event.fields, "statusCode");
  const outcome = stringField(event.fields, "outcome");
  const detailEntries = Object.entries(details);
  const clientName = event.clientSubdomain || client?.subdomain || event.clientAlias;
  return (
    <tr className="align-top hover:bg-slate-50/70">
      <td className="px-3 py-2 font-mono text-xs text-muted-foreground">
        {new Intl.DateTimeFormat(locale, {
          month: "2-digit",
          day: "2-digit",
          hour: "2-digit",
          minute: "2-digit",
          second: "2-digit",
          hour12: false,
        }).format(new Date(event.occurredAtMs))}
      </td>
      <td className="px-3 py-2">
        {client ? (
          <button
            type="button"
            className="max-w-full truncate font-mono text-xs font-medium text-primary hover:underline"
            title={clientName}
            onClick={() => onOpenClient(client)}
          >
            {clientName}
          </button>
        ) : (
          <span className="block truncate font-mono text-xs" title={clientName}>{clientName}</span>
        )}
      </td>
      <td className="px-3 py-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <span className="break-words font-mono text-xs font-medium">{event.message}</span>
          {statusCode ? <StatusCode value={statusCode} /> : null}
          {outcome ? <span className="text-xs text-muted-foreground">{outcome}</span> : null}
        </div>
      </td>
      <td className="px-3 py-2">
        {detailEntries.length ? (
          <details className="text-xs text-muted-foreground">
            <summary className="cursor-pointer select-none">{t("serverLogs.viewDetails")}</summary>
            <pre className="mt-2 max-h-56 overflow-auto whitespace-pre-wrap rounded bg-slate-50 p-2 font-mono text-[11px]">
              {JSON.stringify(details, null, 2)}
            </pre>
          </details>
        ) : (
          <span className="text-xs text-muted-foreground">-</span>
        )}
      </td>
    </tr>
  );
}

function StatusCode({ value }: { value: number }) {
  return (
    <span
      className={cn(
        "font-mono text-xs font-semibold",
        value >= 500 ? "text-rose-700" : value >= 400 ? "text-amber-700" : "text-emerald-700",
      )}
    >
      {value}
    </span>
  );
}

function ClientDetailsDrawer({
  client,
  canUseLiveDiagnostics,
  onOpenChange,
}: {
  client: ServerLogClient | null;
  canUseLiveDiagnostics: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { locale, t } = useLocaleText();
  const [liveTail, setLiveTail] = React.useState<LiveClientLogTail | null>(null);
  const [liveLoading, setLiveLoading] = React.useState(false);
  const [liveError, setLiveError] = React.useState("");

  React.useEffect(() => {
    setLiveTail(null);
    setLiveError("");
    setLiveLoading(false);
  }, [client?.installationId]);

  const loadLiveTail = async () => {
    if (!client?.installationId) return;
    setLiveLoading(true);
    setLiveError("");
    try {
      setLiveTail(await getLiveClientLogTail(client.installationId));
    } catch (cause) {
      setLiveError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLiveLoading(false);
    }
  };

  return (
    <Drawer.Backdrop isOpen={!!client} onOpenChange={onOpenChange}>
      <Drawer.Content placement="right">
        <Drawer.Dialog className="light !w-[min(560px,calc(100vw-16px))] !max-w-[calc(100vw-16px)] !bg-white !text-slate-900">
          <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
          <Drawer.Header>
            <div className="min-w-0 pr-10">
              <Drawer.Heading className="break-all font-mono text-base">
                {client ? clientDisplayName(client) : "-"}
              </Drawer.Heading>
              <p className="mt-1 text-xs text-slate-500">{t("serverLogs.clientDetails")}</p>
            </div>
          </Drawer.Header>
          <Drawer.Body className="overflow-y-auto">
            {client ? (
              <dl className="grid grid-cols-1 gap-x-5 gap-y-4 sm:grid-cols-2">
                <ClientDetail label={t("serverLogs.subdomain")} value={client.subdomain} mono />
                <ClientDetail label={t("serverLogs.platform")} value={client.platform} />
                <ClientDetail label={t("serverLogs.appVersion")} value={client.appVersion} mono />
                <ClientDetail label={t("serverLogs.owner")} value={client.ownerEmail} />
                <ClientDetail label={t("serverLogs.country")} value={[client.countryCode, client.region].filter(Boolean).join(" / ")} />
                <ClientDetail label={t("serverLogs.tunnel")} value={client.tunnelEnabled == null ? "" : client.tunnelEnabled ? t("common.enabled") : t("common.disabled")} />
                <ClientDetail label={t("serverLogs.createdAt")} value={client.createdAt ? formatDateTime(client.createdAt, locale) : ""} />
                <ClientDetail label={t("serverLogs.lastSeenAt")} value={client.lastSeenAt ? formatDateTime(client.lastSeenAt, locale) : ""} />
                {client.installationId ? (
                  <ClientDetail label={t("serverLogs.installationId")} value={client.installationId} mono wide />
                ) : null}
              </dl>
            ) : null}
            {client?.installationId && canUseLiveDiagnostics ? (
              <section className="mt-7 space-y-3">
                <div className="flex flex-wrap items-center gap-2">
                  <div className="min-w-0 flex-1">
                    <h3 className="text-sm font-semibold text-slate-900">
                      {t("serverLogs.liveDiagnostics")}
                    </h3>
                    <p className="mt-1 text-xs text-slate-500">
                      {t("serverLogs.liveDiagnosticsHint")}
                    </p>
                  </div>
                  <Button
                    size="sm"
                    variant="outline"
                    isDisabled={liveLoading}
                    onClick={() => void loadLiveTail()}
                  >
                    <RefreshCw className={cn("h-4 w-4", liveLoading && "animate-spin")} />
                    {liveTail ? t("common.reload") : t("serverLogs.fetchLiveDiagnostics")}
                  </Button>
                </div>
                {liveError ? <Alert status="danger" className="!text-slate-900">{liveError}</Alert> : null}
                {liveTail ? (
                  <div className="space-y-2">
                    <p className="text-xs text-slate-500">
                      {t("serverLogs.liveDiagnosticsMeta", {
                        lines: liveTail.lines,
                        time: formatDateTime(liveTail.fetchedAt, locale),
                      })}
                      {liveTail.truncated ? ` · ${t("serverLogs.truncated")}` : ""}
                    </p>
                    <pre className="max-h-[28rem] overflow-auto whitespace-pre-wrap break-words rounded-md bg-slate-950 p-3 font-mono text-[11px] leading-5 text-slate-100">
                      {liveTail.content || t("serverLogs.empty")}
                    </pre>
                  </div>
                ) : null}
              </section>
            ) : null}
          </Drawer.Body>
        </Drawer.Dialog>
      </Drawer.Content>
    </Drawer.Backdrop>
  );
}

function ClientDetail({
  label,
  value,
  mono = false,
  wide = false,
}: {
  label: string;
  value?: string;
  mono?: boolean;
  wide?: boolean;
}) {
  return (
    <div className={wide ? "sm:col-span-2" : undefined}>
      <dt className="text-xs font-medium text-slate-500">{label}</dt>
      <dd className={cn("mt-1 break-all text-sm text-slate-900", mono && "font-mono text-xs")}>{value || "-"}</dd>
    </div>
  );
}

function scopeMessageKey(scope: ServerLogScope) {
  switch (scope) {
    case "mine":
      return "serverLogs.scope.mine" as const;
    case "all":
      return "serverLogs.scope.all" as const;
    default:
      return "serverLogs.scope.public" as const;
  }
}

function clientDisplayName(client: ServerLogClient) {
  return client.subdomain || client.clientAlias;
}

function clientFilterValue(client: ServerLogClient, scope: ServerLogScope) {
  return scope === "public" ? client.clientAlias : client.installationId || "";
}

function findEventClient(clients: ServerLogClient[], event: ServerLogEvent) {
  return clients.find((client) =>
    event.installationId
      ? client.installationId === event.installationId
      : client.clientAlias === event.clientAlias,
  );
}

function stringField(fields: Record<string, unknown> | undefined, key: string) {
  const value = fields?.[key];
  return typeof value === "string" ? value : "";
}

function numberField(fields: Record<string, unknown> | undefined, key: string) {
  const value = fields?.[key];
  return typeof value === "number" ? value : 0;
}

function retainVisibleEvents(
  events: ServerLogEvent[],
  scope: ServerLogScope,
  publicWindowSeconds?: number,
  serverTimeMs?: number,
) {
  if (scope !== "public" || !publicWindowSeconds || !serverTimeMs) return events;
  const cutoff = serverTimeMs - publicWindowSeconds * 1_000;
  return events.filter((event) => event.occurredAtMs >= cutoff);
}
