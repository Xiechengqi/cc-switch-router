import { OPERATION_LABELS, SHARE_OPERATIONS, type CapabilityConsent, type ShareOperation } from "@/lib/share-capabilities";
import type { ShareSettingsPatch, ShareUserGrantMap } from "@/lib/types";

export type GrantCapabilityEdit = { operations: ShareOperation[]; reconfirm?: ShareOperation[]; expectedRevision: number };
export type GrantCapabilityRow = { operation: ShareOperation; available: boolean; selected: boolean; needsReconfirm: boolean };
export type GrantCapabilityPreview = {
  displayVersion: 3; shareId: string; configRevision: number; ready: boolean; unavailable: boolean; subject?: string;
  rows: GrantCapabilityRow[];
  requiredAcknowledgements?: { condition: string; scope: string; operation: ShareOperation }[];
  previewCredential?: string; expiresAt?: number;
};
export type GrantCapabilityRequest = (shareId: string, input: GrantCapabilityInput, signal?: AbortSignal) => Promise<GrantCapabilityPreview>;
export type GrantCapabilityInput = {
  expectedConfigRevision: number;
  subject?: string;
  grantCapabilityEdits: Record<string, GrantCapabilityEdit>;
  userGrantEdits?: ShareSettingsPatch["userGrantEdits"];
  shareTokenLimit?: number;
};

const knownOperation = (operation: unknown): operation is ShareOperation => SHARE_OPERATIONS.includes(operation as ShareOperation);

export function parseGrantCapabilityPreview(value: unknown, shareId: string, input: GrantCapabilityInput): GrantCapabilityPreview {
  const data = value as GrantCapabilityPreview;
  if (!data || data.displayVersion !== 3 || data.shareId !== shareId
    || data.configRevision !== input.expectedConfigRevision || data.subject !== input.subject
    || typeof data.ready !== "boolean" || typeof data.unavailable !== "boolean"
    || !Array.isArray(data.rows) || data.rows.length > SHARE_OPERATIONS.length
    || data.rows.some(row => !row || !knownOperation(row.operation) || typeof row.available !== "boolean"
      || typeof row.selected !== "boolean" || typeof row.needsReconfirm !== "boolean")
    || data.requiredAcknowledgements && (!Array.isArray(data.requiredAcknowledgements)
      || data.requiredAcknowledgements.some(ack => !ack || typeof ack.scope !== "string"
        || typeof ack.condition !== "string" || !knownOperation(ack.operation)))) {
    throw new Error("CAPABILITY_STATE_CHANGED");
  }
  return data;
}

export function changedGrantCapabilities(next: ShareUserGrantMap, saved: ShareUserGrantMap) {
  const edits: Record<string, GrantCapabilityEdit> = {};
  for (const [email, grant] of Object.entries(next)) {
    if (!grant.active || grant.manager === "routerShareMarket") continue;
    const previous = saved[email];
    const before = previous?.active && previous.manager !== "routerShareMarket" ? previous.policy.allowedOperations ?? [] : [];
    const operations = [...(grant.policy.allowedOperations ?? [])].sort();
    const reconfirm = grant.reconfirmOperations?.filter(op => operations.includes(op));
    if (operations.length === before.length && operations.every(op => before.includes(op)) && !reconfirm?.length) continue;
    edits[email] = { operations, ...(reconfirm?.length ? { reconfirm } : {}), expectedRevision: previous?.revision ?? 0 };
  }
  return edits;
}

export function needsGrantCapabilityReview(patch: ShareSettingsPatch, saved: ShareUserGrantMap) {
  if (patch.grantCapabilityEdits && Object.keys(patch.grantCapabilityEdits).length) return true;
  return Object.entries(saved).some(([email, grant]) => grant.active && grant.manager !== "routerShareMarket"
    && !!grant.policy.allowedOperations?.length && (patch.tokenLimit != null || !!patch.userGrantEdits?.[email]?.policy));
}

export type GrantCapabilityConfirm = (labels: string[]) => Promise<boolean>;

/** Review at the outer save boundary so sign-in or configuration changes cannot reuse a draft approval. */
export async function reviewGrantCapabilities(
  request: GrantCapabilityRequest,
  shareId: string,
  revision: number,
  patch: ShareSettingsPatch,
  zh: boolean,
  confirm: GrantCapabilityConfirm,
): Promise<CapabilityConsent | null> {
  const preview = await request(shareId, {
    expectedConfigRevision: revision, grantCapabilityEdits: patch.grantCapabilityEdits ?? {},
    userGrantEdits: patch.userGrantEdits, shareTokenLimit: patch.tokenLimit,
  });
  if (!preview.ready || !preview.previewCredential || !preview.expiresAt || preview.expiresAt <= Date.now()) {
    throw new Error("GRANT_PREVIEW_EXPIRED");
  }
  const conditions = preview.requiredAcknowledgements ?? [];
  if (conditions.length) {
    const labels = [...new Set(conditions.map(ack => ack.scope + " · " + OPERATION_LABELS[ack.operation][zh ? 0 : 1]))];
    if (!await confirm(labels)) return null;
  }
  return { credential: preview.previewCredential, acceptUncovered: conditions.map(ack => ack.condition) };
}

export type GrantSettingsAttempt = { key: string; operationId: string; patch: ShareSettingsPatch };

export async function prepareGrantSettingsAttempt({
  previous, request, shareId, revision, patch, login, zh, confirm,
}: {
  previous?: GrantSettingsAttempt;
  request: GrantCapabilityRequest;
  shareId: string;
  revision: number;
  patch: ShareSettingsPatch;
  login: string;
  zh: boolean;
  confirm: GrantCapabilityConfirm;
}): Promise<GrantSettingsAttempt | null> {
  const key = JSON.stringify([shareId, revision, login, patch]);
  if (previous?.key === key) return previous;
  const consent = await reviewGrantCapabilities(request, shareId, revision, patch, zh, confirm);
  return consent ? {
    key, operationId: crypto.randomUUID(), patch: { ...patch, capabilityConsent: consent },
  } : null;
}
