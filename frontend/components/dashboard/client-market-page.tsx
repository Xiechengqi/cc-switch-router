"use client";

import * as React from "react";
import { Button, Checkbox, Chip, Modal, toast, Tooltip } from "@heroui/react";
import { CheckSquare, ChevronLeft, ChevronRight, Download, Loader2, Plus, RefreshCw, Trash2, Upload } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CreateClientDialog } from "@/components/dashboard/create-client-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cleanupClientMarketClientWithReason,
  deleteClientMarketHost,
  exportMyClientMarketHosts,
  getClientMarketHosts,
  getClientMarketJob,
  getMyClientMarketBilling,
  importMyClientMarketHosts,
  reverifyClientMarketHost,
} from "@/lib/api";
import { mergeHosts } from "@/lib/client-market-refresh";
import type { ClientMarketBilling, ClientMarketHost, ClientMarketHostImportResponse } from "@/lib/types";
import { usePersistentState } from "@/lib/use-persistent-state";
import { useBatchOperations } from "@/components/dashboard/client-market/use-batch-operations";
import { AddHostDialog } from "@/components/dashboard/client-market/add-host-dialog";
import { HostRow } from "@/components/dashboard/client-market/host-row";
import { HostSortHeader } from "@/components/dashboard/client-market/host-sort-header";
import { ProviderBlocksPanel } from "@/components/dashboard/client-market/provider-blocks-panel";
import {
  BatchProgressItem,
  CLEARED_HOST_SORT,
  CLIENT_MARKET_POLL_MS,
  DEFAULT_HOST_SORT,
  HOST_LIST_TABS,
  HOST_PAGE_SIZE,
  HostListTab,
  HostSortKey,
  HostSortPrefs,
  OWNER_FILTER_KEY,
  PAYMENT_FILTER_KEY,
  PAYMENT_FILTER_KINDS,
  REGION_FILTER_KEY,
  ROUTER_OPEN_LOGIN_EVENT,
  SORT_PREFS_KEY,
  STATUS_FILTER_KEY,
  buildHostPageItems,
  cleanupReasonForHost,
  countBatchStatuses,
  encodeHostTransferDocument,
  hostBelongsToViewer,
  hostCanCleanup,
  hostCanDelete,
  hostCanExport,
  hostCanReverify,
  hostDisplayLabel,
  hostExportKey,
  hostMatchesListTab,
  hostStatusTabTone,
  hostSupportsPaymentKind,
  mapPool,
  normalizeHostListTab,
  normalizeHostSortPrefs,
  normalizeOwnerFilters,
  parseHostTransferLines,
  paymentKindLabelKey,
  prioritizeMineClientOwned,
  sortHosts,
  statusGroupForHost,
  statusGroupHintKey,
  statusGroupLabelKey,
} from "@/components/dashboard/client-market/host-utils";

export function ClientMarketPage() {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const viewerUserId = session?.user?.id;
  const viewerEmail = session?.user?.email;

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [billings, setBillings] = React.useState<ClientMarketBilling[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [addOpen, setAddOpen] = React.useState(false);
  const [pendingAddAfterLogin, setPendingAddAfterLogin] = React.useState(false);
  const [ownerFiltersRaw, setOwnerFiltersRaw] = usePersistentState<string[]>(OWNER_FILTER_KEY, []);
  const ownerFilters = React.useMemo(() => normalizeOwnerFilters(ownerFiltersRaw), [ownerFiltersRaw]);
  const setOwnerFilters = React.useCallback(
    (emails: string[]) => setOwnerFiltersRaw(normalizeOwnerFilters(emails)),
    [setOwnerFiltersRaw],
  );
  const [regionFilters, setRegionFilters] = usePersistentState<string[]>(REGION_FILTER_KEY, []);
  const [paymentFilters, setPaymentFilters] = usePersistentState<string[]>(PAYMENT_FILTER_KEY, []);
  const [listTabRaw, setListTab] = usePersistentState<HostListTab>(STATUS_FILTER_KEY, "mine");
  const [sortPrefsRaw, setSortPrefs] = usePersistentState<HostSortPrefs>(SORT_PREFS_KEY, DEFAULT_HOST_SORT);
  const sortPrefs = React.useMemo(() => normalizeHostSortPrefs(sortPrefsRaw), [sortPrefsRaw]);
  const listTab = normalizeHostListTab(listTabRaw, authed);
  const viewingMine = listTab === "mine";
  const [page, setPage] = React.useState(1);
  const [error, setError] = React.useState("");
  const [fixedHost, setFixedHost] = React.useState<ClientMarketHost | null>(null);
  const [transferBusy, setTransferBusy] = React.useState(false);
  const [importOpen, setImportOpen] = React.useState(false);
  const [exportOpen, setExportOpen] = React.useState(false);
  const [importText, setImportText] = React.useState("");
  const [exportText, setExportText] = React.useState("");
  const [importResult, setImportResult] = React.useState<ClientMarketHostImportResponse | null>(null);
  const [rowUiBusyCount, setRowUiBusyCount] = React.useState(0);
  const [focusInstallationId, setFocusInstallationId] = React.useState<string | null>(null);
  const refreshAbortRef = React.useRef<AbortController | null>(null);
  const rowBusyIdsRef = React.useRef<Set<string>>(new Set());
  const focusAppliedRef = React.useRef(false);

  const setRowUiBusy = React.useCallback((hostId: string, busy: boolean) => {
    const ids = rowBusyIdsRef.current;
    const before = ids.size;
    if (busy) ids.add(hostId);
    else ids.delete(hostId);
    if (ids.size !== before) setRowUiBusyCount(ids.size);
  }, []);


  const load = React.useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent === true;
      // Silent polls never interrupt each other or a visible load.
      if (silent && refreshAbortRef.current) return;
      if (!silent) refreshAbortRef.current?.abort();
      const controller = new AbortController();
      refreshAbortRef.current = controller;
      if (!silent) {
        setLoading(true);
        setError("");
      }
      try {
        const nextHosts = await getClientMarketHosts(undefined, controller.signal);
        if (controller.signal.aborted) return;
        setHosts((prev) => mergeHosts(prev, nextHosts));
        if (authed) {
          try {
            const nextBilling = await getMyClientMarketBilling(controller.signal);
            if (!controller.signal.aborted) setBillings(nextBilling);
          } catch {
            if (!controller.signal.aborted && !silent) {
              /* hosts still usable; renter actions degrade until next poll */
            }
          }
        } else {
          setBillings([]);
        }
      } catch (err) {
        if (controller.signal.aborted) return;
        if (!silent) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (refreshAbortRef.current === controller) refreshAbortRef.current = null;
        if (!silent) setLoading(false);
      }
    },
    [authed],
  );

  const silentRefresh = React.useCallback(() => load({ silent: true }), [load]);

  React.useEffect(() => {
    void load();
    return () => {
      refreshAbortRef.current?.abort();
    };
  }, [load, viewerUserId]);

  const ownerOptions = React.useMemo(() => {
    const emails = Array.from(new Set(hosts.map((host) => host.hostOwnerEmail))).sort((a, b) =>
      a.localeCompare(b),
    );
    return emails.map((email) => ({ value: email, label: email }));
  }, [hosts]);

  const regionOptions = React.useMemo(() => {
    const regionNames = new Intl.DisplayNames([locale], { type: "region" });
    const codes = Array.from(
      new Set(
        hosts
          .map((host) => (host.countryCode || "").trim().toUpperCase())
          .filter(Boolean),
      ),
    ).sort((a, b) => a.localeCompare(b));
    return codes.map((code) => ({
      value: code,
      label: regionNames.of(code) || code,
    }));
  }, [hosts, locale]);

  const paymentOptions = React.useMemo(
    () =>
      PAYMENT_FILTER_KINDS.map((kind) => ({
        value: kind,
        label: t(paymentKindLabelKey(kind)),
      })),
    [t],
  );

  const scopedHosts = React.useMemo(() => {
    const ownerSet = new Set(ownerFilters.map((email) => email.toLowerCase()));
    const regionSet = new Set(regionFilters.map((code) => code.toUpperCase()));
    return hosts.filter((host) => {
      if (ownerSet.size > 0 && !ownerSet.has(host.hostOwnerEmail.toLowerCase())) return false;
      if (regionSet.size > 0) {
        const code = (host.countryCode || "").trim().toUpperCase();
        if (!code || !regionSet.has(code)) return false;
      }
      if (
        paymentFilters.length > 0 &&
        !paymentFilters.every((kind) => hostSupportsPaymentKind(host.paymentMethodKinds, kind))
      ) {
        return false;
      }
      return true;
    });
  }, [hosts, ownerFilters, paymentFilters, regionFilters]);

  const mineHosts = React.useMemo(
    () => (authed ? scopedHosts.filter(hostBelongsToViewer) : []),
    [authed, scopedHosts],
  );

  const statusCounts = React.useMemo(() => {
    const counts: Record<HostListTab, number> = {
      mine: mineHosts.length,
      all: scopedHosts.length,
      idle: 0,
      in_use: 0,
      needs_attention: 0,
    };
    for (const host of scopedHosts) {
      const group = statusGroupForHost(host.status);
      if (group) counts[group] += 1;
    }
    return counts;
  }, [mineHosts.length, scopedHosts]);

  const billingByInstallation = React.useMemo(() => {
    const map = new Map<string, ClientMarketBilling>();
    for (const billing of billings) {
      if (!billing.isClientOwner) continue;
      map.set(billing.installationId, billing);
    }
    return map;
  }, [billings]);

  const visibleHosts = React.useMemo(() => {
    const filtered = scopedHosts.filter((host) => hostMatchesListTab(host, listTab));
    const sorted = sortHosts(filtered, sortPrefs);
    return listTab === "mine" ? prioritizeMineClientOwned(sorted) : sorted;
  }, [listTab, scopedHosts, sortPrefs]);

  const toggleHostSort = React.useCallback((key: HostSortKey) => {
    setSortPrefs((prev) => {
      const current = normalizeHostSortPrefs(prev);
      if (current.key !== key) return { key, dir: "asc" };
      if (current.dir === "asc") return { key, dir: "desc" };
      return CLEARED_HOST_SORT;
    });
  }, [setSortPrefs]);

  const totalPages = Math.max(1, Math.ceil(visibleHosts.length / HOST_PAGE_SIZE));
  const safePage = Math.min(page, totalPages);
  const pagedHosts = React.useMemo(() => {
    const start = (safePage - 1) * HOST_PAGE_SIZE;
    return visibleHosts.slice(start, start + HOST_PAGE_SIZE);
  }, [safePage, visibleHosts]);

  const batch = useBatchOperations({
    hosts,
    visibleHosts,
    pagedHosts,
    viewerEmail,
    authed,
    onChanged: () => void silentRefresh(),
  });

  // Background polling must not yank state out from under an open dialog, an
  // in-flight bulk run, or a row the user is interacting with.
  const refreshPaused =
    batch.uiBusy ||
    transferBusy ||
    addOpen ||
    importOpen ||
    exportOpen ||
    !!fixedHost ||
    rowUiBusyCount > 0;

  React.useEffect(() => {
    const tick = () => {
      if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
      if (refreshPaused) return;
      void silentRefresh();
    };
    const timer = window.setInterval(tick, CLIENT_MARKET_POLL_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") tick();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [refreshPaused, silentRefresh]);


  React.useEffect(() => {
    setPage(1);
  }, [ownerFilters, paymentFilters, regionFilters, sortPrefs.key, sortPrefs.dir, listTab]);

  React.useEffect(() => {
    if (!authed && listTabRaw === "mine") setListTab("all");
  }, [authed, listTabRaw, setListTab]);

  React.useEffect(() => {
    if (typeof window === "undefined" || focusAppliedRef.current) return;
    const params = new URLSearchParams(window.location.search);
    const tab = params.get("tab");
    const focus = params.get("focus");
    if (tab === "mine" && authed) setListTab("mine");
    if (focus) {
      setFocusInstallationId(focus);
      focusAppliedRef.current = true;
    }
  }, [authed, setListTab]);

  React.useEffect(() => {
    if (!focusInstallationId || loading) return;
    const index = visibleHosts.findIndex((host) => host.installationId === focusInstallationId);
    if (index < 0) return;
    const targetPage = Math.floor(index / HOST_PAGE_SIZE) + 1;
    if (targetPage !== page) setPage(targetPage);
    const timer = window.setTimeout(() => {
      document
        .getElementById(`client-market-host-${focusInstallationId}`)
        ?.scrollIntoView({ behavior: "smooth", block: "center" });
    }, 80);
    return () => window.clearTimeout(timer);
  }, [focusInstallationId, loading, page, visibleHosts]);

  React.useEffect(() => {
    if (page > totalPages) setPage(totalPages);
  }, [page, totalPages]);

  React.useEffect(() => {
    if (!pendingAddAfterLogin || !authed) return;
    setPendingAddAfterLogin(false);
    setAddOpen(true);
  }, [authed, pendingAddAfterLogin]);

  const openAddHost = () => {
    if (!authed) {
      setPendingAddAfterLogin(true);
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    setAddOpen(true);
  };

  const openExportDialog = async (selectedOnly: boolean) => {
    setTransferBusy(true);
    try {
      const document = await exportMyClientMarketHosts();
      if (selectedOnly) {
        const keys = new Set(batch.exportEligible.map((host) => hostExportKey(host)).filter(Boolean));
        if (!keys.size) {
          toast.danger(t("clientMarket.batchExportEmpty"));
          return;
        }
        document.hosts = document.hosts.filter((entry) => keys.has(hostExportKey(entry)));
        if (!document.hosts.length) {
          toast.danger(t("clientMarket.batchExportEmpty"));
          return;
        }
      }
      if (!document.hosts.length) {
        toast.danger(t("clientMarket.exportEmpty"));
        return;
      }
      setExportText(encodeHostTransferDocument(document));
      setExportOpen(true);
      if (selectedOnly) batch.clearSelection();
      toast.success(t("clientMarket.exportedHosts", { count: document.hosts.length }));
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setTransferBusy(false);
    }
  };

  const copyExportText = async () => {
    try {
      await navigator.clipboard.writeText(exportText);
      toast.success(t("clientMarket.exportCopied"));
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    }
  };


  const submitImportText = async () => {
    if (new TextEncoder().encode(importText).length > 1024 * 1024) {
      toast.danger(t("clientMarket.importSizeLimit"));
      return;
    }
    const parsed = parseHostTransferLines(importText);
    if (parsed.errorLine) {
      toast.danger(t("clientMarket.importParseError", { line: parsed.errorLine }));
      return;
    }
    if (!parsed.document) {
      toast.danger(t("clientMarket.importEmpty"));
      return;
    }
    setTransferBusy(true);
    try {
      const result = await importMyClientMarketHosts(parsed.document);
      setImportOpen(false);
      setImportText("");
      setImportResult(result);
      await silentRefresh();
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setTransferBusy(false);
    }
  };

  const statusTabs = React.useMemo(
    () =>
      HOST_LIST_TABS.filter((value) => value !== "mine" || authed).map((value) => ({
        value,
        label: t(statusGroupLabelKey(value)),
        hint: t(statusGroupHintKey(value)),
        count: statusCounts[value],
      })),
    [authed, statusCounts, t],
  );

  return (
    <div className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl grid-cols-[minmax(0,1fr)] gap-5 pb-10">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <div className="inline-flex max-w-full overflow-x-auto rounded-lg bg-slate-100 p-1 text-[11px]">
            {statusTabs.map((tab) => (
              <button
                key={tab.value}
                type="button"
                title={tab.hint}
                aria-label={`${tab.label}. ${tab.hint}`}
                onClick={() => setListTab(tab.value)}
                className={`rounded-md px-2.5 py-1.5 transition-colors ${hostStatusTabTone(tab.value, listTab === tab.value)}`}
              >
                {tab.label} · {tab.count}
              </button>
            ))}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {authed ? (
            <>
              {batch.selectionMode ? (
                <Button variant="outline" size="sm" className="h-8" isDisabled={batch.batchBusy} onClick={batch.exitSelectionMode}>
                  <CheckSquare className="h-4 w-4" />
                  {t("clientMarket.batchDoneSelection")}
                </Button>
              ) : (
                <Button
                  variant="outline"
                  size="sm"
                  className="h-8"
                  isDisabled={batch.batchBusy || !visibleHosts.length}
                  onClick={batch.enterSelectionMode}
                >
                  <CheckSquare className="h-4 w-4" />
                  {t("clientMarket.batchEnterSelection")}
                </Button>
              )}
              <Tooltip>
                <Tooltip.Trigger>
                  <Button
                    variant="outline"
                    size="sm"
                    isIconOnly
                    className="h-8 w-8 min-w-8"
                    aria-label={t("clientMarket.importMyHosts")}
                    isDisabled={transferBusy}
                    onClick={() => setImportOpen(true)}
                  >
                    <Upload className="h-4 w-4" />
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>{t("clientMarket.importMyHosts")}</Tooltip.Content>
              </Tooltip>
              <Tooltip>
                <Tooltip.Trigger>
                  <Button
                    variant="outline"
                    size="sm"
                    isIconOnly
                    className="h-8 w-8 min-w-8"
                    aria-label={t("clientMarket.exportMyHosts")}
                    isDisabled={transferBusy || batch.batchBusy}
                    onClick={() => void openExportDialog(false)}
                  >
                    {transferBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Download className="h-4 w-4" />}
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>{t("clientMarket.exportMyHosts")}</Tooltip.Content>
              </Tooltip>
            </>
          ) : null}
          <Button variant="primary" size="sm" className="h-8" onClick={openAddHost}>
            <Plus className="h-4 w-4" />
            {t("clientMarket.addHost")}
          </Button>
        </div>
      </div>

      {batch.selectionMode ? (
        <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-white px-3 py-2 text-sm">
          <span className="font-medium text-foreground">{t("clientMarket.batchSelected", { count: batch.selectedCount })}</span>
          <Button variant="outline" size="sm" isDisabled={batch.batchBusy || !visibleHosts.length} onClick={batch.selectAllFiltered}>
            {t("clientMarket.batchSelectAll")}
          </Button>
          <Button variant="ghost" size="sm" isDisabled={batch.batchBusy || !pagedHosts.length} onClick={batch.selectPage}>
            {t("clientMarket.batchSelectPage")}
          </Button>
          <Button variant="ghost" size="sm" isDisabled={batch.batchBusy || batch.selectedCount === 0} onClick={batch.clearSelection}>
            {t("clientMarket.batchClear")}
          </Button>
          <span className="mx-1 h-4 w-px bg-border" aria-hidden />
          <Button
            variant="outline"
            size="sm"
            isDisabled={batch.batchBusy || batch.cleanupEligible.length === 0}
            aria-label={t("clientMarket.batchActionAria", { action: t("clientMarket.cleanup"), run: batch.cleanupEligible.length, selected: batch.selectedCount })}
            onClick={() => {
              if (!batch.cleanupEligible.length) {
                toast.info(t("clientMarket.batchNothingEligible"));
                return;
              }
              batch.requestBatch("cleanup");
            }}
          >
            {t("clientMarket.cleanup")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: batch.cleanupEligible.length, selected: batch.selectedCount })}
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            isDisabled={batch.batchBusy || batch.reverifyEligible.length === 0}
            aria-label={t("clientMarket.batchActionAria", { action: t("clientMarket.reverifyHost"), run: batch.reverifyEligible.length, selected: batch.selectedCount })}
            onClick={() => {
              if (!batch.reverifyEligible.length) {
                toast.info(t("clientMarket.batchNothingEligible"));
                return;
              }
              batch.requestBatch("reverify");
            }}
          >
            <RefreshCw className="h-4 w-4" />
            {t("clientMarket.reverifyHost")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: batch.reverifyEligible.length, selected: batch.selectedCount })}
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="text-destructive"
            isDisabled={batch.batchBusy || batch.deleteEligible.length === 0}
            aria-label={t("clientMarket.batchActionAria", { action: t("clientMarket.deleteHost"), run: batch.deleteEligible.length, selected: batch.selectedCount })}
            onClick={() => {
              if (!batch.deleteEligible.length) {
                toast.info(t("clientMarket.batchNothingEligible"));
                return;
              }
              batch.requestBatch("delete");
            }}
          >
            <Trash2 className="h-4 w-4" />
            {t("clientMarket.deleteHost")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: batch.deleteEligible.length, selected: batch.selectedCount })}
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            isDisabled={transferBusy || batch.batchBusy || batch.exportEligible.length === 0}
            aria-label={t("clientMarket.batchActionAria", { action: t("clientMarket.batchExportSelected"), run: batch.exportEligible.length, selected: batch.selectedCount })}
            onClick={() => void openExportDialog(true)}
          >
            <Download className="h-4 w-4" />
            {t("clientMarket.batchExportSelected")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: batch.exportEligible.length, selected: batch.selectedCount })}
            </span>
          </Button>
        </div>
      ) : null}

      {!authed ? (
        <p className="text-sm text-muted-foreground">{t("clientMarket.loginToAddHost")}</p>
      ) : null}

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          …
        </div>
      ) : error ? (
        <p className="text-sm text-rose-600">{error}</p>
      ) : visibleHosts.length === 0 ? (
        <div className="grid justify-items-center gap-2 rounded-lg border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
          <span>
            {viewingMine
              ? !ownerFilters.length && !regionFilters.length && !paymentFilters.length
                ? t("clientMarket.scopeMineEmpty")
                : t("dashboard.noFilterResults")
              : scopedHosts.length || ownerFilters.length || regionFilters.length || paymentFilters.length
                ? t("dashboard.noFilterResults")
                : t("clientMarket.noHosts")}
          </span>
          {viewingMine &&
          !mineHosts.length &&
          !ownerFilters.length &&
          !regionFilters.length &&
          !paymentFilters.length ? (
            <button
              type="button"
              className="text-xs font-medium text-primary hover:underline"
              onClick={() => setListTab("all")}
            >
              {t("clientMarket.scopeMineEmptyAction")}
            </button>
          ) : null}
          {ownerFilters.length ||
          regionFilters.length ||
          paymentFilters.length ||
          listTab !== (authed ? "mine" : "all") ? (
            <button
              type="button"
              className="text-xs font-medium text-primary hover:underline"
              onClick={() => {
                setListTab(authed ? "mine" : "all");
                setOwnerFilters([]);
                setRegionFilters([]);
                setPaymentFilters([]);
              }}
            >
              {t("dashboard.clearFilters")}
            </button>
          ) : null}
        </div>
      ) : (
        <div className="overflow-hidden rounded-xl border border-border bg-card shadow-sm">
          <div className="max-h-[min(70vh,40rem)] overflow-auto">
            <table className="w-full min-w-[56rem] border-collapse text-sm">
              <caption className="sr-only">{t("clientMarket.hostsTableCaption")}</caption>
              <thead>
                <tr>
                  {batch.selectionMode ? (
                    <th
                      scope="col"
                      className="sticky top-0 z-10 w-10 border-b border-border bg-card px-2 py-2 text-left"
                    >
                      <Checkbox
                        isSelected={
                          pagedHosts.length > 0 && pagedHosts.every((host) => batch.selectedIds.has(host.id))
                        }
                        isIndeterminate={
                          pagedHosts.some((host) => batch.selectedIds.has(host.id)) &&
                          !pagedHosts.every((host) => batch.selectedIds.has(host.id))
                        }
                        onChange={(checked) => {
                          if (checked) batch.selectPage();
                          else batch.deselectPage();
                        }}
                        isDisabled={batch.batchBusy || !pagedHosts.length}
                        aria-label={t("clientMarket.batchSelectPage")}
                        className="shrink-0"
                      >
                        <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                          <Checkbox.Indicator />
                        </Checkbox.Control>
                      </Checkbox>
                    </th>
                  ) : null}
                  <HostSortHeader columnKey="status" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader
                    columnKey="region"
                    sortPrefs={sortPrefs}
                    onSort={toggleHostSort}
                    filter={
                      <CompactRegionMultiSelect
                        variant="header"
                        columnLabel={t("clientMarket.col.region")}
                        values={regionFilters}
                        onChange={setRegionFilters}
                        options={regionOptions}
                        allLabel={t("clientMarket.allRegions")}
                        moreLabel={(count) => t("clientMarket.regionsMore", { count })}
                        clearLabel={t("clientMarket.clearRegionSelection")}
                        ariaLabel={t("clientMarket.filterRegions")}
                        className="w-full max-w-[10.5rem]"
                      />
                    }
                  />
                  <HostSortHeader
                    columnKey="owner"
                    sortPrefs={sortPrefs}
                    onSort={toggleHostSort}
                    filter={
                      <CompactRegionMultiSelect
                        variant="header"
                        columnLabel={t("clientMarket.col.owner")}
                        values={ownerFilters}
                        onChange={setOwnerFilters}
                        options={ownerOptions}
                        allLabel={t("clientMarket.allOwners")}
                        moreLabel={(count) => t("clientMarket.ownersMore", { count })}
                        clearLabel={t("clientMarket.clearOwnerSelection")}
                        ariaLabel={t("clientMarket.filterOwners")}
                        className="w-full max-w-[12rem]"
                      />
                    }
                  />
                  <HostSortHeader
                    columnKey="offer"
                    sortPrefs={sortPrefs}
                    onSort={toggleHostSort}
                    filter={
                      <CompactRegionMultiSelect
                        variant="header"
                        columnLabel={t("clientMarket.col.offer")}
                        values={paymentFilters}
                        onChange={setPaymentFilters}
                        options={paymentOptions}
                        allLabel={t("clientMarket.allPayments")}
                        moreLabel={(count) => t("clientMarket.paymentsMore", { count })}
                        clearLabel={t("clientMarket.clearPaymentSelection")}
                        ariaLabel={t("clientMarket.filterPayments")}
                        className="w-full max-w-[10.5rem]"
                      />
                    }
                  />
                  <HostSortHeader columnKey="subdomain" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader columnKey="ip" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <th
                    scope="col"
                    className="sticky top-0 z-10 whitespace-nowrap border-b border-border bg-card px-2 py-2 text-right text-xs font-medium text-muted-foreground"
                  >
                    {t("clientMarket.col.actions")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {pagedHosts.map((host) => (
                  <HostRow
                    key={host.id}
                    host={host}
                    billing={
                      host.installationId
                        ? billingByInstallation.get(host.installationId) ?? null
                        : null
                    }
                    highlighted={
                      !!focusInstallationId && host.installationId === focusInstallationId
                    }
                    selectionMode={batch.selectionMode}
                    selected={batch.selectedIds.has(host.id)}
                    onSelectedChange={batch.setHostSelected}
                    selectionDisabled={batch.batchBusy}
                    onChanged={silentRefresh}
                    onCreate={setFixedHost}
                    onUiBusyChange={setRowUiBusy}
                  />
                ))}
              </tbody>
            </table>
          </div>
          {visibleHosts.length > HOST_PAGE_SIZE ? (
            <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t border-border bg-muted/30 px-3 py-2.5">
              <p className="text-xs text-muted-foreground">
                {t("clientMarket.paginationSummary", {
                  start: (safePage - 1) * HOST_PAGE_SIZE + 1,
                  end: Math.min(safePage * HOST_PAGE_SIZE, visibleHosts.length),
                  total: visibleHosts.length,
                })}
              </p>
              <nav className="flex items-center gap-1" aria-label={t("clientMarket.paginationPage", { page: safePage, pages: totalPages })}>
                <button
                  type="button"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-35"
                  disabled={safePage <= 1}
                  aria-label={t("clientMarket.paginationPrev")}
                  onClick={() => setPage((current) => Math.max(1, current - 1))}
                >
                  <ChevronLeft className="h-4 w-4" />
                </button>
                {buildHostPageItems(safePage, totalPages).map((item, index) =>
                  item === "ellipsis" ? (
                    <span
                      key={`ellipsis-${index}`}
                      className="inline-flex h-8 w-6 items-center justify-center text-xs text-muted-foreground/60"
                      aria-hidden
                    >
                      …
                    </span>
                  ) : (
                    <button
                      key={item}
                      type="button"
                      aria-label={t("clientMarket.paginationGoTo", { page: item })}
                      aria-current={item === safePage ? "page" : undefined}
                      className={
                        item === safePage
                          ? "inline-flex h-8 min-w-8 items-center justify-center rounded-lg bg-accent px-2 text-xs font-medium text-accent-foreground shadow-sm shadow-accent/20"
                          : "inline-flex h-8 min-w-8 items-center justify-center rounded-lg px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                      }
                      onClick={() => setPage(item)}
                    >
                      {item}
                    </button>
                  ),
                )}
                <button
                  type="button"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-35"
                  disabled={safePage >= totalPages}
                  aria-label={t("clientMarket.paginationNext")}
                  onClick={() => setPage((current) => Math.min(totalPages, current + 1))}
                >
                  <ChevronRight className="h-4 w-4" />
                </button>
              </nav>
            </div>
          ) : null}
        </div>
      )}

      <AddHostDialog open={addOpen} onOpenChange={setAddOpen} onAdded={() => void silentRefresh()} />
      <CreateClientDialog
        open={!!fixedHost}
        onOpenChange={(next) => { if (!next) setFixedHost(null); }}
        fixedHost={fixedHost}
        onCreated={() => void silentRefresh()}
      />
      <Modal.Backdrop
        isOpen={importOpen}
        onOpenChange={(next) => {
          if (!next && !transferBusy) {
            setImportOpen(false);
          }
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("clientMarket.importDialogTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p className="text-xs leading-relaxed text-muted-foreground">{t("clientMarket.transferFormatHint")}</p>
              <textarea
                value={importText}
                onChange={(event) => setImportText(event.target.value)}
                placeholder={t("clientMarket.importPlaceholder")}
                spellCheck={false}
                className="min-h-56 w-full resize-y rounded-lg border border-border bg-white px-3 py-2 font-mono text-xs leading-5 text-foreground outline-none focus-visible:ring-2 focus-visible:ring-accent"
              />
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={transferBusy} onClick={() => setImportOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button variant="primary" isDisabled={transferBusy} onClick={() => void submitImportText()}>
                {transferBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("clientMarket.importSubmit")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <Modal.Backdrop
        isOpen={exportOpen}
        onOpenChange={(next) => {
          if (!next) setExportOpen(false);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("clientMarket.exportDialogTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p className="text-xs leading-relaxed text-muted-foreground">{t("clientMarket.transferFormatHint")}</p>
              <textarea
                value={exportText}
                readOnly
                spellCheck={false}
                className="min-h-56 w-full resize-y rounded-lg border border-border bg-muted/30 px-3 py-2 font-mono text-xs leading-5 text-foreground outline-none"
              />
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" onClick={() => setExportOpen(false)}>
                {t("common.close")}
              </Button>
              <Button variant="outline" onClick={() => void copyExportText()}>
                {t("clientMarket.exportCopy")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <Modal.Backdrop isOpen={!!importResult} onOpenChange={(next) => { if (!next) setImportResult(null); }}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("clientMarket.importResult")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid max-h-[65vh] gap-3 overflow-y-auto">
              {importResult ? <div className="flex flex-wrap gap-2 text-sm"><Chip size="sm" variant="soft">{t("clientMarket.importedCount", { count: importResult.imported })}</Chip><Chip size="sm" variant="soft">{t("clientMarket.skippedCount", { count: importResult.skipped })}</Chip><Chip size="sm" variant="soft">{t("clientMarket.failedCount", { count: importResult.failed })}</Chip></div> : null}
              <div className="grid gap-1.5">{importResult?.items.map((item) => <div key={`${item.ip}:${item.port}`} className="grid grid-cols-[minmax(0,1fr)_auto] gap-3 rounded-md border px-3 py-2 text-xs"><span className="min-w-0 truncate font-mono">{item.ip}:{item.port}</span><span className={item.status === "failed" ? "text-rose-600" : item.status === "imported" ? "text-emerald-700" : "text-muted-foreground"}>{item.error || item.status}</span></div>)}</div>
            </Modal.Body>
            <Modal.Footer><Button variant="outline" onClick={() => setImportResult(null)}>{t("common.close")}</Button></Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      {batch.confirmCopy ? (
        <ConfirmAlertDialog
          open
          title={batch.confirmCopy.title}
          description={batch.confirmCopy.description}
          confirmLabel={batch.confirmCopy.confirmLabel}
          cancelLabel={t("common.cancel")}
          tone="danger"
          busy={batch.batchBusy}
          onConfirm={() => {
            batch.requestBatch(null);
            batch.confirmCopy?.run();
          }}
          onOpenChange={(nextOpen) => {
            if (!nextOpen && !batch.batchBusy) batch.requestBatch(null);
          }}
        />
      ) : null}

      <Modal.Backdrop
        isOpen={batch.progressOpen}
        onOpenChange={(next) => {
          if (!next && !batch.batchBusy) batch.closeProgress();
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>
                {t("clientMarket.batchProgressTitle", { action: batch.progressLabel })}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[65vh] gap-2 overflow-y-auto">
              {batch.progressItems.map((item) => (
                <div
                  key={item.hostId}
                  className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-3 rounded-md border px-3 py-2 text-xs"
                >
                  <div className="min-w-0">
                    <div className="truncate font-medium text-foreground">{item.label}</div>
                    {item.detail ? (
                      <div className="mt-0.5 whitespace-normal break-words text-muted-foreground">{item.detail}</div>
                    ) : null}
                  </div>
                  <span
                    className={
                      item.status === "failed"
                        ? "text-rose-600"
                        : item.status === "succeeded"
                          ? "text-emerald-700"
                          : item.status === "running"
                            ? "text-primary"
                            : "text-muted-foreground"
                    }
                  >
                    {item.status === "queued"
                      ? t("clientMarket.batchStatus.queued")
                      : item.status === "running"
                        ? t("clientMarket.batchStatus.running")
                        : item.status === "succeeded"
                          ? t("clientMarket.batchStatus.succeeded")
                          : item.status === "failed"
                            ? t("clientMarket.batchStatus.failed")
                            : t("clientMarket.batchStatus.skipped")}
                  </span>
                </div>
              ))}
            </Modal.Body>
            <Modal.Footer>
              <Button
                variant="outline"
                isDisabled={batch.batchBusy}
                onClick={() => batch.closeProgress()}
              >
                {batch.batchBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("common.close")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <ProviderBlocksPanel
        enabled={authed}
        hosting={hosts.some((host) => hostBelongsToViewer(host) || host.isHostOwner === true)}
      />
    </div>
  );
}
