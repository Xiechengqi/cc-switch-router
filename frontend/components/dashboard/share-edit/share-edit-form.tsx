"use client";

import { Checkbox, Input } from "@heroui/react";
import * as React from "react";
import { EmptyBlock } from "@/components/dashboard/drawer-panels";
import {
  DEFAULT_PARALLEL_LIMIT,
  DEFAULT_TOKEN_LIMIT,
  isShareMarket,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  type TFn,
} from "@/components/dashboard/share-dashboard-utils";
import { resolveShareCoreApp, shareAccessApps } from "@/lib/share-app";
import type { DashboardMarket, ShareAccessByApp, ShareView } from "@/lib/types";
import { updateShareSettings } from "@/lib/api";
import {
  applyRecommendedMarketDefaults,
  buildShareEditDraft,
  buildShareEditPatch,
  normalizedUniqueEmails,
  shareEditPatchFingerprint,
  type PriceApp,
  type ShareEditDraft,
} from "./share-edit-draft";
import { ShareEditMarketFields } from "./share-edit-market-fields";
import { EmailTagsField, FieldGroup } from "./share-edit-shared";
import { ShareEditSection } from "./share-edit-section";

export type ShareEditFormApi = {
  draft: ShareEditDraft;
  shareApp?: PriceApp;
  busy: boolean;
  error: string;
  notice: string;
  confirmFreeOpen: boolean;
  transferTargetEmail: string;
  descriptionLength: number;
  descriptionInvalid: boolean;
  tokenInvalid: boolean;
  parallelInvalid: boolean;
  expiryInvalid: boolean;
  pricingInvalid: boolean;
  shareMarketInvalid: boolean;
  formInvalid: boolean;
  isDirty: boolean;
  marketSelectKey: number;
  tokenMarkets: DashboardMarket[];
  shareMarkets: DashboardMarket[];
  shareMarketEmails: ReadonlySet<string>;
  transferableShareEmails: string[];
  setError: (value: string) => void;
  setNotice: (value: string) => void;
  setConfirmFreeOpen: (value: boolean) => void;
  setTransferTargetEmail: (value: string) => void;
  handleForSaleChange: (next: "Yes" | "No" | "Free") => void;
  onDescriptionChange: (value: string) => void;
  onDraftChange: (updater: (current: ShareEditDraft) => ShareEditDraft) => void;
  onMarketPicked: (raw: string) => void;
  handleTokenUnlimited: (checked: boolean) => void;
  handleParallelUnlimited: (checked: boolean) => void;
  resetDraft: () => void;
  save: () => Promise<void>;
  transferOwner: () => Promise<void>;
  confirmFree: () => void;
};

export function useShareEditForm({
  share,
  markets,
  t,
  onSaved,
  onClose,
}: {
  share: ShareView | null;
  markets: DashboardMarket[];
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
  const [confirmFreeOpen, setConfirmFreeOpen] = React.useState(false);
  const [transferTargetEmail, setTransferTargetEmail] = React.useState("");
  const [marketSelectKey, setMarketSelectKey] = React.useState(0);

  const editShare = baseShare || share;
  const activeShareApps = React.useMemo(() => shareAccessApps(editShare), [editShare]);
  const shareApp = activeShareApps[0] ?? resolveShareCoreApp(editShare);
  const tokenMarkets = React.useMemo(() => markets.filter((market) => !isShareMarket(market)), [markets]);
  const shareMarkets = React.useMemo(() => markets.filter(isShareMarket), [markets]);
  const publicMarketEmails = React.useMemo(
    () => new Set(markets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [markets],
  );
  const tokenMarketEmails = React.useMemo(
    () => new Set(tokenMarkets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [tokenMarkets],
  );
  const shareMarketEmails = React.useMemo(
    () => new Set(shareMarkets.map((market) => (market.email || "").toLowerCase()).filter(Boolean)),
    [shareMarkets],
  );

  const applyDraft = React.useCallback((next: ShareEditDraft, recommend = false) => {
    setDraft(recommend ? applyRecommendedMarketDefaults(next, tokenMarkets, shareMarkets) : next);
    setMarketSelectKey((current) => current + 1);
  }, [shareMarkets, tokenMarkets]);

  React.useEffect(() => {
    if (!share) {
      setBaseShare(null);
      setBaseDraft(null);
      setDraft(null);
      return;
    }
    if (baseShare?.shareId === share.shareId) return;
    const initial = buildShareEditDraft(
      share,
      publicMarketEmails,
      tokenMarketEmails,
      shareMarketEmails,
    );
    setBaseShare(share);
    setBaseDraft(initial);
    applyDraft(initial);
    setError("");
    setNotice("");
    setConfirmFreeOpen(false);
    setTransferTargetEmail("");
  }, [applyDraft, baseShare?.shareId, publicMarketEmails, share, shareMarketEmails, tokenMarketEmails, tokenMarkets, shareMarkets]);

  React.useEffect(() => {
    if (!share || !draft || !baseDraft || baseShare?.shareId !== share.shareId) return;
    if (JSON.stringify(draft) !== JSON.stringify(baseDraft)) return;
    if (!tokenMarkets.length && !shareMarkets.length) return;
    const recommended = applyRecommendedMarketDefaults(draft, tokenMarkets, shareMarkets);
    if (JSON.stringify(recommended) === JSON.stringify(draft)) return;
    setDraft(recommended);
  }, [baseDraft, baseShare?.shareId, draft, share, shareMarkets, tokenMarkets]);

  const onDraftChange = React.useCallback(
    (updater: (current: ShareEditDraft) => ShareEditDraft) => {
      setDraft((current) => {
        if (!current) return current;
        return applyRecommendedMarketDefaults(updater(current), tokenMarkets, shareMarkets);
      });
    },
    [shareMarkets, tokenMarkets],
  );

  const onDescriptionChange = React.useCallback((value: string) => {
    setDraft((current) => (current ? { ...current, description: value } : current));
  }, []);

  const handleForSaleChange = React.useCallback(
    (next: "Yes" | "No" | "Free") => {
      if (!draft) return;
      if (next === "Free" && draft.forSale !== "Free") {
        setConfirmFreeOpen(true);
        return;
      }
      onDraftChange((current) => {
        const updated = {
          ...current,
          forSale: next,
          priceInputs:
            next === "Yes"
              ? current.priceInputs
              : { claude: "", codex: "", gemini: "" },
        };
        if (next === "Yes") {
          return applyRecommendedMarketDefaults(updated, tokenMarkets, shareMarkets);
        }
        return updated;
      });
    },
    [draft, onDraftChange, shareMarkets, tokenMarkets],
  );

  const confirmFree = React.useCallback(() => {
    onDraftChange((current) => ({
      ...current,
      forSale: "Free",
      priceInputs: { claude: "", codex: "", gemini: "" },
    }));
    setConfirmFreeOpen(false);
  }, [onDraftChange]);

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

  const onMarketPicked = React.useCallback(
    (raw: string) => {
      if (!raw || !draft) return;
      if (draft.saleMarketKind === "share") {
        const normalized = raw.toLowerCase();
        if (!shareMarketEmails.has(normalized)) return;
        onDraftChange((current) => ({
          ...current,
          marketAccessMode: "selected",
          selectedShareMarketEmail: normalized,
        }));
        setMarketSelectKey((current) => current + 1);
        return;
      }
      if (raw === "__all__") {
        onDraftChange((current) => ({
          ...current,
          marketAccessMode: "all",
          selectedMarketEmails: [],
        }));
        setMarketSelectKey((current) => current + 1);
        return;
      }
      const normalized = raw.toLowerCase();
      onDraftChange((current) => ({
        ...current,
        marketAccessMode: "selected",
        selectedMarketEmails: Array.from(new Set([...current.selectedMarketEmails, normalized])).sort(),
      }));
      setMarketSelectKey((current) => current + 1);
    },
    [draft, onDraftChange, shareMarketEmails],
  );

  const transferableShareEmails = React.useMemo(() => {
    if (!draft) return [];
    return normalizedUniqueEmails(
      Object.values(draft.shareToEmailsByApp)
        .flat()
        .filter((email) => !publicMarketEmails.has(email)),
    );
  }, [draft, publicMarketEmails]);

  if (!share || !draft || !baseDraft) return null;

  const descriptionLength = draft.description.trim().length;
  const descriptionInvalid = descriptionLength > 200;
  const tokenParsed = Number.parseInt(draft.tokenLimitInput, 10);
  const tokenInvalid = !draft.tokenLimitUnlimited && (!Number.isFinite(tokenParsed) || tokenParsed <= 0);
  const parallelParsed = Number.parseInt(draft.parallelLimitInput, 10);
  const parallelInvalid =
    !draft.parallelLimitUnlimited && (!Number.isFinite(parallelParsed) || parallelParsed <= 0);
  const expiryInvalid = !draft.expiresPermanent && !draft.expiresAtInput.trim();
  const pricingInvalid =
    draft.forSale === "Yes" &&
    draft.saleMarketKind === "token" &&
    activeShareApps.some((app) => {
      const raw = draft.priceInputs[app];
      if (!raw) return false;
      return !/^(?:[1-9]|[1-9][0-9]|100)$/.test(raw);
    });
  const shareMarketInvalid =
    draft.forSale === "Yes" && draft.saleMarketKind === "share" && !draft.selectedShareMarketEmail;
  const formInvalid =
    descriptionInvalid || tokenInvalid || parallelInvalid || expiryInvalid || pricingInvalid || shareMarketInvalid;

  const currentPatch = buildShareEditPatch(draft, editShare!, activeShareApps, publicMarketEmails);
  const basePatch = buildShareEditPatch(baseDraft, editShare!, activeShareApps, publicMarketEmails);
  const isDirty = shareEditPatchFingerprint(currentPatch) !== shareEditPatchFingerprint(basePatch);

  const resetDraft = () => {
    if (!baseDraft || busy) return;
    applyDraft(baseDraft);
    setError("");
    setNotice("");
    setConfirmFreeOpen(false);
    setTransferTargetEmail("");
  };

  const save = async () => {
    if (!share || busy || formInvalid || !isDirty) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const res = await updateShareSettings(share.shareId, currentPatch);
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

  const transferOwner = async () => {
    if (!share || busy || !transferTargetEmail) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const targetEmail = transferTargetEmail.toLowerCase();
      const effectiveSaleMarketKind = draft.forSale === "Yes" ? draft.saleMarketKind : "token";
      const effectiveMarketAccessMode =
        effectiveSaleMarketKind === "share" ? "selected" : draft.marketAccessMode;
      const accessByApp: ShareAccessByApp = {};
      for (const app of activeShareApps) {
        const shareToEmails = (draft.shareToEmailsByApp[app] ?? []).filter(
          (email) => !publicMarketEmails.has(email),
        );
        const saleEmails =
          draft.forSale === "Yes" && effectiveSaleMarketKind === "token" && effectiveMarketAccessMode === "selected"
            ? draft.selectedMarketEmails
            : draft.forSale === "Yes" && effectiveSaleMarketKind === "share" && draft.selectedShareMarketEmail
              ? [draft.selectedShareMarketEmail]
              : [];
        accessByApp[app] = {
          sharedWithEmails: normalizedUniqueEmails([
            ...shareToEmails.filter((email) => email !== targetEmail),
            share.ownerEmail || "",
            ...saleEmails,
          ]),
          marketAccessMode: effectiveMarketAccessMode,
        };
      }
      const nextShared = normalizedUniqueEmails(
        Object.values(accessByApp).flatMap((access) => access?.sharedWithEmails ?? []),
      );
      const res = await updateShareSettings(share.shareId, {
        ownerEmail: targetEmail,
        sharedWithEmails: nextShared,
        accessByApp,
        saleMarketKind: effectiveSaleMarketKind,
        marketAccessMode: effectiveMarketAccessMode,
      });
      await onSaved({ appliedSynchronously: res.appliedSynchronously });
      setTransferTargetEmail("");
      if (res.appliedSynchronously) {
        onClose();
      } else {
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
    busy,
    error,
    notice,
    confirmFreeOpen,
    transferTargetEmail,
    descriptionLength,
    descriptionInvalid,
    tokenInvalid,
    parallelInvalid,
    expiryInvalid,
    pricingInvalid,
    shareMarketInvalid,
    formInvalid,
    isDirty,
    marketSelectKey,
    tokenMarkets,
    shareMarkets,
    shareMarketEmails,
    transferableShareEmails,
    setError,
    setNotice,
    setConfirmFreeOpen,
    setTransferTargetEmail,
    handleForSaleChange,
    onDescriptionChange,
    onDraftChange,
    onMarketPicked,
    resetDraft,
    save,
    transferOwner,
    confirmFree,
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
  const { draft, shareApp } = form;

  if (!shareApp) {
    return <EmptyBlock>{t("dashboard.shareEditNoAppType")}</EmptyBlock>;
  }

  return (
    <>
      <ShareEditMarketFields
        t={t}
        share={share}
        shareApp={shareApp}
        draft={draft}
        tokenMarkets={form.tokenMarkets}
        shareMarkets={form.shareMarkets}
        marketSelectKey={form.marketSelectKey}
        descriptionLength={form.descriptionLength}
        descriptionInvalid={form.descriptionInvalid}
        pricingInvalid={form.pricingInvalid}
        shareMarketInvalid={form.shareMarketInvalid}
        onDescriptionChange={form.onDescriptionChange}
        onForSaleChange={form.handleForSaleChange}
        onDraftChange={form.onDraftChange}
        onMarketPicked={form.onMarketPicked}
      />

      <ShareEditSection title={t("dashboard.shareEdit.section.access")}>
        <FieldGroup label={t("dashboard.field.sharedWith")} hint={t("dashboard.hint.sharedWith")}>
          <EmailTagsField
            value={draft.shareToEmailsByApp[shareApp] ?? []}
            placeholder="friend@example.com, teammate@example.com"
            onChange={(emails) =>
              form.onDraftChange((current) => ({
                ...current,
                shareToEmailsByApp: { ...current.shareToEmailsByApp, [shareApp]: emails },
              }))
            }
            onPromote={(email) => form.setTransferTargetEmail(email)}
            promotableEmails={form.transferableShareEmails}
            promoteLabel={t("dashboard.setAsOwner")}
          />
        </FieldGroup>

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
      </ShareEditSection>
    </>
  );
}
