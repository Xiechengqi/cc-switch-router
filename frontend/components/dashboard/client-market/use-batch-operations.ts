"use client";

import * as React from "react";
import { toast } from "@heroui/react";
import {
  cleanupClientMarketProviderRental,
  deleteClientMarketHost,
  getClientMarketJob,
  reverifyClientMarketHost,
} from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  BatchProgressItem,
  cleanupReasonForHost,
  countBatchStatuses,
  hostCanCleanup,
  hostCanDelete,
  hostCanExport,
  hostCanReverify,
  hostDisplayLabel,
  mapPool,
} from "@/components/dashboard/client-market/host-utils";
import type { ClientMarketHost } from "@/lib/types";

export type BatchAction = "cleanup" | "delete" | "reverify";

export type BatchConfirmCopy = {
  title: string;
  description: string;
  confirmLabel: string;
  run: () => void;
};

/**
 * Selection mode and bulk host operations.
 *
 * This is roughly 300 lines of state, eligibility maths, concurrency-limited
 * execution and progress bookkeeping that previously sat inline in the Client Market
 * page, interleaved with data loading, filtering, sorting and pagination. It is
 * self-contained: everything it needs arrives as arguments, and the page consumes
 * only the returned surface.
 */
export function useBatchOperations({
  hosts,
  visibleHosts,
  pagedHosts,
  viewerEmail,
  authed,
  onChanged,
}: {
  hosts: ClientMarketHost[];
  visibleHosts: ClientMarketHost[];
  pagedHosts: ClientMarketHost[];
  viewerEmail?: string | null;
  authed: boolean;
  onChanged: () => void;
}) {
  const { t } = useLocaleText();

  const [selectionMode, setSelectionMode] = React.useState(false);
  const [selectedIds, setSelectedIds] = React.useState<Set<string>>(new Set());
  const [batchBusy, setBatchBusy] = React.useState(false);
  const [batchConfirm, setBatchConfirm] = React.useState<BatchAction | null>(null);
  const [progressOpen, setProgressOpen] = React.useState(false);
  const [progressAction, setProgressAction] = React.useState<BatchAction>("cleanup");
  const [progressItems, setProgressItems] = React.useState<BatchProgressItem[]>([]);

  // Drop selections that left the current filter set (or disappeared from hosts).
  React.useEffect(() => {
    if (!selectionMode) return;
    const visibleIds = new Set(visibleHosts.map((host) => host.id));
    setSelectedIds((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const id of prev) {
        if (visibleIds.has(id)) next.add(id);
        else changed = true;
      }
      return changed || next.size !== prev.size ? next : prev;
    });
  }, [selectionMode, visibleHosts]);

  React.useEffect(() => {
    if (!authed && selectionMode) {
      setSelectionMode(false);
      setSelectedIds(new Set());
    }
  }, [authed, selectionMode]);

  const selectedHosts = React.useMemo(
    () => hosts.filter((host) => selectedIds.has(host.id)),
    [hosts, selectedIds],
  );
  const selectedCount = selectedIds.size;

  const cleanupEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanCleanup(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );
  const reverifyEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanReverify(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );
  const deleteEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanDelete(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );
  const exportEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanExport(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );

  const setHostSelected = React.useCallback((hostId: string, selected: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (selected) next.add(hostId);
      else next.delete(hostId);
      return next;
    });
  }, []);

  const enterSelectionMode = React.useCallback(() => setSelectionMode(true), []);
  const exitSelectionMode = React.useCallback(() => {
    setSelectionMode(false);
    setSelectedIds(new Set());
  }, []);
  const selectPage = React.useCallback(() => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      for (const host of pagedHosts) next.add(host.id);
      return next;
    });
  }, [pagedHosts]);
  const selectAllFiltered = React.useCallback(() => {
    setSelectedIds(new Set(visibleHosts.map((host) => host.id)));
  }, [visibleHosts]);
  const deselectPage = React.useCallback(() => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      for (const host of pagedHosts) next.delete(host.id);
      return next;
    });
  }, [pagedHosts]);
  const clearSelection = React.useCallback(() => setSelectedIds(new Set()), []);

  const finishBatch = React.useCallback(
    (items: BatchProgressItem[]) => {
      const summary = countBatchStatuses(items);
      toast[summary.failed > 0 ? "danger" : summary.succeeded > 0 ? "success" : "info"](
        t("clientMarket.batchSummary", summary),
      );
      if (summary.failed > 0) {
        // Keep the failures selected so a retry does not require re-picking them.
        setSelectionMode(true);
        setSelectedIds(
          new Set(items.filter((item) => item.status === "failed").map((item) => item.hostId)),
        );
      } else {
        setSelectionMode(false);
        setSelectedIds(new Set());
      }
      onChanged();
    },
    [onChanged, t],
  );

  const beginProgress = React.useCallback(
    (action: BatchAction, targets: ClientMarketHost[], skippedHosts: ClientMarketHost[]) => {
      const items: BatchProgressItem[] = [
        ...targets.map((host) => ({
          hostId: host.id,
          label: hostDisplayLabel(host),
          status: "queued" as const,
        })),
        ...skippedHosts.map((host) => ({
          hostId: host.id,
          label: hostDisplayLabel(host),
          status: "skipped" as const,
        })),
      ];
      const byId = new Map(items.map((item) => [item.hostId, item]));
      setProgressAction(action);
      setProgressItems(items);
      setProgressOpen(true);
      const patch = (hostId: string, next: Partial<BatchProgressItem>) => {
        const current = byId.get(hostId);
        if (!current) return;
        const updated = { ...current, ...next };
        byId.set(hostId, updated);
        setProgressItems(Array.from(byId.values()));
      };
      return { byId, patch };
    },
    [],
  );

  const pollCleanupJobQuiet = React.useCallback(
    async (jobId: string) => {
      for (let i = 0; i < 180; i++) {
        await new Promise((r) => setTimeout(r, 1200));
        try {
          const latest = await getClientMarketJob(jobId);
          if (latest.status === "succeeded") return { ok: true as const };
          if (latest.status === "failed") {
            const detail =
              latest.failureCode || latest.log.split("\n").filter(Boolean).at(-1) || "";
            return { ok: false as const, detail };
          }
        } catch {
          continue;
        }
      }
      return { ok: false as const, detail: t("clientMarket.cleanupTimedOut") };
    },
    [t],
  );

  const runBatchCleanup = React.useCallback(async () => {
    const targets = cleanupEligible;
    const skippedHosts = selectedHosts.filter((host) => !hostCanCleanup(host, viewerEmail));
    const { byId, patch } = beginProgress("cleanup", targets, skippedHosts);
    setBatchBusy(true);
    try {
      await mapPool(targets, 2, async (host) => {
        patch(host.id, { status: "running" });
        if (!host.installationId) {
          patch(host.id, { status: "skipped" });
          return;
        }
        try {
          const { jobId } = await cleanupClientMarketProviderRental(host.installationId, {
            reason: cleanupReasonForHost(host),
            denyClientAccess: false,
          });
          const result = await pollCleanupJobQuiet(jobId);
          if (result.ok) patch(host.id, { status: "succeeded" });
          else patch(host.id, { status: "failed", detail: result.detail });
        } catch (err) {
          patch(host.id, {
            status: "failed",
            detail: err instanceof Error ? err.message : String(err),
          });
        }
      });
      finishBatch(Array.from(byId.values()));
    } finally {
      setBatchBusy(false);
    }
  }, [beginProgress, cleanupEligible, finishBatch, pollCleanupJobQuiet, selectedHosts, viewerEmail]);

  const runBatchReverify = React.useCallback(async () => {
    const targets = reverifyEligible;
    const skippedHosts = selectedHosts.filter((host) => !hostCanReverify(host, viewerEmail));
    const { byId, patch } = beginProgress("reverify", targets, skippedHosts);
    setBatchBusy(true);
    try {
      await mapPool(targets, 5, async (host) => {
        patch(host.id, { status: "running" });
        try {
          await reverifyClientMarketHost(host.id);
          patch(host.id, { status: "succeeded" });
        } catch (err) {
          patch(host.id, {
            status: "failed",
            detail: err instanceof Error ? err.message : String(err),
          });
        }
      });
      finishBatch(Array.from(byId.values()));
    } finally {
      setBatchBusy(false);
    }
  }, [beginProgress, finishBatch, reverifyEligible, selectedHosts, viewerEmail]);

  const runBatchDelete = React.useCallback(async () => {
    const targets = deleteEligible;
    const skippedHosts = selectedHosts.filter((host) => !hostCanDelete(host, viewerEmail));
    const { byId, patch } = beginProgress("delete", targets, skippedHosts);
    setBatchBusy(true);
    try {
      await mapPool(targets, 5, async (host) => {
        patch(host.id, { status: "running" });
        try {
          await deleteClientMarketHost(host.id);
          patch(host.id, { status: "succeeded" });
        } catch (err) {
          patch(host.id, {
            status: "failed",
            detail: err instanceof Error ? err.message : String(err),
          });
        }
      });
      finishBatch(Array.from(byId.values()));
    } finally {
      setBatchBusy(false);
    }
  }, [beginProgress, deleteEligible, finishBatch, selectedHosts, viewerEmail]);

  const confirmCopy: BatchConfirmCopy | null =
    batchConfirm === "cleanup"
      ? {
          title: t("clientMarket.batchConfirmCleanupTitle"),
          description: t("clientMarket.batchConfirmCleanupDesc", {
            run: cleanupEligible.length,
            skip: selectedCount - cleanupEligible.length,
          }),
          confirmLabel: t("clientMarket.cleanup"),
          run: () => void runBatchCleanup(),
        }
      : batchConfirm === "reverify"
        ? {
            title: t("clientMarket.batchConfirmReverifyTitle"),
            description: t("clientMarket.batchConfirmReverifyDesc", {
              run: reverifyEligible.length,
              skip: selectedCount - reverifyEligible.length,
            }),
            confirmLabel: t("clientMarket.reverifyHost"),
            run: () => void runBatchReverify(),
          }
        : batchConfirm === "delete"
          ? {
              title: t("clientMarket.batchConfirmDeleteTitle"),
              description: t("clientMarket.batchConfirmDeleteDesc", {
                run: deleteEligible.length,
                skip: selectedCount - deleteEligible.length,
              }),
              confirmLabel: t("clientMarket.deleteHost"),
              run: () => void runBatchDelete(),
            }
          : null;

  const progressLabel =
    progressAction === "cleanup"
      ? t("clientMarket.batchProgressCleanup")
      : progressAction === "reverify"
        ? t("clientMarket.batchProgressReverify")
        : t("clientMarket.batchProgressDelete");

  /** True while any batch surface would be disrupted by a background refresh. */
  const uiBusy = batchBusy || batchConfirm != null || progressOpen;

  return {
    selectionMode,
    enterSelectionMode,
    exitSelectionMode,
    selectedIds,
    selectedCount,
    setHostSelected,
    selectPage,
    deselectPage,
    selectAllFiltered,
    clearSelection,
    batchBusy,
    uiBusy,
    cleanupEligible,
    reverifyEligible,
    deleteEligible,
    exportEligible,
    requestBatch: setBatchConfirm,
    confirmCopy,
    dismissConfirm: () => setBatchConfirm(null),
    progressOpen,
    progressItems,
    progressLabel,
    closeProgress: () => setProgressOpen(false),
  };
}

export type BatchOperations = ReturnType<typeof useBatchOperations>;
