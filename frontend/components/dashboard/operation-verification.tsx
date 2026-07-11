"use client";

import { toast } from "@heroui/react";
import * as React from "react";
import { marketOperationalSummary, shareOperationalSummary } from "@/components/dashboard/operational-status";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { DashboardResponse, OperationalState } from "@/lib/types";
import { recordDashboardUxEvent } from "@/lib/api";

type VerificationTarget = {
  kind: "share" | "market";
  id: string;
  submittedAt: number;
  baselineSnapshot?: string;
  expectedState?: OperationalState;
  requireHealthyRoute?: boolean;
};

type OperationVerificationValue = {
  trackOperation: (target: Omit<VerificationTarget, "submittedAt">) => void;
};

const OperationVerificationContext = React.createContext<OperationVerificationValue | null>(null);
const VERIFY_TIMEOUT_MS = 30_000;

export function OperationVerificationProvider({ data, children }: { data: DashboardResponse | null; children: React.ReactNode }) {
  const [pending, setPending] = React.useState<VerificationTarget[]>([]);
  const { t } = useLocaleText();

  const trackOperation = React.useCallback((target: Omit<VerificationTarget, "submittedAt">) => {
    setPending((current) => [...current.filter((item) => item.kind !== target.kind || item.id !== target.id), { ...target, submittedAt: Date.now(), baselineSnapshot: data?.generatedAt }]);
    toast.info(t("dashboard.operationSubmitted"));
    void recordDashboardUxEvent({ eventType: "operation_submitted", targetType: target.kind });
  }, [data?.generatedAt, t]);

  React.useEffect(() => {
    if (!data || !pending.length) return;
    const now = Date.now();
    const remaining: VerificationTarget[] = [];
    for (const item of pending) {
      if (now - item.submittedAt >= VERIFY_TIMEOUT_MS) {
        toast.warning(t("dashboard.operationUnverified"));
        continue;
      }
      if (!data.generatedAt || data.generatedAt === item.baselineSnapshot) {
        remaining.push(item);
        continue;
      }
      if (item.kind === "share") {
        const share = data.shares?.find((candidate) => candidate.shareId === item.id);
        if (!share) {
          remaining.push(item);
          continue;
        }
        if (share.activeEdit?.status === "rejected") {
          toast.danger(t("dashboard.operationRejected"));
          continue;
        }
        if (share.activeEdit?.status === "pending") {
          remaining.push(item);
          continue;
        }
        const summary = shareOperationalSummary(share);
        if (item.requireHealthyRoute && (summary.state === "offline" || summary.primaryReason?.code === "route_offline")) {
          remaining.push(item);
          continue;
        }
        toast.success(t(item.requireHealthyRoute ? "dashboard.operationRouteVerified" : "dashboard.operationObserved"));
        void recordDashboardUxEvent({ eventType: "operation_verified", targetType: item.kind, elapsedMs: now - item.submittedAt });
        continue;
      }
      const market = data.markets?.find((candidate) => candidate.id === item.id);
      if (!market) {
        remaining.push(item);
        continue;
      }
      const summary = marketOperationalSummary(market);
      if (item.expectedState && summary.state !== item.expectedState) {
        remaining.push(item);
        continue;
      }
      toast.success(t("dashboard.operationObserved"));
      void recordDashboardUxEvent({ eventType: "operation_verified", targetType: item.kind, elapsedMs: now - item.submittedAt });
    }
    if (remaining.length !== pending.length) setPending(remaining);
  }, [data, pending, t]);

  const value = React.useMemo(() => ({ trackOperation }), [trackOperation]);
  return <OperationVerificationContext.Provider value={value}>{children}</OperationVerificationContext.Provider>;
}

export function useOperationVerification() {
  const value = React.useContext(OperationVerificationContext);
  if (!value) throw new Error("useOperationVerification must be used inside OperationVerificationProvider");
  return value;
}
