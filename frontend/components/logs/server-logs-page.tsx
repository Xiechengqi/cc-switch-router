"use client";

import { Alert, Button, Chip } from "@heroui/react";
import {
  ChevronDown,
  ChevronRight,
  Loader2,
  RefreshCw,
  Search,
} from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getServerLogMeta, getServerLogs } from "@/lib/api";
import type {
  ServerLogClient,
  ServerLogEvent,
  ServerLogMeta,
} from "@/lib/types";
import { cn, formatRelativeTime } from "@/lib/utils";

const EMPTY_EVENTS: ServerLogEvent[] = [];

export function ServerLogsPage() {
  const { session } = useAuth();
  const { locale, t } = useLocaleText();
  const authIdentity = [
    session?.authenticated ? "authenticated" : "anonymous",
    session?.user?.email?.trim().toLocaleLowerCase("en-US") || "",
    session?.isAdmin ? "admin" : "user",
  ].join(":");
  const [metaState, setMetaState] = React.useState<{
    authIdentity: string;
    value: ServerLogMeta;
  } | null>(null);
  const [filter, setFilter] = React.useState("");
  const [expandedAliases, setExpandedAliases] = React.useState<Set<string>>(
    () => new Set(),
  );
  const [refreshToken, setRefreshToken] = React.useState(0);
  const [loading, setLoading] = React.useState(true);
  const [errorState, setErrorState] = React.useState<{
    authIdentity: string;
    message: string;
  } | null>(null);
  const metaRequestRef = React.useRef(0);
  const meta = metaState?.authIdentity === authIdentity ? metaState.value : null;
  const error = errorState?.authIdentity === authIdentity ? errorState.message : "";
  const pageLoading = loading || (!meta && !error);

  const loadMeta = React.useCallback(
    async (silent = false) => {
      const requestId = ++metaRequestRef.current;
      if (!silent) setLoading(true);
      try {
        const next = await getServerLogMeta();
        if (requestId !== metaRequestRef.current) return;
        setMetaState({ authIdentity, value: next });
        setErrorState(null);
      } catch (cause) {
        if (requestId !== metaRequestRef.current) return;
        setErrorState({
          authIdentity,
          message: cause instanceof Error ? cause.message : String(cause),
        });
      } finally {
        if (requestId === metaRequestRef.current) setLoading(false);
      }
    },
    [authIdentity],
  );

  React.useEffect(() => {
    void loadMeta();
    return () => {
      metaRequestRef.current += 1;
    };
  }, [loadMeta]);

  React.useEffect(() => {
    const timer = window.setInterval(() => void loadMeta(true), 10_000);
    return () => window.clearInterval(timer);
  }, [loadMeta]);

  const toggleClient = React.useCallback((clientAlias: string) => {
    setExpandedAliases((current) => {
      const next = new Set(current);
      if (next.has(clientAlias)) next.delete(clientAlias);
      else next.add(clientAlias);
      return next;
    });
  }, []);

  const normalizedFilter = filter.trim().toLocaleLowerCase(locale);
  const clients = (meta?.clients || []).filter((client) => {
    if (!normalizedFilter) return true;
    return [
      client.subdomain,
      client.clientAlias,
      client.platform,
      client.appVersion,
      client.countryCode,
      client.region,
    ].some((value) => value?.toLocaleLowerCase(locale).includes(normalizedFilter));
  });

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl gap-4 pb-8 text-foreground">
      <header className="flex flex-wrap items-center gap-2">
        <label className="relative min-w-[15rem] flex-1">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" />
          <input
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
            placeholder={t("serverLogs.filterClients")}
            aria-label={t("serverLogs.filterClients")}
            className="h-10 w-full rounded-md border bg-white pl-9 pr-3 text-sm outline-none transition-colors focus:border-primary"
          />
        </label>
        {meta ? (
          <Chip size="sm" variant="soft">
            {t("serverLogs.retainedLines", { count: meta.retainedLineLimit })}
          </Chip>
        ) : null}
        <Button
          isIconOnly
          variant="outline"
          aria-label={t("common.reload")}
          isDisabled={pageLoading}
          onClick={() => {
            setRefreshToken((current) => current + 1);
            void loadMeta();
          }}
        >
          <RefreshCw className={cn("h-4 w-4", pageLoading && "animate-spin")} />
        </Button>
      </header>

      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {meta && !meta.ingestEnabled ? (
        <Alert status="warning" className="!text-slate-900">
          {t("serverLogs.collectionDisabled")}
        </Alert>
      ) : null}

      <section className="min-w-0 overflow-hidden rounded-md border bg-white">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[760px] table-fixed text-left text-sm">
            <thead className="border-b bg-slate-50 text-xs text-muted-foreground">
              <tr>
                <th className="w-10 px-2 py-2 font-medium" aria-label={t("serverLogs.logText")} />
                <th className="w-[32%] px-3 py-2 font-medium">{t("serverLogs.client")}</th>
                <th className="w-32 px-3 py-2 font-medium">{t("dashboard.platform")}</th>
                <th className="w-28 px-3 py-2 font-medium">{t("dashboard.version")}</th>
                <th className="w-44 px-3 py-2 font-medium">{t("dashboard.lastSeen")}</th>
                <th className="w-40 px-3 py-2 font-medium">{t("serverLogs.access")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {clients.map((client) => {
                const expanded = expandedAliases.has(client.clientAlias);
                return (
                  <React.Fragment key={client.clientAlias}>
                    <tr
                      className={cn(
                        "cursor-pointer transition-colors hover:bg-slate-50",
                        expanded && "bg-slate-50",
                      )}
                      onClick={() => toggleClient(client.clientAlias)}
                    >
                      <td className="px-2 py-2 text-center">
                        <button
                          type="button"
                          className="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-600 outline-none hover:bg-slate-200 focus-visible:ring-2 focus-visible:ring-primary/30"
                          aria-expanded={expanded}
                          aria-label={clientLabel(client, t("serverLogs.noSubdomain"))}
                          onClick={(event) => {
                            event.stopPropagation();
                            toggleClient(client.clientAlias);
                          }}
                        >
                          {expanded ? (
                            <ChevronDown className="h-4 w-4" />
                          ) : (
                            <ChevronRight className="h-4 w-4" />
                          )}
                        </button>
                      </td>
                      <td className="px-3 py-2">
                        <div
                          className="truncate font-mono text-xs font-medium text-slate-900"
                          title={clientLabel(client, t("serverLogs.noSubdomain"))}
                        >
                          {clientLabel(client, t("serverLogs.noSubdomain"))}
                        </div>
                        {client.subdomain ? (
                          <div className="mt-0.5 truncate font-mono text-[11px] text-muted-foreground">
                            {client.clientAlias}
                          </div>
                        ) : null}
                      </td>
                      <td className="truncate px-3 py-2 text-xs" title={client.platform}>
                        {client.platform || "-"}
                      </td>
                      <td className="truncate px-3 py-2 font-mono text-xs" title={client.appVersion}>
                        {client.appVersion || "-"}
                      </td>
                      <td
                        className="px-3 py-2 text-xs text-muted-foreground"
                        title={new Date(client.lastSeenAt).toLocaleString(locale)}
                      >
                        {formatRelativeTime(client.lastSeenAt, locale)}
                      </td>
                      <td className="px-3 py-2">
                        <div className="flex flex-wrap items-center gap-2">
                          <Chip
                            size="sm"
                            variant="soft"
                            color={client.fullLogAccess ? "success" : "default"}
                          >
                            {accessLabel(client, Boolean(meta?.isRouterOwner), {
                              routerOwner: t("serverLogs.routerOwner"),
                              clientOwner: t("serverLogs.clientOwner"),
                              publicAccess: t("serverLogs.publicAccess"),
                            })}
                          </Chip>
                          <span className="font-mono text-[11px] text-muted-foreground">
                            {t("serverLogs.visibleLines", {
                              count: client.fullLogAccess
                                ? meta?.retainedLineLimit || 100
                                : meta?.publicLineLimit || 10,
                            })}
                          </span>
                        </div>
                      </td>
                    </tr>
                    {expanded && meta ? (
                      <tr>
                        <td colSpan={6} className="border-t bg-slate-100 p-3">
                          <ClientLogViewer
                            key={`${client.clientAlias}:${client.fullLogAccess}`}
                            client={client}
                            locale={locale}
                            pollIntervalSeconds={meta.pollIntervalSeconds}
                            refreshToken={refreshToken}
                          />
                        </td>
                      </tr>
                    ) : null}
                  </React.Fragment>
                );
              })}
            </tbody>
          </table>
        </div>
        {pageLoading && !meta ? (
          <div className="flex h-48 items-center justify-center text-muted-foreground">
            <Loader2 className="h-5 w-5 animate-spin" />
          </div>
        ) : !pageLoading && clients.length === 0 ? (
          <div className="flex h-48 items-center justify-center text-sm text-muted-foreground">
            {t("serverLogs.emptyClients")}
          </div>
        ) : null}
      </section>
    </main>
  );
}

function ClientLogViewer({
  client,
  locale,
  pollIntervalSeconds,
  refreshToken,
}: {
  client: ServerLogClient;
  locale: string;
  pollIntervalSeconds: number;
  refreshToken: number;
}) {
  const { t } = useLocaleText();
  const [events, setEvents] = React.useState<ServerLogEvent[]>(EMPTY_EVENTS);
  const [visibleLineLimit, setVisibleLineLimit] = React.useState(
    client.fullLogAccess ? 100 : 10,
  );
  const [loading, setLoading] = React.useState(true);
  const [refreshing, setRefreshing] = React.useState(false);
  const [error, setError] = React.useState("");
  const inFlightRef = React.useRef(false);

  const load = React.useCallback(
    async (silent = false) => {
      if (inFlightRef.current) return;
      inFlightRef.current = true;
      if (silent) setRefreshing(true);
      else setLoading(true);
      try {
        const response = await getServerLogs({
          clientAlias: client.clientAlias,
          limit: client.fullLogAccess ? 100 : 10,
        });
        setEvents(response.events);
        setVisibleLineLimit(response.visibleLineLimit);
        setError("");
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        inFlightRef.current = false;
        setLoading(false);
        setRefreshing(false);
      }
    },
    [client.clientAlias, client.fullLogAccess],
  );

  React.useEffect(() => {
    void load(false);
    const timer = window.setInterval(
      () => void load(true),
      Math.max(1, pollIntervalSeconds) * 1_000,
    );
    return () => window.clearInterval(timer);
  }, [load, pollIntervalSeconds, refreshToken]);

  const text = React.useMemo(
    () =>
      events.length > 0
        ? [...events]
            .reverse()
            .map((event) => formatLogLine(event, locale))
            .join("\n")
        : t("serverLogs.noLogs"),
    [events, locale, t],
  );

  return (
    <div className="min-w-0" onClick={(event) => event.stopPropagation()}>
      <div className="mb-2 flex min-h-8 items-center justify-between gap-3">
        <div className="min-w-0 truncate font-mono text-xs font-medium text-slate-700">
          {clientLabel(client, t("serverLogs.noSubdomain"))}
          <span className="ml-2 text-muted-foreground">
            {t("serverLogs.visibleLines", { count: visibleLineLimit })}
          </span>
        </div>
        <Button
          isIconOnly
          size="sm"
          variant="ghost"
          aria-label={t("serverLogs.refreshClient")}
          isDisabled={loading || refreshing}
          onClick={() => void load(false)}
        >
          <RefreshCw
            className={cn("h-3.5 w-3.5", (loading || refreshing) && "animate-spin")}
          />
        </Button>
      </div>
      {error ? <Alert status="danger" className="mb-2 !text-slate-900">{error}</Alert> : null}
      <LogTextArea
        value={text}
        label={t("serverLogs.logText")}
        loading={loading && events.length === 0}
      />
    </div>
  );
}

function LogTextArea({
  value,
  label,
  loading,
}: {
  value: string;
  label: string;
  loading: boolean;
}) {
  const ref = React.useRef<HTMLTextAreaElement>(null);
  const stickToBottomRef = React.useRef(true);

  React.useLayoutEffect(() => {
    if (stickToBottomRef.current && ref.current) {
      ref.current.scrollTop = ref.current.scrollHeight;
    }
  }, [value]);

  if (loading) {
    return (
      <div className="flex h-80 items-center justify-center border bg-white text-muted-foreground">
        <Loader2 className="h-5 w-5 animate-spin" />
      </div>
    );
  }

  return (
    <textarea
      ref={ref}
      readOnly
      spellCheck={false}
      wrap="off"
      value={value}
      aria-label={label}
      className="block h-80 w-full resize-none overflow-auto border bg-white p-3 font-mono text-xs leading-5 text-slate-950 outline-none focus:border-primary"
      onScroll={(event) => {
        const target = event.currentTarget;
        stickToBottomRef.current =
          target.scrollHeight - target.scrollTop - target.clientHeight < 24;
      }}
    />
  );
}

function clientLabel(client: ServerLogClient, fallback: string) {
  return client.subdomain || client.clientAlias || fallback;
}

function accessLabel(
  client: ServerLogClient,
  isRouterOwner: boolean,
  labels: { routerOwner: string; clientOwner: string; publicAccess: string },
) {
  if (isRouterOwner) return labels.routerOwner;
  if (client.owned) return labels.clientOwner;
  return labels.publicAccess;
}

function formatLogLine(event: ServerLogEvent, locale: string) {
  if (event.rawLine) return event.rawLine;

  const timestamp = new Intl.DateTimeFormat(locale, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    fractionalSecondDigits: 3,
    hour12: false,
  }).format(new Date(event.receivedAtMs));
  const details = {
    ...(event.fields || {}),
    ...(event.file ? { source: `${event.file}${event.line ? `:${event.line}` : ""}` } : {}),
    ...(event.streamId ? { streamId: event.streamId } : {}),
    ...(event.sequence !== undefined ? { sequence: event.sequence } : {}),
    ...(event.serverVersion ? { serverVersion: event.serverVersion } : {}),
    ...(event.commitId ? { commitId: event.commitId } : {}),
  };
  const suffix = Object.keys(details).length > 0 ? ` ${JSON.stringify(details)}` : "";
  return `${timestamp} ${event.level.toUpperCase().padEnd(5)} ${singleLine(event.target)} ${singleLine(event.message || "-")}${suffix}`;
}

function singleLine(value: string) {
  return value.replaceAll("\r", "\\r").replaceAll("\n", "\\n");
}
