"use client";

/* Hallmark · preview: Provider service fee · eight states · not routed */
import { CommissionPolicyForm } from "./commission-policy-settings";
import type { CommissionPolicy } from "@/lib/wallet-policy-api";

const policy: CommissionPolicy = {
  version: 1, current: { policyVersion: 1, rateBps: 1000, effectiveAt: "2026-10-22T00:00:00Z" },
  scheduled: null, increaseNoticeDays: 7, canEdit: true,
};

export function CommissionPolicyPreview() {
  return <div className="grid max-w-xl gap-6">
    {(["default", "hover", "focus", "active", "disabled", "loading", "error", "success"] as const).map((state) => (
      <div key={state} className="grid gap-2">
        <p className="text-sm font-medium">{state}</p>
        <CommissionPolicyForm
          policy={state === "disabled" ? { ...policy, canEdit: false } : policy}
          rate={state === "error" ? "101" : "10"}
          busy={state === "loading"}
          error={state === "error" ? "Enter a rate from 0–100%, with up to 2 decimal places." : ""}
          saved={state === "success"}
          stale={false}
          previewState={state === "hover" || state === "focus" || state === "active" ? state : undefined}
          onSave={(event) => event.preventDefault()}
          onRateChange={() => {}}
          onRetry={() => {}}
        />
      </div>
    ))}
  </div>;
}
