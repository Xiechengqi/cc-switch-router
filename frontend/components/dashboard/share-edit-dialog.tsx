"use client";

import { Crown, Loader2, RotateCcw, Save, X } from "lucide-react";
import { Button, Checkbox, Input, ListBox, Modal, Select, TextArea } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { updateShareSettings } from "@/lib/api";
import { shareAccessApps, SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import type { DashboardMarket, ShareAccessByApp, ShareAppRuntimes, ShareSettingsPatch, ShareUpstreamProvider, ShareView } from "@/lib/types";
import { DEFAULT_PARALLEL_LIMIT, DEFAULT_TOKEN_LIMIT, MIN_PARALLEL_LIMIT, PERMANENT_EXPIRES_AT_ISO, UNLIMITED_PARALLEL_LIMIT, UNLIMITED_TOKEN_LIMIT, isOfficialRuntime, isPermanentExpiryDate, isShareMarket, isUnlimitedExpiry, isUnlimitedParallelLimit, isUnlimitedTokenLimit, marketLabel, type TFn } from "@/components/dashboard/share-dashboard-utils";

function splitEmails(value: string) {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);
}

function EmailTagsField({
  value,
  onChange,
  disabled,
  placeholder,
  onPromote,
  promotableEmails,
  promoteLabel,
}: {
  value: string[];
  onChange: (next: string[]) => void;
  disabled?: boolean;
  placeholder?: string;
  onPromote?: (email: string) => void;
  promotableEmails?: string[];
  promoteLabel?: string;
}) {
  const [draft, setDraft] = React.useState("");
  const promotableSet = React.useMemo(
    () => new Set(promotableEmails ?? []),
    [promotableEmails],
  );
  const commit = (raw: string) => {
    const parts = splitEmails(raw);
    setDraft("");
    if (!parts.length) return;
    const next = [...value];
    for (const part of parts) {
      if (!next.includes(part)) next.push(part);
    }
    if (next.length !== value.length) onChange(next);
  };
  const removeAt = (idx: number) => onChange(value.filter((_, i) => i !== idx));
  return (
    <div
      className={`flex min-h-10 w-full flex-wrap items-center gap-1.5 rounded-lg border border-slate-200 bg-white px-2 py-1.5 text-sm transition-colors focus-within:border-primary/50 ${disabled ? "cursor-not-allowed opacity-60" : ""}`}
    >
      {value.map((email, idx) => {
        const canPromote =
          !disabled && Boolean(onPromote) && promotableSet.has(email);
        return (
          <span
            key={email}
            className="inline-flex max-w-full items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
          >
            <span className="min-w-0 truncate">{email}</span>
            {canPromote ? (
              <button
                type="button"
                aria-label={`${promoteLabel ?? "Set as owner"}: ${email}`}
                title={promoteLabel ?? "Set as owner"}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-amber-100/70 text-amber-700 transition-colors hover:bg-amber-200/80"
                onClick={() => onPromote?.(email)}
              >
                <Crown className="h-3 w-3" />
              </button>
            ) : null}
            {disabled ? null : (
              <button
                type="button"
                aria-label={`remove ${email}`}
                className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
                onClick={() => removeAt(idx)}
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </span>
        );
      })}
      <input
        value={draft}
        disabled={disabled}
        className="h-7 min-w-[10rem] flex-1 bg-transparent text-slate-900 placeholder:text-muted-foreground focus:outline-none disabled:cursor-not-allowed"
        placeholder={value.length ? "" : placeholder}
        onChange={(event) => setDraft(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === ",") {
            event.preventDefault();
            commit(draft);
          } else if (event.key === "Backspace" && draft === "" && value.length) {
            event.preventDefault();
            removeAt(value.length - 1);
          }
        }}
        onBlur={() => commit(draft)}
        onPaste={(event) => {
          const text = event.clipboardData.getData("text");
          if (/[\s,;]/.test(text)) {
            event.preventDefault();
            commit(text);
          }
        }}
      />
    </div>
  );
}

function toLocalDateTimeValue(value?: string) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (num: number) => String(num).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function fromLocalDateTimeValue(value: string) {
  if (!value.trim()) return undefined;
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : value;
}

function providerHint(runtime?: ShareUpstreamProvider) {
  if (!runtime) return "";
  if (isOfficialRuntime(runtime)) return "Official";
  return runtime.accountEmail || runtime.apiUrl || runtime.kind || "";
}

type PriceApp = CoreShareApp;
const PRICE_APPS: Array<{ key: PriceApp; label: string }> = [
  { key: "claude", label: SHARE_APP_LABELS.claude },
  { key: "codex", label: SHARE_APP_LABELS.codex },
  { key: "gemini", label: SHARE_APP_LABELS.gemini },
];

function effectiveShareAccessByApp(share: ShareView): ShareAccessByApp {
  if (share.accessByApp && Object.keys(share.accessByApp).length > 0) return share.accessByApp;
  const result: ShareAccessByApp = {};
  for (const app of shareAccessApps(share)) {
    result[app] = {
      sharedWithEmails: share.sharedWithEmails ?? [],
      marketAccessMode: share.marketAccessMode === "all" ? "all" : "selected",
    };
  }
  return result;
}

function normalizedUniqueEmails(values: string[]) {
  return Array.from(new Set(values.map((value) => value.trim().toLowerCase()).filter(Boolean))).sort();
}

type ShareEditDraft = {
  description: string;
  forSale: "Yes" | "No" | "Free";
  saleMarketKind: "token" | "share";
  marketAccessMode: "selected" | "all";
  selectedMarketEmails: string[];
  selectedShareMarketEmail: string;
  shareToEmailsByApp: Record<PriceApp, string[]>;
  tokenLimitInput: string;
  tokenLimitUnlimited: boolean;
  lastFiniteTokenLimit: number;
  parallelLimitInput: string;
  parallelLimitUnlimited: boolean;
  lastFiniteParallelLimit: number;
  expiresAtInput: string;
  expiresPermanent: boolean;
  priceInputs: Record<PriceApp, string>;
};

function buildShareEditDraft(
  share: ShareView,
  publicMarketEmails: ReadonlySet<string>,
  tokenMarketEmails: ReadonlySet<string>,
  shareMarketEmails: ReadonlySet<string>,
): ShareEditDraft {
  const pendingPricing =
    share.activeEdit?.status === "rejected"
      ? share.activeEdit.patch.forSaleOfficialPricePercentByApp || {}
      : {};
  const sharePricing = share.forSaleOfficialPricePercentByApp || {};
  const priceInputs: Record<PriceApp, string> = { claude: "", codex: "", gemini: "" };
  for (const app of PRICE_APPS) {
    const pending = pendingPricing[app.key];
    const fallback = sharePricing[app.key];
    const value = typeof pending === "number" ? pending : fallback;
    priceInputs[app.key] = typeof value === "number" && value > 0 ? String(value) : "";
  }

  const saleMarketKind = share.saleMarketKind === "share" ? "share" : "token";
  const initialMode = (share.marketAccessMode as "selected" | "all") || "selected";
  const marketLinks = share.marketLinks || [];
  const accessByApp = effectiveShareAccessByApp(share);
  const tokenLimit = share.tokenLimit ?? UNLIMITED_TOKEN_LIMIT;
  const tokenLimitUnlimited = isUnlimitedTokenLimit(tokenLimit);
  const parallelLimit = Number.isFinite(share.parallelLimit)
    ? share.parallelLimit
    : UNLIMITED_PARALLEL_LIMIT;
  const parallelLimitUnlimited = isUnlimitedParallelLimit(parallelLimit);
  const expiresPermanent = isPermanentExpiryDate(share.expiresAt) || isUnlimitedExpiry(share.expiresAt);

  return {
    description: share.description || "",
    forSale: (share.forSale as "Yes" | "No" | "Free") || "No",
    saleMarketKind,
    marketAccessMode: saleMarketKind === "share" ? "selected" : initialMode,
    selectedMarketEmails:
      saleMarketKind === "token" && initialMode === "selected"
        ? normalizedUniqueEmails(
            marketLinks
              .map((link) => (link.email || "").toLowerCase())
              .filter((email) => email && !shareMarketEmails.has(email)),
          )
        : [],
    selectedShareMarketEmail:
      saleMarketKind === "share"
        ? marketLinks
            .map((link) => (link.email || "").toLowerCase())
            .find((email) => email && !tokenMarketEmails.has(email)) || ""
        : "",
    shareToEmailsByApp: {
      claude: splitEmails((accessByApp.claude?.sharedWithEmails || []).join("\n")).filter((email) => !publicMarketEmails.has(email)),
      codex: splitEmails((accessByApp.codex?.sharedWithEmails || []).join("\n")).filter((email) => !publicMarketEmails.has(email)),
      gemini: splitEmails((accessByApp.gemini?.sharedWithEmails || []).join("\n")).filter((email) => !publicMarketEmails.has(email)),
    },
    tokenLimitInput: tokenLimitUnlimited ? String(UNLIMITED_TOKEN_LIMIT) : String(tokenLimit),
    tokenLimitUnlimited,
    lastFiniteTokenLimit: !tokenLimitUnlimited && tokenLimit > 0 ? tokenLimit : DEFAULT_TOKEN_LIMIT,
    parallelLimitInput: parallelLimitUnlimited ? String(UNLIMITED_PARALLEL_LIMIT) : String(parallelLimit),
    parallelLimitUnlimited,
    lastFiniteParallelLimit: !parallelLimitUnlimited && parallelLimit >= MIN_PARALLEL_LIMIT ? parallelLimit : DEFAULT_PARALLEL_LIMIT,
    expiresAtInput: expiresPermanent ? "" : toLocalDateTimeValue(share.expiresAt),
    expiresPermanent,
    priceInputs,
  };
}

function buildShareEditPricingPayload(draft: ShareEditDraft, share?: ShareView | null) {
  if (draft.saleMarketKind === "share") return {};
  const result: Record<string, number> = {};
  for (const app of shareAccessApps(share ?? null)) {
    if (!share?.support?.[app]) continue;
    const raw = draft.priceInputs[app];
    if (!raw || !raw.trim()) continue;
    const value = Number.parseInt(raw, 10);
    if (Number.isFinite(value) && value >= 1 && value <= 100) result[app] = value;
  }
  return result;
}

function buildShareEditPatch(
  draft: ShareEditDraft,
  share: ShareView,
  activeShareApps: PriceApp[],
  publicMarketEmails: ReadonlySet<string>,
): ShareSettingsPatch {
  const effectiveSaleMarketKind = draft.forSale === "Yes" ? draft.saleMarketKind : "token";
  const effectiveMarketAccessMode = effectiveSaleMarketKind === "share" ? "selected" : draft.marketAccessMode;
  const tokenLimit = draft.tokenLimitUnlimited ? UNLIMITED_TOKEN_LIMIT : Number.parseInt(draft.tokenLimitInput, 10);
  const parallelLimit = draft.parallelLimitUnlimited ? UNLIMITED_PARALLEL_LIMIT : Number.parseInt(draft.parallelLimitInput, 10);
  const expiresIso = draft.expiresPermanent ? PERMANENT_EXPIRES_AT_ISO : fromLocalDateTimeValue(draft.expiresAtInput);
  const accessByApp: ShareAccessByApp = {};
  const appSettings: NonNullable<ShareSettingsPatch["appSettings"]> = {};
  for (const app of activeShareApps) {
    const shareToEmails = (draft.shareToEmailsByApp[app] ?? []).filter((email) => !publicMarketEmails.has(email));
    const saleEmails =
      draft.forSale === "Yes" && effectiveSaleMarketKind === "token" && effectiveMarketAccessMode === "selected"
        ? draft.selectedMarketEmails
        : draft.forSale === "Yes" && effectiveSaleMarketKind === "share" && draft.selectedShareMarketEmail
          ? [draft.selectedShareMarketEmail]
          : [];
    accessByApp[app] = {
      sharedWithEmails: normalizedUniqueEmails([...shareToEmails, ...saleEmails]),
      marketAccessMode: effectiveMarketAccessMode,
    };
    appSettings[app] = {
      forSale: draft.forSale,
      saleMarketKind: effectiveSaleMarketKind,
      marketAccessMode: effectiveMarketAccessMode,
      sharedWithEmails: accessByApp[app]?.sharedWithEmails ?? [],
      tokenLimit,
      parallelLimit,
      expiresAt: expiresIso || share.expiresAt,
    };
  }
  const patch: ShareSettingsPatch = {
    description: draft.description.trim() || null,
    forSale: draft.forSale,
    saleMarketKind: effectiveSaleMarketKind,
    marketAccessMode: effectiveMarketAccessMode,
    sharedWithEmails: normalizedUniqueEmails(
      Object.values(accessByApp).flatMap((access) => access?.sharedWithEmails ?? []),
    ),
    accessByApp,
    appSettings,
    tokenLimit,
    parallelLimit,
    forSaleOfficialPricePercentByApp: buildShareEditPricingPayload(draft, share),
  };
  if (expiresIso) patch.expiresAt = expiresIso;
  return patch;
}

function shareEditPatchFingerprint(patch: ShareSettingsPatch) {
  return JSON.stringify(patch);
}

export function ShareEditDialog({
  share,
  markets,
  onClose,
  onSaved,
}: {
  share: ShareView | null;
  markets: DashboardMarket[];
  onClose: () => void;
  onSaved: (result: { appliedSynchronously: boolean }) => Promise<void>;
}) {
  const [description, setDescription] = React.useState("");
  const [forSale, setForSale] = React.useState<"Yes" | "No" | "Free">("No");
  const [saleMarketKind, setSaleMarketKind] = React.useState<"token" | "share">("token");
  const [marketAccessMode, setMarketAccessMode] = React.useState<"selected" | "all">("selected");
  const [selectedMarketEmails, setSelectedMarketEmails] = React.useState<string[]>([]);
  const [selectedShareMarketEmail, setSelectedShareMarketEmail] = React.useState("");
  const [shareToEmailsByApp, setShareToEmailsByApp] = React.useState<Record<PriceApp, string[]>>({ claude: [], codex: [], gemini: [] });
  const [tokenLimitInput, setTokenLimitInput] = React.useState(String(DEFAULT_TOKEN_LIMIT));
  const [tokenLimitUnlimited, setTokenLimitUnlimited] = React.useState(false);
  const [lastFiniteTokenLimit, setLastFiniteTokenLimit] = React.useState(DEFAULT_TOKEN_LIMIT);
  const [parallelLimitInput, setParallelLimitInput] = React.useState(String(DEFAULT_PARALLEL_LIMIT));
  const [parallelLimitUnlimited, setParallelLimitUnlimited] = React.useState(false);
  const [lastFiniteParallelLimit, setLastFiniteParallelLimit] = React.useState(DEFAULT_PARALLEL_LIMIT);
  const [expiresAtInput, setExpiresAtInput] = React.useState("");
  const [expiresPermanent, setExpiresPermanent] = React.useState(false);
  const [priceInputs, setPriceInputs] = React.useState<Record<PriceApp, string>>({ claude: "", codex: "", gemini: "" });
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [notice, setNotice] = React.useState("");
  const [confirmFreeOpen, setConfirmFreeOpen] = React.useState(false);
  const [transferTargetEmail, setTransferTargetEmail] = React.useState("");
  const [marketSelectKey, setMarketSelectKey] = React.useState(0);
  const [baseShare, setBaseShare] = React.useState<ShareView | null>(null);
  const [baseDraft, setBaseDraft] = React.useState<ShareEditDraft | null>(null);
  const { t } = useLocaleText();
  const readOnly = !!share && !share.canManage;
  const editShare = baseShare || share;
  const activeShareApps = React.useMemo(() => shareAccessApps(editShare), [editShare]);
  const shareApp = activeShareApps[0];
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
  const transferableShareEmails = React.useMemo(
    () => normalizedUniqueEmails(Object.values(shareToEmailsByApp).flat().filter((email) => !publicMarketEmails.has(email))),
    [publicMarketEmails, shareToEmailsByApp],
  );

  const applyDraft = React.useCallback((draft: ShareEditDraft) => {
    setDescription(draft.description);
    setForSale(draft.forSale);
    setSaleMarketKind(draft.saleMarketKind);
    setMarketAccessMode(draft.marketAccessMode);
    setSelectedMarketEmails(draft.selectedMarketEmails);
    setSelectedShareMarketEmail(draft.selectedShareMarketEmail);
    setShareToEmailsByApp(draft.shareToEmailsByApp);
    setTokenLimitInput(draft.tokenLimitInput);
    setTokenLimitUnlimited(draft.tokenLimitUnlimited);
    setLastFiniteTokenLimit(draft.lastFiniteTokenLimit);
    setParallelLimitInput(draft.parallelLimitInput);
    setParallelLimitUnlimited(draft.parallelLimitUnlimited);
    setLastFiniteParallelLimit(draft.lastFiniteParallelLimit);
    setExpiresAtInput(draft.expiresAtInput);
    setExpiresPermanent(draft.expiresPermanent);
    setPriceInputs(draft.priceInputs);
    setMarketSelectKey((current) => current + 1);
  }, []);

  React.useEffect(() => {
    if (!share) {
      setBaseShare(null);
      setBaseDraft(null);
      return;
    }
    if (baseShare?.shareId === share.shareId) return;
    const draft = buildShareEditDraft(share, publicMarketEmails, tokenMarketEmails, shareMarketEmails);
    setBaseShare(share);
    setBaseDraft(draft);
    applyDraft(draft);
    setError(share.activeEdit?.status === "rejected" ? share.activeEdit.errorMessage || t("dashboard.applyFailedFallback") : "");
    setNotice("");
    setConfirmFreeOpen(false);
    setTransferTargetEmail("");
  }, [applyDraft, baseShare?.shareId, publicMarketEmails, share, shareMarketEmails, t, tokenMarketEmails]);

  const handleForSaleChange = (next: "Yes" | "No" | "Free") => {
    if (next === "Free" && forSale !== "Free") {
      setConfirmFreeOpen(true);
      return;
    }
    setForSale(next);
  };

  const handleSaleMarketKindChange = (next: "token" | "share") => {
    setSaleMarketKind(next);
    if (next === "share") {
      setMarketAccessMode("selected");
      setSelectedMarketEmails([]);
      setSelectedShareMarketEmail((current) => current || shareMarkets[0]?.email.toLowerCase() || "");
      setPriceInputs({ claude: "", codex: "", gemini: "" });
    } else {
      setSelectedShareMarketEmail("");
    }
  };

  const handleTokenUnlimited = (checked: boolean) => {
    setTokenLimitUnlimited(checked);
    if (checked) {
      const parsed = Number.parseInt(tokenLimitInput, 10);
      if (Number.isFinite(parsed) && parsed > 0) setLastFiniteTokenLimit(parsed);
      setTokenLimitInput(String(UNLIMITED_TOKEN_LIMIT));
    } else {
      setTokenLimitInput(String(lastFiniteTokenLimit));
    }
  };

  const handleParallelUnlimited = (checked: boolean) => {
    setParallelLimitUnlimited(checked);
    if (checked) {
      const parsed = Number.parseInt(parallelLimitInput, 10);
      if (Number.isFinite(parsed) && parsed >= MIN_PARALLEL_LIMIT) setLastFiniteParallelLimit(parsed);
      setParallelLimitInput(String(UNLIMITED_PARALLEL_LIMIT));
    } else {
      setParallelLimitInput(String(lastFiniteParallelLimit));
    }
  };

  const removeMarketEmail = (email: string) => {
    setSelectedMarketEmails((current) => current.filter((value) => value !== email));
  };

  const onMarketPicked = (raw: string) => {
    if (!raw) return;
    if (saleMarketKind === "share") {
      const normalized = raw.toLowerCase();
      if (!shareMarketEmails.has(normalized)) return;
      setMarketAccessMode("selected");
      setSelectedShareMarketEmail(normalized);
      setMarketSelectKey((current) => current + 1);
      return;
    }
    if (raw === "__all__") {
      setMarketAccessMode("all");
      setSelectedMarketEmails([]);
      setMarketSelectKey((current) => current + 1);
      return;
    }
    const normalized = raw.toLowerCase();
    setMarketAccessMode("selected");
    setSelectedMarketEmails((current) => Array.from(new Set([...current, normalized])).sort());
    setMarketSelectKey((current) => current + 1);
  };

  const availableMarkets = React.useMemo(() => {
    if (saleMarketKind === "share") {
      return [...shareMarkets].sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
    }
    const blocked = new Set(selectedMarketEmails);
    return tokenMarkets
      .filter((market) => market.email && !blocked.has(market.email.toLowerCase()))
      .sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
  }, [saleMarketKind, selectedMarketEmails, shareMarkets, tokenMarkets]);

  const currentDraft = React.useMemo<ShareEditDraft>(() => ({
    description,
    forSale,
    saleMarketKind,
    marketAccessMode,
    selectedMarketEmails,
    selectedShareMarketEmail,
    shareToEmailsByApp,
    tokenLimitInput,
    tokenLimitUnlimited,
    lastFiniteTokenLimit,
    parallelLimitInput,
    parallelLimitUnlimited,
    lastFiniteParallelLimit,
    expiresAtInput,
    expiresPermanent,
    priceInputs,
  }), [
    description,
    forSale,
    saleMarketKind,
    marketAccessMode,
    selectedMarketEmails,
    selectedShareMarketEmail,
    shareToEmailsByApp,
    tokenLimitInput,
    tokenLimitUnlimited,
    lastFiniteTokenLimit,
    parallelLimitInput,
    parallelLimitUnlimited,
    lastFiniteParallelLimit,
    expiresAtInput,
    expiresPermanent,
    priceInputs,
  ]);

  const descriptionLength = description.trim().length;
  const descriptionInvalid = descriptionLength > 200;

  const tokenParsed = Number.parseInt(tokenLimitInput, 10);
  const tokenInvalid = !tokenLimitUnlimited && (!Number.isFinite(tokenParsed) || tokenParsed <= 0);

  const parallelParsed = Number.parseInt(parallelLimitInput, 10);
  const parallelInvalid =
    !parallelLimitUnlimited && (!Number.isFinite(parallelParsed) || parallelParsed < MIN_PARALLEL_LIMIT);

  const expiryInvalid = !expiresPermanent && !expiresAtInput.trim();

  const pricingInvalid = React.useMemo(() => {
    if (saleMarketKind === "share") return false;
    const check = (raw: string) => {
      if (!raw || !raw.trim()) return false;
      const value = Number.parseInt(raw, 10);
      return !(Number.isFinite(value) && value >= 1 && value <= 100);
    };
    return activeShareApps.some((app) => check(priceInputs[app]));
  }, [activeShareApps, priceInputs, saleMarketKind]);

  const shareMarketInvalid = forSale === "Yes" && saleMarketKind === "share" && !selectedShareMarketEmail;

  const formInvalid =
    descriptionInvalid || tokenInvalid || parallelInvalid || expiryInvalid || pricingInvalid || shareMarketInvalid;

  const currentPatch = React.useMemo(
    () => (editShare ? buildShareEditPatch(currentDraft, editShare, activeShareApps, publicMarketEmails) : null),
    [activeShareApps, currentDraft, editShare, publicMarketEmails],
  );
  const basePatch = React.useMemo(
    () => (editShare && baseDraft ? buildShareEditPatch(baseDraft, editShare, activeShareApps, publicMarketEmails) : null),
    [activeShareApps, baseDraft, editShare, publicMarketEmails],
  );
  const isDirty = !!currentPatch && !!basePatch && shareEditPatchFingerprint(currentPatch) !== shareEditPatchFingerprint(basePatch);

  const resetDraft = () => {
    if (!baseDraft || busy) return;
    applyDraft(baseDraft);
    setError("");
    setNotice("");
    setConfirmFreeOpen(false);
    setTransferTargetEmail("");
  };

  const save = async () => {
    if (!share || !currentPatch || readOnly || busy || formInvalid || !isDirty) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const res = await updateShareSettings(share.shareId, currentPatch);
      await onSaved({ appliedSynchronously: res.appliedSynchronously });
      if (res.appliedSynchronously) {
        onClose();
      } else {
        setBaseDraft(currentDraft);
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
    if (!share || readOnly || busy || !transferTargetEmail) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const targetEmail = transferTargetEmail.toLowerCase();
      const effectiveSaleMarketKind = forSale === "Yes" ? saleMarketKind : "token";
      const effectiveMarketAccessMode = effectiveSaleMarketKind === "share" ? "selected" : marketAccessMode;
      const accessByApp: ShareAccessByApp = {};
      for (const app of activeShareApps) {
        const shareToEmails = (shareToEmailsByApp[app] ?? []).filter((email) => !publicMarketEmails.has(email));
        const saleEmails =
          forSale === "Yes" && effectiveSaleMarketKind === "token" && effectiveMarketAccessMode === "selected"
            ? selectedMarketEmails
            : forSale === "Yes" && effectiveSaleMarketKind === "share" && selectedShareMarketEmail
              ? [selectedShareMarketEmail]
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

  return (
    <>
      <Modal isOpen={!!share} onOpenChange={(open) => !open && !busy && onClose()}>
        <Modal.Backdrop>
          <Modal.Container>
            <Modal.Dialog className="share-edit-surface light w-[min(760px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
              <Modal.Header>
                <Modal.Heading>{readOnly ? t("dashboard.shareViewSettings") : t("dashboard.shareEditSettings")}</Modal.Heading>
                <p className="mt-1 break-all text-sm text-muted-foreground">{share?.subdomain || share?.shareName}</p>
                {readOnly ? (
                  <p className="mt-2 text-xs text-muted-foreground">{t("dashboard.shareReadOnlyNotice")}</p>
                ) : null}
              </Modal.Header>
              <Modal.Body className="grid max-h-[72vh] gap-4 overflow-y-auto">
                {error ? (
                  <div className="rounded-md border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">{error}</div>
                ) : null}
                {notice ? (
                  <div className="rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-700">{notice}</div>
                ) : null}

                <div className="grid gap-3 sm:grid-cols-2">
                  <FieldGroup label={t("dashboard.field.ownerEmail")}>
                    <Input value={share?.ownerEmail || ""} disabled />
                  </FieldGroup>
                  <FieldGroup label={t("dashboard.field.subdomain")}>
                    <Input value={share?.subdomain || ""} disabled />
                  </FieldGroup>
                </div>

                <FieldGroup
                  label={t("dashboard.field.description")}
                  hint={<span>{t("dashboard.hint.maxChars")}<span className="ml-2 font-mono">{descriptionLength}/200</span></span>}
                  invalid={descriptionInvalid}
                >
                  <TextArea
                    value={description}
                    maxLength={200}
                    disabled={readOnly}
                    onChange={(event) => setDescription(event.target.value)}
                  />
                </FieldGroup>

                {shareApp ? (
                  <>

                <div className="grid gap-3 sm:grid-cols-3">
                  <FieldGroup label={t("dashboard.field.forSale")}>
	                    <Select
	                      selectedKey={forSale}
	                      onSelectionChange={(key) => handleForSaleChange(String(key || "No") as "Yes" | "No" | "Free")}
                      isDisabled={readOnly}
	                    >
                      <Select.Trigger>
                        <Select.Value>{forSale}</Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          {["No", "Yes", "Free"].map((item) => (
                            <ListBox.Item key={item} id={item}>{item}</ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                  </FieldGroup>

                  {forSale === "Yes" ? (
                    <FieldGroup label={t("dashboard.field.marketType")}>
                      <Select
                        selectedKey={saleMarketKind}
                        onSelectionChange={(key) => handleSaleMarketKindChange(String(key || "token") as "token" | "share")}
                        isDisabled={readOnly}
                      >
                        <Select.Trigger>
                          <Select.Value>{saleMarketKind === "share" ? t("dashboard.shareMarket") : t("dashboard.tokenMarket")}</Select.Value>
                          <Select.Indicator />
                        </Select.Trigger>
                        <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                          <ListBox>
                            <ListBox.Item id="token">{t("dashboard.tokenMarket")}</ListBox.Item>
                            <ListBox.Item id="share">{t("dashboard.shareMarket")}</ListBox.Item>
                          </ListBox>
                        </Select.Popover>
                      </Select>
                    </FieldGroup>
                  ) : null}

                  <FieldGroup
                    label={t("dashboard.field.marketAccess")}
                    hint={
                      forSale !== "Yes"
                        ? t("dashboard.hint.forSaleOnly")
                        : saleMarketKind === "share"
                          ? t("dashboard.hint.shareMarketSingle")
                          : undefined
                    }
                    invalid={shareMarketInvalid}
                  >
                    <Select
                      key={marketSelectKey}
	                      selectedKey={null}
	                      onSelectionChange={(key) => onMarketPicked(String(key || ""))}
	                      isDisabled={readOnly || forSale !== "Yes" || (saleMarketKind === "share" && shareMarkets.length === 0)}
	                    >
                      <Select.Trigger>
                        <Select.Value>
                          {saleMarketKind === "share"
                            ? selectedShareMarketEmail
                              ? marketLabel(shareMarkets.find((market) => market.email.toLowerCase() === selectedShareMarketEmail) || {
                                  email: selectedShareMarketEmail,
                                  publicBaseUrl: "",
                                  subdomain: "",
                                })
                              : t("dashboard.selectShareMarket")
                            : marketAccessMode === "all" ? t("dashboard.allMarkets") : t("dashboard.addMarket")}
                        </Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="share-edit-popover light !bg-white !text-slate-900">
                        <ListBox>
                          {saleMarketKind === "token" ? (
                            <ListBox.Item id="__all__">{t("dashboard.allMarkets")}</ListBox.Item>
                          ) : null}
                          {availableMarkets.map((market) => (
                            <ListBox.Item key={market.email} id={market.email.toLowerCase()}>
                              {marketLabel(market)}
                              <span className="ml-1 text-muted-foreground">· {market.email}</span>
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                    {shareMarketInvalid ? <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span> : null}
                  </FieldGroup>
                </div>

                {forSale === "Yes" && saleMarketKind === "token" ? (
                <div className="grid gap-1.5 text-sm">
                  <div className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
                    <span className="mono-label text-muted-foreground">{t("dashboard.field.modelPricing")}</span>
                    <span className="text-xs text-muted-foreground">{t("dashboard.hint.modelPricing")}</span>
                  </div>
                  <div className="grid gap-3 sm:grid-cols-3">
                    {PRICE_APPS.filter((app) => app.key === shareApp).map((app) => {
                      const supported = !!share?.support?.[app.key];
                      const hint = providerHint(share?.appRuntimes?.[app.key]);
                      return (
                        <div key={app.key} className="grid gap-1">
                          <span className="mono-label text-muted-foreground">{app.label}</span>
                          <Input
                            type="number"
                            min={1}
                            max={100}
                            step={1}
                            value={priceInputs[app.key]}
                            disabled={readOnly || !supported}
                            placeholder={supported ? t("common.unset") : t("dashboard.noCurrentNode")}
                            onChange={(event) =>
                              setPriceInputs((current) => ({ ...current, [app.key]: event.target.value }))
                            }
                          />
                          <span className="truncate text-[11px] text-muted-foreground">{hint || "-"}</span>
                        </div>
                      );
                    })}
                  </div>
                  {pricingInvalid ? (
                    <span className="text-xs text-red-600">{t("dashboard.fieldInvalid")}</span>
                  ) : null}
                </div>
                ) : null}

                {forSale === "Yes" && saleMarketKind === "token" && marketAccessMode === "selected" ? (
                  <FieldGroup label={t("dashboard.field.selectedMarkets")} hint={t("dashboard.hint.selectedMarkets")}>
                    {selectedMarketEmails.length ? (
                      <div className="flex flex-wrap gap-1.5">
                        {selectedMarketEmails.map((email) => {
                          const meta = tokenMarkets.find((market) => (market.email || "").toLowerCase() === email);
                          const label = meta ? marketLabel(meta) : email;
                          return (
                            <span
                              key={email}
                              className="inline-flex items-center gap-1.5 rounded-full border border-primary/20 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary"
                            >
                              {label}
	                              {readOnly ? null : (
	                                <button
	                                  type="button"
	                                  aria-label={`remove ${email}`}
	                                  className="inline-flex h-4 w-4 items-center justify-center rounded-full bg-primary/15 transition-colors hover:bg-primary/25"
	                                  onClick={() => removeMarketEmail(email)}
	                                >
	                                  <X className="h-3 w-3" />
	                                </button>
	                              )}
                            </span>
                          );
                        })}
                      </div>
                    ) : (
                      <div className="rounded-lg border border-dashed border-border bg-muted/30 px-3 py-2 text-xs text-muted-foreground">
                        {t("dashboard.noAuthorizedMarkets")}
                      </div>
                    )}
                  </FieldGroup>
                ) : null}

                {forSale === "Yes" && saleMarketKind === "token" && marketAccessMode === "all" ? (
                  <div className="rounded-lg border border-primary/20 bg-primary/5 px-3 py-2 text-xs text-primary">
                    {t("dashboard.allMarketsSelected")}
	                    <button
	                      type="button"
	                      className="ml-3 text-[11px] underline decoration-dotted underline-offset-2 hover:text-primary/80"
                      disabled={readOnly}
	                      onClick={() => {
                        setMarketAccessMode("selected");
                        setSelectedMarketEmails([]);
                      }}
                    >
                      {t("dashboard.switchToSelected")}
                    </button>
                  </div>
                ) : null}

	                <FieldGroup label={t("dashboard.field.sharedWith")} hint={readOnly ? t("dashboard.hint.sharedWithReadOnly") : t("dashboard.hint.sharedWith")}>
                    <EmailTagsField
                      value={shareToEmailsByApp[shareApp] ?? []}
                      placeholder="friend@example.com, teammate@example.com"
                      disabled={readOnly}
                      onChange={(emails) =>
                        setShareToEmailsByApp((current) => ({ ...current, [shareApp]: emails }))
                      }
                      onPromote={(email) => setTransferTargetEmail(email)}
                      promotableEmails={transferableShareEmails}
                      promoteLabel={t("dashboard.setAsOwner")}
                    />
                </FieldGroup>

                <div className="grid gap-3 md:grid-cols-3">
                  <FieldGroup label={t("dashboard.field.tokenLimit")} invalid={tokenInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="number"
                        min={1}
                        step={1}
	                        value={tokenLimitInput}
	                        disabled={readOnly || tokenLimitUnlimited}
                        onChange={(event) => {
                          setTokenLimitInput(event.target.value);
                          const parsed = Number.parseInt(event.target.value, 10);
                          if (Number.isFinite(parsed) && parsed > 0) setLastFiniteTokenLimit(parsed);
                        }}
                      />
                      <Checkbox
	                        isSelected={tokenLimitUnlimited}
	                        onChange={(value: boolean) => handleTokenUnlimited(value)}
                          isDisabled={readOnly}
	                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">{t("common.unlimited")}</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>

                  <FieldGroup label={t("dashboard.field.parallelLimit")} hint={t("dashboard.hint.minValue", { value: MIN_PARALLEL_LIMIT })} invalid={parallelInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="number"
                        min={MIN_PARALLEL_LIMIT}
                        step={1}
	                        value={parallelLimitInput}
	                        disabled={readOnly || parallelLimitUnlimited}
                        onChange={(event) => {
                          setParallelLimitInput(event.target.value);
                          const parsed = Number.parseInt(event.target.value, 10);
                          if (Number.isFinite(parsed) && parsed >= MIN_PARALLEL_LIMIT) {
                            setLastFiniteParallelLimit(parsed);
                          }
                        }}
                      />
                      <Checkbox
	                        isSelected={parallelLimitUnlimited}
	                        onChange={(value: boolean) => handleParallelUnlimited(value)}
                          isDisabled={readOnly}
	                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">{t("common.unlimited")}</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>

                  <FieldGroup label={t("dashboard.field.expiresAt")} invalid={expiryInvalid}>
                    <div className="grid gap-2">
                      <Input
                        type="datetime-local"
	                        value={expiresAtInput}
	                        disabled={readOnly || expiresPermanent}
	                        onChange={(event) => setExpiresAtInput(event.target.value)}
                      />
                      <Checkbox
	                        isSelected={expiresPermanent}
	                        onChange={(value: boolean) => setExpiresPermanent(value)}
                          isDisabled={readOnly}
	                      >
                        <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                        <Checkbox.Content><span className="text-xs text-muted-foreground">{t("dashboard.permanent")}</span></Checkbox.Content>
                      </Checkbox>
                    </div>
                  </FieldGroup>
                </div>
                  </>
                ) : (
                  <div className="min-h-24 rounded-lg border border-dashed border-border bg-muted/20" />
                )}
              </Modal.Body>
              <Modal.Footer>
	                {readOnly ? null : (
	                  <Button variant="outline" onClick={resetDraft} isDisabled={busy || !isDirty}>
	                    <RotateCcw className="h-4 w-4" />
	                    {t("common.reset")}
	                  </Button>
	                )}
	                <Button variant="outline" onClick={onClose} isDisabled={busy}>{readOnly ? t("common.close") : t("common.cancel")}</Button>
	                {readOnly ? null : (
	                  <Button variant="primary" onClick={save} isDisabled={busy || formInvalid || !isDirty}>
	                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
	                    {t("common.save")}
	                  </Button>
	                )}
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>

      <ConfirmAlertDialog
        open={confirmFreeOpen}
        title={t("dashboard.confirmFreeTitle")}
        description={t("dashboard.confirmFreeDesc")}
        confirmLabel={t("dashboard.confirmFree")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        onConfirm={() => {
          setForSale("Free");
          setConfirmFreeOpen(false);
        }}
        onOpenChange={(open) => !open && setConfirmFreeOpen(false)}
      />
      <ConfirmAlertDialog
        open={Boolean(transferTargetEmail)}
        title={t("dashboard.transferOwnerTitle")}
        description={t("dashboard.transferOwnerDesc", { target: transferTargetEmail || "-", owner: share?.ownerEmail || "-" })}
        confirmLabel={t("dashboard.transferOwnerConfirm")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        onConfirm={transferOwner}
        onOpenChange={(open) => !open && setTransferTargetEmail("")}
      />
    </>
  );
}

export function FieldGroup({
  label,
  hint,
  invalid,
  children,
}: {
  label: string;
  hint?: React.ReactNode;
  invalid?: boolean;
  children: React.ReactNode;
}) {
  const { t } = useLocaleText();
  return (
    <div className="grid gap-1.5 text-sm">
      <span className="mono-label text-muted-foreground">{label}</span>
      {children}
      {hint || invalid ? (
        <span className={`text-xs ${invalid ? "text-red-600" : "text-muted-foreground"}`}>
          {invalid ? t("dashboard.fieldInvalid") : null}
          {hint && !invalid ? hint : null}
        </span>
      ) : null}
    </div>
  );
}
