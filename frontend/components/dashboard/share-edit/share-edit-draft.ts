import { shareAccessApps, SHARE_APP_LABELS, type CoreShareApp } from "@/lib/share-app";
import type {
  DashboardMarket,
  ShareAccessByApp,
  ShareSettingsPatch,
  ShareUserGrantMap,
  ShareUserPolicy,
  ShareView,
} from "@/lib/types";
import {
  DEFAULT_PARALLEL_LIMIT,
  DEFAULT_TOKEN_LIMIT,
  isPermanentExpiryDate,
  isShareMarket,
  isUnlimitedExpiry,
  isUnlimitedParallelLimit,
  isUnlimitedTokenLimit,
  marketLabel,
  PERMANENT_EXPIRES_AT_ISO,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
} from "@/components/dashboard/share-dashboard-utils";

export type PriceApp = CoreShareApp;

export const PRICE_APPS: Array<{ key: PriceApp; label: string }> = [
  { key: "claude", label: SHARE_APP_LABELS.claude },
  { key: "codex", label: SHARE_APP_LABELS.codex },
  { key: "gemini", label: SHARE_APP_LABELS.gemini },
];

export type ShareEditDraft = {
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
  userGrantsSupported: boolean;
  userGrants: ShareUserGrantMap;
};

export function splitEmails(value: string) {
  return value
    .split(/[\s,;]+/)
    .map((item) => item.trim().toLowerCase())
    .filter(Boolean);
}

export function toLocalDateTimeValue(value?: string) {
  if (!value) return "";
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const pad = (num: number) => String(num).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function fromLocalDateTimeValue(value: string) {
  if (!value.trim()) return undefined;
  const date = new Date(value);
  return Number.isFinite(date.getTime()) ? date.toISOString() : value;
}

export function effectiveShareAccessByApp(share: ShareView): ShareAccessByApp {
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

export function normalizedUniqueEmails(values: string[]) {
  return Array.from(new Set(values.map((value) => value.trim().toLowerCase()).filter(Boolean))).sort();
}

export function sortedTokenMarkets(markets: DashboardMarket[]) {
  return markets
    .filter((market) => !isShareMarket(market) && market.email)
    .sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
}

export function sortedShareMarkets(markets: DashboardMarket[]) {
  return markets.filter(isShareMarket).sort((a, b) => marketLabel(a).localeCompare(marketLabel(b)));
}

/** 推荐：Share Market 取排序后第一个；Token Market 取排序后第一个（selected 模式）。 */
export function recommendedShareMarketEmail(shareMarkets: DashboardMarket[]) {
  return sortedShareMarkets(shareMarkets)[0]?.email?.toLowerCase() || "";
}

export function recommendedTokenMarketEmail(tokenMarkets: DashboardMarket[]) {
  return sortedTokenMarkets(tokenMarkets)[0]?.email?.toLowerCase() || "";
}

export function applyRecommendedMarketDefaults(
  draft: ShareEditDraft,
  tokenMarkets: DashboardMarket[],
  shareMarkets: DashboardMarket[],
): ShareEditDraft {
  if (draft.forSale !== "Yes") return draft;

  if (draft.saleMarketKind === "share") {
    if (draft.selectedShareMarketEmail || !shareMarkets.length) return draft;
    const email = recommendedShareMarketEmail(shareMarkets);
    if (!email) return draft;
    return {
      ...draft,
      marketAccessMode: "selected",
      selectedShareMarketEmail: email,
    };
  }

  if (draft.marketAccessMode !== "selected" || draft.selectedMarketEmails.length) return draft;
  const email = recommendedTokenMarketEmail(tokenMarkets);
  if (!email) return draft;
  return {
    ...draft,
    selectedMarketEmails: [email],
  };
}

export function buildShareEditDraft(
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
  const parallelLimit = Number.isFinite(share.parallelLimit) ? share.parallelLimit : UNLIMITED_PARALLEL_LIMIT;
  const parallelLimitUnlimited = isUnlimitedParallelLimit(parallelLimit);
  const expiresPermanent = isPermanentExpiryDate(share.expiresAt) || isUnlimitedExpiry(share.expiresAt);
  const defaultUserPolicy: ShareUserPolicy = {
    parallelLimit: parallelLimitUnlimited ? undefined : parallelLimit,
    tokenLimit: tokenLimitUnlimited ? undefined : tokenLimit,
    tokenPeriod: "lifetime",
    expiresAt: expiresPermanent ? undefined : new Date(share.expiresAt).getTime(),
  };
  const userGrantsSupported = Object.keys(share.userGrants || {}).length > 0;
  const userGrants: ShareUserGrantMap = { ...(share.userGrants || {}) };
  const ownerEmail = (share.ownerEmail || "").trim().toLowerCase();
  if (userGrantsSupported && ownerEmail && !userGrants[ownerEmail]) {
    userGrants[ownerEmail] = {
      email: ownerEmail,
      role: "owner",
      active: true,
      policy: { ...defaultUserPolicy },
    };
  }
  const accessEmails = normalizedUniqueEmails(
    Object.values(accessByApp)
      .flatMap((access) => access?.sharedWithEmails || []),
  );
  for (const email of userGrantsSupported ? accessEmails : []) {
    if (!userGrants[email]) {
      userGrants[email] = {
        email,
        role: "shareto",
        active: true,
        policy: { ...defaultUserPolicy },
      };
    }
  }

  const draft: ShareEditDraft = {
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
      claude: splitEmails((accessByApp.claude?.sharedWithEmails || []).join("\n")).filter(
        (email) => !publicMarketEmails.has(email),
      ),
      codex: splitEmails((accessByApp.codex?.sharedWithEmails || []).join("\n")).filter(
        (email) => !publicMarketEmails.has(email),
      ),
      gemini: splitEmails((accessByApp.gemini?.sharedWithEmails || []).join("\n")).filter(
        (email) => !publicMarketEmails.has(email),
      ),
    },
    tokenLimitInput: tokenLimitUnlimited ? String(UNLIMITED_TOKEN_LIMIT) : String(tokenLimit),
    tokenLimitUnlimited,
    lastFiniteTokenLimit: !tokenLimitUnlimited && tokenLimit > 0 ? tokenLimit : DEFAULT_TOKEN_LIMIT,
    parallelLimitInput: parallelLimitUnlimited ? String(UNLIMITED_PARALLEL_LIMIT) : String(parallelLimit),
    parallelLimitUnlimited,
    lastFiniteParallelLimit:
      !parallelLimitUnlimited && parallelLimit > 0 ? parallelLimit : DEFAULT_PARALLEL_LIMIT,
    expiresAtInput: expiresPermanent ? "" : toLocalDateTimeValue(share.expiresAt),
    expiresPermanent,
    priceInputs,
    userGrantsSupported,
    userGrants,
  };

  return draft;
}

function buildShareEditPricingPayload(draft: ShareEditDraft, share?: ShareView | null) {
  if (draft.forSale !== "Yes" || draft.saleMarketKind !== "token") return {};
  const result: Record<string, number> = {};
  for (const app of shareAccessApps(share ?? null)) {
    if (!share?.support?.[app]) continue;
    const raw = draft.priceInputs[app];
    if (!raw || !raw.trim()) continue;
    if (!/^(?:[1-9]|[1-9][0-9]|100)$/.test(raw)) continue;
    result[app] = Number(raw);
  }
  return result;
}

export function buildShareEditPatch(
  draft: ShareEditDraft,
  share: ShareView,
  activeShareApps: PriceApp[],
  publicMarketEmails: ReadonlySet<string>,
): ShareSettingsPatch {
  const effectiveSaleMarketKind = draft.forSale === "Yes" ? draft.saleMarketKind : "token";
  const effectiveMarketAccessMode = effectiveSaleMarketKind === "share" ? "selected" : draft.marketAccessMode;
  const tokenLimit = draft.tokenLimitUnlimited ? UNLIMITED_TOKEN_LIMIT : Number.parseInt(draft.tokenLimitInput, 10);
  const parallelLimit = draft.parallelLimitUnlimited
    ? UNLIMITED_PARALLEL_LIMIT
    : Number.parseInt(draft.parallelLimitInput, 10);
  const expiresIso = draft.expiresPermanent ? PERMANENT_EXPIRES_AT_ISO : fromLocalDateTimeValue(draft.expiresAtInput);
  const accessByApp: ShareAccessByApp = {};
  const appSettings: NonNullable<ShareSettingsPatch["appSettings"]> = {};
  const directShareToEmails = normalizedUniqueEmails(
    Object.values(draft.shareToEmailsByApp).flat(),
  );
  const activeGrantEmails = new Set<string>();
  const ownerEmail = (share.ownerEmail || "").trim().toLowerCase();
  if (ownerEmail) activeGrantEmails.add(ownerEmail);
  for (const app of activeShareApps) {
    const shareToEmails = directShareToEmails.filter((email) => !publicMarketEmails.has(email));
    const saleEmails =
      draft.forSale === "Yes" && effectiveSaleMarketKind === "token" && effectiveMarketAccessMode === "selected"
        ? draft.selectedMarketEmails
        : draft.forSale === "Yes" && effectiveSaleMarketKind === "share" && draft.selectedShareMarketEmail
          ? [draft.selectedShareMarketEmail]
          : [];
    for (const email of [...shareToEmails, ...saleEmails]) {
      activeGrantEmails.add(email);
    }
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
  const defaultUserPolicy: ShareUserPolicy = {
    parallelLimit: parallelLimit >= 0 ? parallelLimit : undefined,
    tokenLimit: tokenLimit >= 0 ? tokenLimit : undefined,
    tokenPeriod: "lifetime",
    expiresAt:
      !draft.expiresPermanent && expiresIso
        ? new Date(expiresIso).getTime()
        : undefined,
  };
  const userGrants: ShareUserGrantMap = {};
  for (const email of activeGrantEmails) {
    const previous = draft.userGrants[email];
    userGrants[email] = {
      ...previous,
      email,
      role: email === ownerEmail ? "owner" : "shareto",
      active: true,
      policy: previous?.policy ?? { ...defaultUserPolicy },
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
  if (draft.userGrantsSupported) patch.userGrants = userGrants;
  if (expiresIso) patch.expiresAt = expiresIso;
  return patch;
}

export function shareEditPatchFingerprint(patch: ShareSettingsPatch) {
  return JSON.stringify(patch);
}
