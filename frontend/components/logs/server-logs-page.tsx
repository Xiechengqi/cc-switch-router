"use client";

import { Alert, Button, Chip } from "@heroui/react";
import { Download, Loader2, RefreshCw, Search } from "lucide-react";
import * as React from "react";
import { CompactSelect } from "@/components/common/compact-select";
import { SegmentedControl } from "@/components/common/segmented-control";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { exportServerLogs, getServerLogMeta, getServerLogs } from "@/lib/api";
import type {
  ServerLogEvent,
  ServerLogMeta,
  ServerLogScope,
} from "@/lib/types";
import { cn } from "@/lib/utils";

const EMPTY_EVENTS: ServerLogEvent[] = [];

export function ServerLogsPage() {
  const { session } = useAuth();
  const { locale, t } = useLocaleText();
  const [meta, setMeta] = React.useState<ServerLogMeta | null>(null);
  const [scope, setScope] = React.useState<ServerLogScope>("public");
  const [level, setLevel] = React.useState("");
  const [installationId, setInstallationId] = React.useState("");
  const [search, setSearch] = React.useState("");
  const [events, setEvents] = React.useState<ServerLogEvent[]>(EMPTY_EVENTS);
  const [nextCursor, setNextCursor] = React.useState<string>();
  const [loading, setLoading] = React.useState(true);
  const [loadingMore, setLoadingMore] = React.useState(false);
  const [error, setError] = React.useState("");
  const requestRef = React.useRef(0);
  const scopeInitializedRef = React.useRef(false);

  const loadMeta = React.useCallback(async () => {
    const next = await getServerLogMeta();
    setMeta(next);
    const preferred: ServerLogScope = next.isAdmin
      ? "all"
      : next.authenticated
        ? "mine"
        : "public";
    setScope((current) => {
      if (!scopeInitializedRef.current) {
        scopeInitializedRef.current = true;
        return preferred;
      }
      return next.scopes.includes(current) ? current : preferred;
    });
    if (!next.scopes.includes(preferred)) setEvents(EMPTY_EVENTS);
    return next;
  }, []);

  const load = React.useCallback(
    async (cursor?: string, preserveExisting = false) => {
      if (!meta?.scopes.includes(scope)) {
        setLoading(false);
        return;
      }
      const requestId = ++requestRef.current;
      cursor ? setLoadingMore(true) : setLoading(true);
      setError("");
      try {
        const response = await getServerLogs({
          scope,
          installationId:
            scope === "public" || !installationId ? undefined : installationId,
          level: level || undefined,
          search: search.trim() || undefined,
          cursor,
          limit: 200,
        });
        if (requestId !== requestRef.current) return;
        setEvents((current) => {
          if (cursor) return mergeEvents(current, response.events);
          if (preserveExisting) return mergeEvents(response.events, current);
          return response.events;
        });
        setNextCursor((current) => (preserveExisting ? current : response.nextCursor));
      } catch (cause) {
        if (requestId !== requestRef.current) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        if (!cursor) setEvents(EMPTY_EVENTS);
      } finally {
        if (requestId === requestRef.current) {
          setLoading(false);
          setLoadingMore(false);
        }
      }
    },
    [installationId, level, meta, scope, search],
  );

  React.useEffect(() => {
    setLoading(true);
    loadMeta()
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setLoading(false));
  }, [loadMeta, session?.isAdmin, session?.user?.email]);

  React.useEffect(() => {
    if (!meta) return;
    const timer = window.setTimeout(() => void load(), 250);
    return () => window.clearTimeout(timer);
  }, [load, meta]);

  React.useEffect(() => {
    const timer = window.setInterval(() => {
      void loadMeta().catch((cause) =>
        setError(cause instanceof Error ? cause.message : String(cause)),
      );
    }, 10_000);
    return () => window.clearInterval(timer);
  }, [loadMeta]);

  React.useEffect(() => {
    if (!meta?.scopes.includes(scope)) return;
    const timer = window.setInterval(() => void load(undefined, true), 5_000);
    return () => window.clearInterval(timer);
  }, [load, meta, scope]);

  const scopeItems = (meta?.scopes || []).map((item) => ({
    id: item,
    label: t(`serverLogs.scope.${item}`),
  }));
  const clientOptions = [
    { value: "", label: t("serverLogs.allClients") },
    ...(meta?.clients || []).map((client) => ({
      value: client.installationId,
      label: client.clientAlias,
    })),
  ];

  const download = async () => {
    if (scope === "public") return;
    setError("");
    try {
      const blob = await exportServerLogs({
        scope,
        installationId: installationId || undefined,
        level: level || undefined,
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
      <header className="flex flex-wrap items-end justify-between gap-3 border-b pb-4">
        <div>
          <div className="section-label">Server</div>
          <h1 className="mt-2 font-display text-2xl">{t("serverLogs.title")}</h1>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {scope === "public" ? (
            <Chip size="sm" variant="soft">{t("serverLogs.lastFiveMinutes")}</Chip>
          ) : meta ? (
            <Chip size="sm" variant="soft">
              {t("serverLogs.retentionDays", { days: meta.retentionDays })}
            </Chip>
          ) : null}
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
            onClick={() => void load(undefined, true)}
          >
            <RefreshCw className={cn("h-4 w-4", loading && "animate-spin")} />
          </Button>
        </div>
      </header>

      {scopeItems.length > 1 ? (
        <SegmentedControl
          value={scope}
          onChange={(next) => {
            setScope(next);
            setInstallationId("");
          }}
          items={scopeItems}
          ariaLabel={t("serverLogs.scopeAria")}
          size="md"
        />
      ) : null}

      <section className="flex flex-wrap items-center gap-2 border-b pb-4">
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
        <CompactSelect
          value={level}
          onChange={setLevel}
          ariaLabel={t("serverLogs.level")}
          className="w-36"
          options={[
            { value: "", label: t("serverLogs.allLevels") },
            { value: "error", label: "ERROR" },
            { value: "warn", label: "WARN" },
            { value: "info", label: "INFO" },
          ]}
        />
        {scope !== "public" && clientOptions.length > 1 ? (
          <CompactSelect
            value={installationId}
            onChange={setInstallationId}
            ariaLabel={t("serverLogs.client")}
            className="w-48"
            options={clientOptions}
          />
        ) : null}
      </section>

      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {!loading && meta && !meta.scopes.includes(scope) ? (
        <Alert status="default" className="!text-slate-900">
          {t("serverLogs.publicDisabled")}
        </Alert>
      ) : null}

      <section className="min-w-0 overflow-hidden rounded-md border bg-white">
        <div className="overflow-x-auto">
          <table className="w-full min-w-[760px] table-fixed text-left text-sm">
            <thead className="border-b bg-slate-50 text-xs text-muted-foreground">
              <tr>
                <th className="w-44 px-3 py-2 font-medium">{t("serverLogs.time")}</th>
                <th className="w-32 px-3 py-2 font-medium">{t("serverLogs.client")}</th>
                <th className="w-20 px-3 py-2 font-medium">{t("serverLogs.level")}</th>
                <th className="w-56 px-3 py-2 font-medium">{t("serverLogs.target")}</th>
                <th className="px-3 py-2 font-medium">{t("serverLogs.message")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {events.map((event) => (
                <LogRow key={event.eventId} event={event} locale={locale} />
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

      {nextCursor ? (
        <div className="flex justify-center">
          <Button
            variant="outline"
            isDisabled={loadingMore}
            onClick={() => void load(nextCursor)}
          >
            {loadingMore ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
            {t("serverLogs.loadOlder")}
          </Button>
        </div>
      ) : null}
    </main>
  );
}

function LogRow({
  event,
  locale,
}: {
  event: ServerLogEvent;
  locale: string;
}) {
  const { t } = useLocaleText();
  const details = {
    ...(event.fields || {}),
    ...(event.file ? { source: `${event.file}${event.line ? `:${event.line}` : ""}` } : {}),
    ...(event.serverVersion ? { serverVersion: event.serverVersion } : {}),
    ...(event.commitId ? { commitId: event.commitId } : {}),
  };
  const hasDetails = Object.keys(details).length > 0;
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
        }).format(new Date(event.receivedAtMs))}
      </td>
      <td className="truncate px-3 py-2 font-mono text-xs" title={event.clientAlias}>
        {event.clientAlias}
      </td>
      <td className="px-3 py-2">
        <span
          className={cn(
            "font-mono text-xs font-semibold",
            event.level === "error"
              ? "text-rose-700"
              : event.level === "warn"
                ? "text-amber-700"
                : "text-sky-700",
          )}
        >
          {event.level.toUpperCase()}
        </span>
      </td>
      <td className="truncate px-3 py-2 font-mono text-xs text-muted-foreground" title={event.target}>
        {event.target}
      </td>
      <td className="px-3 py-2">
        <div className="break-words leading-5">{event.message || "-"}</div>
        {hasDetails ? (
          <details className="mt-1 text-xs text-muted-foreground">
            <summary className="cursor-pointer select-none">{t("serverLogs.details")}</summary>
            <pre className="mt-2 max-h-56 overflow-auto whitespace-pre-wrap rounded bg-slate-50 p-2 font-mono text-[11px]">
              {JSON.stringify(details, null, 2)}
            </pre>
          </details>
        ) : null}
      </td>
    </tr>
  );
}

function mergeEvents(current: ServerLogEvent[], incoming: ServerLogEvent[]) {
  const seen = new Set(current.map((event) => event.eventId));
  return [...current, ...incoming.filter((event) => !seen.has(event.eventId))];
}
