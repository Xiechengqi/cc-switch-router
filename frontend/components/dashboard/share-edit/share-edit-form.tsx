"use client";

import { Checkbox, Input } from "@heroui/react";
import * as React from "react";
import { EmptyBlock } from "@/components/dashboard/drawer-panels";
import {
  DEFAULT_PARALLEL_LIMIT,
  DEFAULT_TOKEN_LIMIT,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  type TFn,
} from "@/components/dashboard/share-dashboard-utils";
import { resolveShareCoreApp, shareProviderSupportedApps } from "@/lib/share-app";
import type { ShareView } from "@/lib/types";
import { updateShareSettings } from "@/lib/api";
import {
  buildShareEditDraft,
  buildShareEditPatch,
  shareEditPatchFingerprint,
  type PriceApp,
  type ShareEditDraft,
} from "./share-edit-draft";
import { ShareEditMarketFields, ShareEditSaleAccessFields } from "./share-edit-market-fields";
import { FieldGroup } from "./share-edit-shared";
import { ShareEditSection } from "./share-edit-section";
import { ShareUserGrantsEditor } from "./share-user-grants-editor";

function shareUserGrantsFingerprint(share: ShareView | null) {
  if (!share) return "";
  return JSON.stringify(
    Object.entries(share.userGrants || {})
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([email, grant]) => [
        email,
        grant.active,
        grant.role,
        grant.revision ?? 0,
        grant.policy,
        grant.usageQuota ?? null,
        grant.usageRebase ?? null,
      ]),
  );
}

export type ShareEditFormApi = {
  liveShare: ShareView;
  draft: ShareEditDraft;
  shareApp?: PriceApp;
  activeShareApps: PriceApp[];
  busy: boolean;
  locked: boolean;
  remoteRefreshPending: boolean;
  error: string;
  notice: string;
  descriptionLength: number;
  descriptionInvalid: boolean;
  tokenInvalid: boolean;
  parallelInvalid: boolean;
  expiryInvalid: boolean;
  appApiInvalid: boolean;
  formInvalid: boolean;
  isDirty: boolean;
  setError: (value: string) => void;
  setNotice: (value: string) => void;
  onDescriptionChange: (value: string) => void;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
  handleTokenUnlimited: (checked: boolean) => void;
  handleParallelUnlimited: (checked: boolean) => void;
  resetDraft: () => void;
  save: () => Promise<void>;
};

export function useShareEditForm({
  share,
  t,
  onSaved,
  onClose,
}: {
  share: ShareView | null;
  t: TFn;
  onSaved: (result: { appliedSynchronously: boolean }) => Promise<void>;
  onClose: () => void;
}): ShareEditFormApi | null {
  const [draft, setDraft] = React.useState<ShareEditDraft | null>(null);
  const [baseDraft, setBaseDraft] = React.useState<ShareEditDraft | null>(null);
  const [baseShare, setBaseShare] = React.useState<ShareView | null>(null);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [remoteRefreshPending, setRemoteRefreshPending] = React.useState(false);
  const draftRef = React.useRef<ShareEditDraft | null>(null);
  const baseDraftRef = React.useRef<ShareEditDraft | null>(null);
  const baseShareRef = React.useRef<ShareView | null>(null);

  const liveShare = share || baseShare;
  const activeShareApps = React.useMemo(() => shareProviderSupportedApps(liveShare), [liveShare]);
  const shareApp = activeShareApps[0] ?? resolveShareCoreApp(liveShare);
  const applyDraft = React.useCallback((next: ShareEditDraft) => setDraft(next), []);
  const locked = Boolean(liveShare?.activeEdit?.status === "pending" || liveShare?.canEditSettings === false);
  draftRef.current = draft;
  baseDraftRef.current = baseDraft;
  baseShareRef.current = baseShare;

  React.useEffect(() => {
    if (!share) {
      setBaseShare(null);
      setBaseDraft(null);
      setDraft(null);
      setRemoteRefreshPending(false);
      setError("");
      setNotice("");
      return;
    }
    const incoming = buildShareEditDraft(share);
    const currentBaseShare = baseShareRef.current;
    const currentBaseDraft = baseDraftRef.current;
    const currentDraft = draftRef.current;
    if (!currentBaseShare || currentBaseShare.shareId !== share.shareId) {
      setBaseShare(share);
      setBaseDraft(incoming);
      applyDraft(incoming);
      setRemoteRefreshPending(false);
      setError("");
      setNotice("");
      return;
    }
    const incomingApps = shareProviderSupportedApps(share);
    const currentApps = shareProviderSupportedApps(currentBaseShare);
    const sameRevision = (currentBaseShare.configRevision ?? 0) === (share.configRevision ?? 0);
    const sameFingerprint =
      shareEditPatchFingerprint(buildShareEditPatch(incoming, share, incomingApps)) ===
      shareEditPatchFingerprint(
        buildShareEditPatch(currentBaseDraft ?? incoming, currentBaseShare, currentApps),
      );
    const sameGrants = shareUserGrantsFingerprint(share) === shareUserGrantsFingerprint(currentBaseShare);
    if (sameRevision && sameFingerprint && sameGrants) {
      setBaseShare(share);
      return;
    }
    const dirty = Boolean(
      currentDraft &&
        currentBaseDraft &&
        shareEditPatchFingerprint(buildShareEditPatch(currentDraft, currentBaseShare, currentApps)) !==
          shareEditPatchFingerprint(buildShareEditPatch(currentBaseDraft, currentBaseShare, currentApps)),
    );
    if (!dirty) {
      setBaseShare(share);
      setBaseDraft(incoming);
      applyDraft(incoming);
      setRemoteRefreshPending(false);
      setNotice("");
      return;
    }
    setRemoteRefreshPending(true);
  }, [applyDraft, share]);

  const onDraftChange = React.useCallback(
    (updater: (current: ShareEditDraft) => ShareEditDraft) => {
      setDraft((current) => {
        if (!current) return current;
        return updater(current);
      });
    },
    [],
  );

  const onDescriptionChange = React.useCallback((value: string) => {
    setDraft((current) => (current ? { ...current, description: value } : current));
  }, []);

  const handleTokenUnlimited = React.useCallback((checked: boolean) => {
    onDraftChange((current) => {
      if (checked) {
        const parsed = Number.parseInt(current.tokenLimitInput, 10);
        return {
          ...current,
          tokenLimitUnlimited: true,
          lastFiniteTokenLimit:
            Number.isFinite(parsed) && parsed > 0 ? parsed : current.lastFiniteTokenLimit || DEFAULT_TOKEN_LIMIT,
          tokenLimitInput: String(UNLIMITED_TOKEN_LIMIT),
        };
      }
      return {
        ...current,
        tokenLimitUnlimited: false,
        tokenLimitInput: String(current.lastFiniteTokenLimit || DEFAULT_TOKEN_LIMIT),
      };
    });
  }, [onDraftChange]);

  const handleParallelUnlimited = React.useCallback((checked: boolean) => {
    onDraftChange((current) => {
      if (checked) {
        const parsed = Number.parseInt(current.parallelLimitInput, 10);
        return {
          ...current,
          parallelLimitUnlimited: true,
          lastFiniteParallelLimit:
            Number.isFinite(parsed) && parsed > 0
              ? parsed
              : current.lastFiniteParallelLimit || DEFAULT_PARALLEL_LIMIT,
          parallelLimitInput: String(UNLIMITED_PARALLEL_LIMIT),
        };
      }
      return {
        ...current,
        parallelLimitUnlimited: false,
        parallelLimitInput: String(current.lastFiniteParallelLimit || DEFAULT_PARALLEL_LIMIT),
      };
    });
  }, [onDraftChange]);

  if (!liveShare || !draft || !baseDraft) return null;

  const descriptionLength = draft.description.trim().length;
  const descriptionInvalid = descriptionLength > 200;
  const tokenParsed = Number.parseInt(draft.tokenLimitInput, 10);
  const tokenInvalid = !draft.tokenLimitUnlimited && (!Number.isFinite(tokenParsed) || tokenParsed <= 0);
  const parallelParsed = Number.parseInt(draft.parallelLimitInput, 10);
  const parallelInvalid =
    !draft.parallelLimitUnlimited && (!Number.isFinite(parallelParsed) || parallelParsed <= 0);
  const expiryInvalid = !draft.expiresPermanent && !draft.expiresAtInput.trim();
  const appApiInvalid = !activeShareApps.some((app) => draft.enabledApps[app]);
  const formInvalid =
    descriptionInvalid ||
    tokenInvalid ||
    parallelInvalid ||
    expiryInvalid || appApiInvalid;

  const currentPatch = buildShareEditPatch(draft, liveShare, activeShareApps);
  const basePatch = buildShareEditPatch(baseDraft, liveShare, activeShareApps);
  const isDirty = shareEditPatchFingerprint(currentPatch) !== shareEditPatchFingerprint(basePatch);

  const resetDraft = () => {
    if (!baseDraft || busy) return;
    const latest = share ? buildShareEditDraft(share) : baseDraft;
    if (share) setBaseShare(share);
    setBaseDraft(latest);
    applyDraft(latest);
    setRemoteRefreshPending(false);
    setError("");
    setNotice("");
  };

  const save = async () => {
    if (!liveShare || busy || locked || formInvalid || !isDirty) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const res = await updateShareSettings(
        liveShare.shareId,
        currentPatch,
        liveShare.configRevision,
      );
      await onSaved({ appliedSynchronously: res.appliedSynchronously });
      if (res.appliedSynchronously) {
        onClose();
      } else {
        setBaseDraft(draft);
        setBaseShare(liveShare);
        setRemoteRefreshPending(false);
        setNotice(t("dashboard.shareEditQueued"));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return {
    liveShare,
    draft,
    shareApp,
    activeShareApps,
    busy,
    locked,
    remoteRefreshPending,
    error,
    notice,
    descriptionLength,
    descriptionInvalid,
    tokenInvalid,
    parallelInvalid,
    expiryInvalid,
    appApiInvalid,
    formInvalid,
    isDirty,
    setError,
    setNotice,
    onDescriptionChange,
    onDraftChange,
    resetDraft,
    save,
    handleTokenUnlimited,
    handleParallelUnlimited,
  };
}

export function ShareEditFormBody({
  share,
  t,
  form,
}: {
  share: ShareView;
  t: TFn;
  form: ShareEditFormApi;
}) {
  const { activeShareApps, draft, shareApp, liveShare, locked } = form;
  const fieldsDisabled = locked;
  const displayShare = liveShare || share;

  if (!shareApp) {
    return <EmptyBlock>{t("dashboard.shareEditNoAppType")}</EmptyBlock>;
  }
  const defaultUserPolicy = {
    parallelLimit: draft.parallelLimitUnlimited
      ? undefined
      : Number.parseInt(draft.parallelLimitInput, 10),
    tokenLimit: draft.tokenLimitUnlimited
      ? undefined
      : Number.parseInt(draft.tokenLimitInput, 10),
    tokenPeriod: "lifetime" as const,
    expiresAt: draft.expiresPermanent
      ? undefined
      : new Date(draft.expiresAtInput).getTime(),
  };

  return (
    <>
      <ShareEditMarketFields
        t={t}
        share={displayShare}
        activeShareApps={activeShareApps}
        draft={draft}
        descriptionInvalid={form.descriptionInvalid}
        appApiInvalid={form.appApiInvalid}
        disabled={fieldsDisabled}
        onDescriptionChange={form.onDescriptionChange}
        onDraftChange={form.onDraftChange}
      />

      <ShareEditSection title={t("dashboard.shareEdit.section.access")}>
        <ShareEditSaleAccessFields
          t={t}
          draft={draft}
          disabled={fieldsDisabled}
          onDraftChange={form.onDraftChange}
        />

        <div className="grid gap-3 md:grid-cols-3">
          <FieldGroup label={t("dashboard.field.tokenLimit")} invalid={form.tokenInvalid}>
            <div className="grid gap-2">
              <Input
                type="number"
                min={1}
                step={1}
                value={draft.tokenLimitInput}
                disabled={fieldsDisabled || draft.tokenLimitUnlimited}
                onChange={(event) => {
                  const value = event.target.value;
                  form.onDraftChange((current) => {
                    const parsed = Number.parseInt(value, 10);
                    return {
                      ...current,
                      tokenLimitInput: value,
                      lastFiniteTokenLimit:
                        Number.isFinite(parsed) && parsed > 0 ? parsed : current.lastFiniteTokenLimit,
                    };
                  });
                }}
              />
              <Checkbox
                isSelected={draft.tokenLimitUnlimited}
                isDisabled={fieldsDisabled}
                onChange={(value: boolean) => form.handleTokenUnlimited(value)}
              >
                <Checkbox.Control>
                  <Checkbox.Indicator />
                </Checkbox.Control>
                <Checkbox.Content>
                  <span className="text-xs text-muted-foreground">{t("common.unlimited")}</span>
                </Checkbox.Content>
              </Checkbox>
            </div>
          </FieldGroup>

          <FieldGroup label={t("dashboard.field.parallelLimit")} invalid={form.parallelInvalid}>
            <div className="grid gap-2">
              <Input
                type="number"
                min={1}
                step={1}
                value={draft.parallelLimitInput}
                disabled={fieldsDisabled || draft.parallelLimitUnlimited}
                onChange={(event) => {
                  const value = event.target.value;
                  form.onDraftChange((current) => {
                    const parsed = Number.parseInt(value, 10);
                    return {
                      ...current,
                      parallelLimitInput: value,
                      lastFiniteParallelLimit:
                        Number.isFinite(parsed) && parsed > 0 ? parsed : current.lastFiniteParallelLimit,
                    };
                  });
                }}
              />
              <Checkbox
                isSelected={draft.parallelLimitUnlimited}
                isDisabled={fieldsDisabled}
                onChange={(value: boolean) => form.handleParallelUnlimited(value)}
              >
                <Checkbox.Control>
                  <Checkbox.Indicator />
                </Checkbox.Control>
                <Checkbox.Content>
                  <span className="text-xs text-muted-foreground">{t("common.unlimited")}</span>
                </Checkbox.Content>
              </Checkbox>
            </div>
          </FieldGroup>

          <FieldGroup label={t("dashboard.field.expiresAt")} invalid={form.expiryInvalid}>
            <div className="grid gap-2">
              <Input
                type="datetime-local"
                value={draft.expiresAtInput}
                disabled={fieldsDisabled || draft.expiresPermanent}
                onChange={(event) =>
                  form.onDraftChange((current) => ({ ...current, expiresAtInput: event.target.value }))
                }
              />
              <Checkbox
                isSelected={draft.expiresPermanent}
                isDisabled={fieldsDisabled}
                onChange={(value: boolean) =>
                  form.onDraftChange((current) => ({ ...current, expiresPermanent: value }))
                }
              >
                <Checkbox.Control>
                  <Checkbox.Indicator />
                </Checkbox.Control>
                <Checkbox.Content>
                  <span className="text-xs text-muted-foreground">{t("dashboard.permanent")}</span>
                </Checkbox.Content>
              </Checkbox>
            </div>
          </FieldGroup>
        </div>

        <ShareUserGrantsEditor
          value={draft.userGrants}
          ownerEmail={displayShare.ownerEmail || ""}
          defaultPolicy={defaultUserPolicy}
          supportedPeriods={displayShare.supportedUserTokenPeriods}
          t={t}
          disabled={fieldsDisabled}
          shareId={displayShare.shareId}
          usageEdits={draft.userUsageEdits}
          onUsageEditsChange={(userUsageEdits) =>
            form.onDraftChange((current) => ({ ...current, userUsageEdits }))
          }
          onChange={(userGrants) =>
            form.onDraftChange((current) => ({ ...current, userGrants }))
          }
        />
      </ShareEditSection>
    </>
  );
}
