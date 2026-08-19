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

export type ShareEditFormApi = {
  draft: ShareEditDraft;
  shareApp?: PriceApp;
  activeShareApps: PriceApp[];
  busy: boolean;
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

  const editShare = baseShare || share;
  const activeShareApps = React.useMemo(() => shareProviderSupportedApps(editShare), [editShare]);
  const shareApp = activeShareApps[0] ?? resolveShareCoreApp(editShare);
  const applyDraft = React.useCallback((next: ShareEditDraft) => setDraft(next), []);

  React.useEffect(() => {
    if (!share) {
      setBaseShare(null);
      setBaseDraft(null);
      setDraft(null);
      return;
    }
    if (baseShare?.shareId === share.shareId) return;
    const initial = buildShareEditDraft(share);
    setBaseShare(share);
    setBaseDraft(initial);
    applyDraft(initial);
    setError("");
    setNotice("");
  }, [applyDraft, baseShare?.shareId, share]);

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

  if (!share || !draft || !baseDraft) return null;

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

  const currentPatch = buildShareEditPatch(draft, editShare!, activeShareApps);
  const basePatch = buildShareEditPatch(baseDraft, editShare!, activeShareApps);
  const isDirty = shareEditPatchFingerprint(currentPatch) !== shareEditPatchFingerprint(basePatch);

  const resetDraft = () => {
    if (!baseDraft || busy) return;
    applyDraft(baseDraft);
    setError("");
    setNotice("");
  };

  const save = async () => {
    if (!share || busy || formInvalid || !isDirty) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const res = await updateShareSettings(
        share.shareId,
        currentPatch,
        share.configRevision,
      );
      await onSaved({ appliedSynchronously: res.appliedSynchronously });
      if (res.appliedSynchronously) {
        onClose();
      } else {
        setBaseDraft(draft);
        if (editShare) setBaseShare(editShare);
        setNotice(t("dashboard.shareEditQueued"));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return {
    draft,
    shareApp,
    activeShareApps,
    busy,
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
  const { activeShareApps, draft, shareApp } = form;

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
        share={share}
        activeShareApps={activeShareApps}
        draft={draft}
        descriptionLength={form.descriptionLength}
        descriptionInvalid={form.descriptionInvalid}
        appApiInvalid={form.appApiInvalid}
        onDescriptionChange={form.onDescriptionChange}
        onDraftChange={form.onDraftChange}
      />

      <ShareEditSection title={t("dashboard.shareEdit.section.access")}>
        <ShareEditSaleAccessFields
          t={t}
          draft={draft}
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
                disabled={draft.tokenLimitUnlimited}
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
                disabled={draft.parallelLimitUnlimited}
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
                disabled={draft.expiresPermanent}
                onChange={(event) =>
                  form.onDraftChange((current) => ({ ...current, expiresAtInput: event.target.value }))
                }
              />
              <Checkbox
                isSelected={draft.expiresPermanent}
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
          ownerEmail={share.ownerEmail || ""}
          defaultPolicy={defaultUserPolicy}
          supportedPeriods={share.supportedUserTokenPeriods}
          t={t}
          onChange={(userGrants) =>
            form.onDraftChange((current) => ({ ...current, userGrants }))
          }
        />
      </ShareEditSection>
    </>
  );
}
