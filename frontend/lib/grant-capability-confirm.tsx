"use client";

import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import type { GrantCapabilityConfirm } from "@/lib/share-grant-capabilities";

/** HeroUI confirmation for uncovered grant usage. Cancel resolves false and does not save. */
export function useGrantCapabilityConfirm(copy: { title: string; confirm: string; cancel: string }) {
  const [request, setRequest] = React.useState<{ labels: string[]; done: (accepted: boolean) => void } | null>(null);
  const confirm = React.useCallback<GrantCapabilityConfirm>((labels) => new Promise((done) => {
    setRequest({ labels, done });
  }), []);
  const finish = (accepted: boolean) => {
    request?.done(accepted);
    setRequest(null);
  };
  const dialog = (
    <ConfirmAlertDialog
      open={request !== null}
      title={copy.title}
      description={request ? <span className="whitespace-pre-line">{request.labels.join("\n")}</span> : ""}
      confirmLabel={copy.confirm}
      cancelLabel={copy.cancel}
      tone="warning"
      onConfirm={() => finish(true)}
      onOpenChange={(open) => { if (!open && request) finish(false); }}
    />
  );
  return { confirm, dialog };
}
