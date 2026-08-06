"use client";

import { Button, Modal, Tooltip } from "@heroui/react";
import { LoaderCircle, RefreshCw, RefreshCwOff, TriangleAlert } from "lucide-react";
import * as React from "react";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientLogs } from "@/lib/api";
import type { ClientLogsResponse, DashboardClient } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

const POLL_INTERVAL_MS = 10_000;
const MAX_ERROR_BACKOFF_MS = 30_000;
const BOTTOM_THRESHOLD_PX = 32;

type PendingScroll = { followBottom: boolean; scrollTop: number };

function clientLabel(client: DashboardClient | null) {
  return client?.clientTunnel?.subdomain || client?.installation.id || "";
}

export function ClientLogsDialog({
  client,
  open,
  onOpenChange,
}: {
  client: DashboardClient | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { t } = useLocaleText();
  const installationId = client?.installation.id || "";
  const [response, setResponse] = React.useState<ClientLogsResponse | null>(null);
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");
  const [autoRefresh, setAutoRefresh] = React.useState(true);
  const textareaRef = React.useRef<HTMLTextAreaElement>(null);
  const timerRef = React.useRef<number | null>(null);
  const abortRef = React.useRef<AbortController | null>(null);
  const generationRef = React.useRef(0);
  const failureCountRef = React.useRef(0);
  const inFlightRef = React.useRef(false);
  const autoRefreshRef = React.useRef(true);
  const fetchRef = React.useRef<() => void>(() => undefined);
  const pendingScrollRef = React.useRef<PendingScroll | null>(null);

  autoRefreshRef.current = autoRefresh;

  const clearTimer = React.useCallback(() => {
    if (timerRef.current == null) return;
    window.clearTimeout(timerRef.current);
    timerRef.current = null;
  }, []);

  const schedule = React.useCallback(
    (delayMs: number) => {
      clearTimer();
      if (!autoRefreshRef.current) return;
      timerRef.current = window.setTimeout(() => fetchRef.current(), delayMs);
    },
    [clearTimer],
  );

  const fetchLogs = React.useCallback(async () => {
    if (!open || !installationId || inFlightRef.current) return;
    clearTimer();
    const generation = generationRef.current;
    const controller = new AbortController();
    abortRef.current = controller;
    inFlightRef.current = true;
    setLoading(true);
    let nextDelay = POLL_INTERVAL_MS;

    try {
      const next = await getClientLogs(installationId, controller.signal);
      if (generation !== generationRef.current || controller.signal.aborted) return;
      const textarea = textareaRef.current;
      pendingScrollRef.current = {
        followBottom:
          !response ||
          !textarea ||
          textarea.scrollHeight - textarea.scrollTop - textarea.clientHeight <= BOTTOM_THRESHOLD_PX,
        scrollTop: textarea?.scrollTop || 0,
      };
      setResponse(next);
      setError("");
      failureCountRef.current = 0;
    } catch (reason) {
      if (generation !== generationRef.current || controller.signal.aborted) return;
      failureCountRef.current += 1;
      nextDelay = Math.min(
        POLL_INTERVAL_MS * 2 ** Math.max(0, failureCountRef.current - 1),
        MAX_ERROR_BACKOFF_MS,
      );
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      if (generation === generationRef.current) {
        inFlightRef.current = false;
        abortRef.current = null;
        setLoading(false);
        schedule(nextDelay);
      }
    }
  }, [clearTimer, installationId, open, response, schedule]);

  fetchRef.current = () => void fetchLogs();

  React.useEffect(() => {
    generationRef.current += 1;
    clearTimer();
    abortRef.current?.abort();
    abortRef.current = null;
    inFlightRef.current = false;
    failureCountRef.current = 0;
    setResponse(null);
    setError("");
    setLoading(false);
    setAutoRefresh(true);
    autoRefreshRef.current = true;
    pendingScrollRef.current = null;
    if (open && installationId) {
      fetchRef.current();
    }
    return () => {
      generationRef.current += 1;
      clearTimer();
      abortRef.current?.abort();
      abortRef.current = null;
      inFlightRef.current = false;
    };
  }, [clearTimer, installationId, open]);

  React.useLayoutEffect(() => {
    const textarea = textareaRef.current;
    const pending = pendingScrollRef.current;
    if (!textarea || !pending) return;
    textarea.scrollTop = pending.followBottom ? textarea.scrollHeight : pending.scrollTop;
    pendingScrollRef.current = null;
  }, [response?.content]);

  const toggleAutoRefresh = () => {
    if (autoRefresh) {
      autoRefreshRef.current = false;
      clearTimer();
      setAutoRefresh(false);
      return;
    }
    failureCountRef.current = 0;
    autoRefreshRef.current = true;
    setAutoRefresh(true);
    if (!inFlightRef.current) {
      clearTimer();
      fetchRef.current();
    }
  };

  const refreshActionLabel = autoRefresh ? t("clientLogs.pauseAutoRefresh") : t("clientLogs.resumeAutoRefresh");

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Container placement="center" size="lg">
        <Modal.Dialog className="light w-[min(900px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
          <Modal.Header>
            <div className="min-w-0 pr-10">
              <Modal.Heading className="!text-slate-900">{t("clientLogs.title")}</Modal.Heading>
              <p className="mt-1 truncate font-mono text-xs text-slate-500" title={clientLabel(client)}>
                {clientLabel(client)}
              </p>
            </div>
          </Modal.Header>
          <Modal.Body className="grid gap-3">
            <div className="flex min-h-8 flex-wrap items-center justify-between gap-2 text-xs text-slate-500">
              <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1">
                {response ? (
                  <>
                    <span>{response.fullLogAccess ? t("clientLogs.fullAccess") : t("clientLogs.publicAccess")}</span>
                    <span>{t("clientLogs.lineCount", { count: response.lines, limit: response.limit })}</span>
                    <span>{t("clientLogs.updatedAt", { time: formatDateTime(response.fetchedAt) })}</span>
                  </>
                ) : loading ? (
                  <span className="inline-flex items-center gap-1.5">
                    <LoaderCircle className="h-3.5 w-3.5 animate-spin" aria-hidden />
                    {t("clientLogs.loading")}
                  </span>
                ) : null}
              </div>
              <Tooltip>
                <Tooltip.Trigger>
                  <Button
                    isIconOnly
                    variant="ghost"
                    size="sm"
                    className={`h-8 w-8 min-w-8 rounded-md ${autoRefresh ? "text-slate-600" : "text-rose-600 hover:bg-rose-50 hover:text-rose-700"}`}
                    onClick={toggleAutoRefresh}
                    isDisabled={loading && autoRefresh}
                    aria-label={refreshActionLabel}
                    aria-pressed={!autoRefresh}
                  >
                    {autoRefresh ? (
                      <RefreshCw className={`h-4 w-4 ${loading ? "animate-spin" : ""}`} aria-hidden />
                    ) : (
                      <RefreshCwOff className="h-4 w-4" aria-hidden />
                    )}
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>{refreshActionLabel}</Tooltip.Content>
              </Tooltip>
            </div>

            {error ? (
              <div
                role="status"
                className="flex items-start gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900"
              >
                <TriangleAlert className="mt-0.5 h-3.5 w-3.5 shrink-0" aria-hidden />
                <span className="break-words">{error}</span>
              </div>
            ) : null}

            <div className="relative min-h-72">
              <textarea
                ref={textareaRef}
                value={response?.content || ""}
                readOnly
                spellCheck={false}
                aria-label={t("clientLogs.logText")}
                className="h-[min(58vh,34rem)] min-h-72 w-full resize-none overflow-auto rounded-md border border-slate-200 bg-white p-3 font-mono text-xs leading-5 text-slate-800 outline-none"
              />
              {!loading && !error && response && !response.content ? (
                <div className="pointer-events-none absolute inset-0 flex items-center justify-center text-sm text-slate-400">
                  {t("clientLogs.empty")}
                </div>
              ) : null}
            </div>
          </Modal.Body>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
