"use client";

import * as React from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { Button, Chip, Modal } from "@heroui/react";
import { Check, Copy, Loader2, LogIn, Minus, Plus, RotateCcw, Server } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { CountryFlag } from "@/components/common/country-flag";
import { SegmentedControl } from "@/components/common/segmented-control";
import { marketEligibilityFromError } from "@/components/common/seller-approval-dialog";
import { buildClientInstallCommand } from "@/components/dashboard/install-guide-dialog";
import { ClientCheckoutForm } from "@/components/dashboard/client-market/client-checkout-form";
import { clientCheckoutState, preserveClientDrafts, type ClientDraft as Draft, type ClientSubdomainCheck as SubdomainCheck } from "@/lib/client-checkout";
import { ProvisionJobLog } from "@/components/dashboard/provision-job-log";
import {
  persistedClientMarketProviderSelection,
  recommendedClientMarketProviderId,
} from "@/components/dashboard/client-market/host-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  ApiError,
  cancelClientMarketQuote,
  checkClientTunnelSubdomainAvailability,
  commitClientMarketQuote,
  createClientMarketQuote,
  getClientMarketBatch,
  getClientMarketProviderSupply,
} from "@/lib/api";
import type {
  ClientMarketAllocationQuote,
  ClientMarketHost,
  ClientMarketProvider,
  CreateClientRegionsPersist,
  CreateClientSelectionPersist,
  MarketProviderRecommendationMode,
  ProvisioningJob,
} from "@/lib/types";
import { formatUsdMoney } from "@/lib/market-money";
import { walletTopupHref } from "@/lib/market-wallet-money";
import { usePersistentState } from "@/lib/use-persistent-state";

const PROVIDERS_KEY = "cc_switch_router_create_client_providers_v2";
const REGIONS_KEY = "cc_switch_router_create_client_regions_v2";

type Phase = "form" | "quote" | "running" | "complete";
type CreateMode = "manual" | "online";

function randomSubdomain() {
  const letters = "abcdefghijklmnopqrstuvwxyz";
  const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789";
  let output = letters[Math.floor(Math.random() * letters.length)];
  for (let index = 1; index < 10; index += 1) output += alphabet[Math.floor(Math.random() * alphabet.length)];
  return output;
}

function normalizeDraftSubdomain(value: string) {
  return value.trim().toLowerCase();
}

function isValidDraftSubdomain(value: string) {
  return (
    value.length >= 6 &&
    value.length <= 30 &&
    /^[a-z][a-z0-9-]*[a-z0-9]$/.test(value) &&
    !value.includes("--") &&
    !["admin", "api", "cdn-cgi", "router", "www"].includes(value)
  );
}

function normalizeProviderPersist(value: unknown): CreateClientSelectionPersist {
  if (!value || typeof value !== "object") return { mode: "official_default", providerIds: [] };
  const candidate = value as Partial<CreateClientSelectionPersist>;
  return {
    mode: candidate.mode === "custom" ? "custom" : "official_default",
    providerIds: Array.isArray(candidate.providerIds)
      ? candidate.providerIds.filter((item): item is string => typeof item === "string")
      : [],
  };
}

function normalizeRegionPersist(value: unknown): CreateClientRegionsPersist {
  if (!value || typeof value !== "object") return { mode: "all", codes: [] };
  const candidate = value as Partial<CreateClientRegionsPersist>;
  return {
    mode: candidate.mode === "subset" ? "subset" : "all",
    codes: Array.isArray(candidate.codes)
      ? candidate.codes.filter((item): item is string => typeof item === "string")
      : [],
  };
}

function formatOffer(
  dailyRateMinor: number | undefined,
  locale: string,
  _currency = "USD",
  freeDurationDays?: number,
  cyclePriceMinor?: number,
) {
  if (cyclePriceMinor != null) {
    const amount = formatUsdMoney(cyclePriceMinor, locale);
    return locale.startsWith("zh") ? `${amount} / 月` : `${amount} / month`;
  }
  if (dailyRateMinor == null) {
    if (freeDurationDays != null) {
      return locale.startsWith("zh")
        ? `免费 · ${freeDurationDays} 天`
        : `Free · ${freeDurationDays} ${freeDurationDays === 1 ? "day" : "days"}`;
    }
    return locale.startsWith("zh") ? "免费 · 永久" : "Free · permanent";
  }
  const amount = formatUsdMoney(dailyRateMinor, locale);
  return locale.startsWith("zh") ? `${amount} / 天` : `${amount} / day`;
}

function secondsRemaining(value?: string) {
  if (!value) return 0;
  return Math.max(0, Math.ceil((Date.parse(value) - Date.now()) / 1000));
}

export function CreateClientDialog({
  open,
  onOpenChange,
  fixedHost,
  onCreated,
  onMarketEligibilityRequired,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  fixedHost?: ClientMarketHost | null;
  onCreated?: () => void;
  onMarketEligibilityRequired?: (host: ClientMarketHost, eligibility: ClientMarketHost["eligibility"]) => void;
}) {
  const { locale, t } = useLocaleText();
  const router = useRouter();
  const searchParams = useSearchParams();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;
  const ownerEmail = session?.user?.email?.trim() || t("dashboard.installClientCommandOwnerPlaceholder");
  const [mode, setMode] = React.useState<CreateMode>("online");
  const [phase, setPhase] = React.useState<Phase>("form");
  const [providers, setProviders] = React.useState<ClientMarketProvider[]>([]);
  const [providerSupplyLoaded, setProviderSupplyLoaded] = React.useState(false);
  const [officialProviderId, setOfficialProviderId] = React.useState<string>();
  const [recommendationMode, setRecommendationMode] = React.useState<
    MarketProviderRecommendationMode | string
  >("off");
  const [providerPersist, setProviderPersist] = usePersistentState<CreateClientSelectionPersist>(
    PROVIDERS_KEY,
    { mode: "official_default", providerIds: [] },
  );
  const [regionPersist, setRegionPersist] = usePersistentState<CreateClientRegionsPersist>(
    REGIONS_KEY,
    { mode: "all", codes: [] },
  );
  const [quantity, setQuantity] = React.useState(1);
  const [quote, setQuote] = React.useState<ClientMarketAllocationQuote | null>(null);
  const [drafts, setDrafts] = React.useState<Record<string, Draft>>({});
  /** Drafts rescued from an expired quote, keyed by hostId so they survive the new
   *  quote's fresh item ids. Losing a filled-in subdomain and password on a 120s
   *  timer was the single most punishing moment in this flow. */
  const preservedDrafts = React.useRef<Record<string, Draft>>({});
  const [subdomainChecks, setSubdomainChecks] = React.useState<Record<string, SubdomainCheck>>({});
  const [jobs, setJobs] = React.useState<Record<string, ProvisioningJob>>({});
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");
  const [copied, setCopied] = React.useState(false);
  const [clock, setClock] = React.useState(0);
  const pollGeneration = React.useRef(0);
  const quoteRequestGeneration = React.useRef(0);
  const liveQuote = React.useRef<ClientMarketAllocationQuote | null>(null);
  const [expired, setExpired] = React.useState(false);
  const [submissionUncertain, setSubmissionUncertain] = React.useState(false);

  const commitKeyRef = React.useRef<{ quoteId: string; key: string } | null>(null);
  const subdomainCheckGeneration = React.useRef(0);

  const safeProviders = React.useMemo(() => normalizeProviderPersist(providerPersist), [providerPersist]);
  const safeRegions = React.useMemo(() => normalizeRegionPersist(regionPersist), [regionPersist]);
  /** Clients-page pool create only allocates free Hosts; Client Market fixed Host may be paid. */
  const freeOnly = !fixedHost;
  const availableProviderIds = React.useMemo(() => new Set(providers.map((provider) => provider.providerId)), [providers]);
  const defaultProviderId = React.useMemo(
    () => recommendedClientMarketProviderId(providers, officialProviderId, recommendationMode),
    [officialProviderId, providers, recommendationMode],
  );
  const selectedProviderIds = React.useMemo(() => {
    if (fixedHost?.providerId) return [fixedHost.providerId];
    if (safeProviders.mode === "official_default") {
      return defaultProviderId && availableProviderIds.has(defaultProviderId) ? [defaultProviderId] : [];
    }
    return safeProviders.providerIds.filter((id) => availableProviderIds.has(id));
  }, [availableProviderIds, defaultProviderId, fixedHost?.providerId, safeProviders]);
  const selectedProviders = React.useMemo(
    () => providers.filter((provider) => selectedProviderIds.includes(provider.providerId)),
    [providers, selectedProviderIds],
  );
  const providerFreeIdle = React.useCallback((provider: ClientMarketProvider) => {
    if (typeof provider.countries?.[0]?.freeIdle === "number") {
      return provider.countries.reduce((sum, country) => sum + (country.freeIdle || 0), 0);
    }
    return Math.max(0, (provider.freeHostTotal || 0) - (provider.freeAllocatedTotal || 0));
  }, []);
  const providerFreeTotal = React.useCallback((provider: ClientMarketProvider) => {
    if (typeof provider.countries?.[0]?.freeTotal === "number") {
      return provider.countries.reduce((sum, country) => sum + (country.freeTotal || 0), 0);
    }
    return provider.freeHostTotal || 0;
  }, []);
  const providerOptions = React.useMemo(
    () =>
      providers.map((provider) => {
        const idle = freeOnly ? providerFreeIdle(provider) : provider.idleTotal;
        const total = freeOnly ? providerFreeTotal(provider) : provider.hostTotal;
        return {
          value: provider.providerId,
          label: [
            provider.ownerEmail,
            provider.official ? t("createClient.official") : null,
            `${idle}/${total}`,
          ]
            .filter(Boolean)
            .join(" · "),
        };
      }),
    [freeOnly, providerFreeIdle, providerFreeTotal, providers, t],
  );
  /** CompactRegionMultiSelect treats `[]` as “all”; mirror that when every owner is selected. */
  const providerFilterValues = React.useMemo(() => {
    if (
      providers.length > 0 &&
      selectedProviderIds.length === providers.length &&
      providers.every((provider) => selectedProviderIds.includes(provider.providerId))
    ) {
      return [];
    }
    return selectedProviderIds;
  }, [providers, selectedProviderIds]);
  const setSelectedProviders = React.useCallback(
    (ids: string[]) => {
      setProviderPersist(persistedClientMarketProviderSelection(
        ids,
        providers,
      ));
    },
    [providers, setProviderPersist],
  );
  const regionOptions = React.useMemo(() => {
    const counts = new Map<string, { idle: number; total: number }>();
    for (const provider of selectedProviders) {
      for (const country of provider.countries) {
        const value = counts.get(country.code) || { idle: 0, total: 0 };
        if (freeOnly) {
          value.idle += country.freeIdle || 0;
          value.total += country.freeTotal || 0;
        } else {
          value.idle += country.idle;
          value.total += country.total;
        }
        counts.set(country.code, value);
      }
    }
    return Array.from(counts.entries())
      .map(([code, count]) => ({ code, ...count }))
      .filter((region) => (freeOnly ? region.total > 0 || region.idle > 0 : true))
      .sort((left, right) => left.code.localeCompare(right.code));
  }, [freeOnly, selectedProviders]);
  const selectedCountryCodes = React.useMemo(() => {
    if (fixedHost?.countryCode) return [fixedHost.countryCode];
    const options = regionOptions.map((region) => region.code);
    if (safeRegions.mode === "all") return options;
    const selected = safeRegions.codes.filter((code) => options.includes(code));
    return selected.length ? selected : options;
  }, [fixedHost?.countryCode, regionOptions, safeRegions]);
  const capacity = React.useMemo(() => {
    if (fixedHost) return fixedHost.status === "idle" ? 1 : 0;
    const selected = new Set(selectedCountryCodes);
    return regionOptions.reduce((sum, region) => sum + (selected.has(region.code) ? region.idle : 0), 0);
  }, [fixedHost, regionOptions, selectedCountryCodes]);
  const regionNames = React.useMemo(() => new Intl.DisplayNames([locale], { type: "region" }), [locale]);

  React.useEffect(() => {
    if (!open) return;
    pollGeneration.current += 1;
    if (liveQuote.current) void cancelClientMarketQuote(liveQuote.current.id).catch(() => undefined);
    liveQuote.current = null;
    setPhase("form");
    setSubmissionUncertain(false);
    setExpired(false);
    preservedDrafts.current = {};
    setMode("online");
    setQuote(null);
    commitKeyRef.current = null;
    setDrafts({});
    setSubdomainChecks({});
    subdomainCheckGeneration.current += 1;
    setJobs({});
    setQuantity(1);
    setError("");
    setLoading(true);
    setProviderSupplyLoaded(false);
    setRecommendationMode("off");
    if (fixedHost) { setLoading(false); return; }
    let active = true;
    getClientMarketProviderSupply()
      .then((response) => {
        if (!active) return;
        setProviders(response.providers);
        setOfficialProviderId(response.officialProviderId);
        setRecommendationMode(response.recommendationMode || "off");
        setProviderSupplyLoaded(true);
      })
      .catch((reason) => { if (active) setError(reason instanceof Error ? reason.message : String(reason)); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [open, fixedHost?.id]);

  React.useEffect(() => {
    if (!providerSupplyLoaded || safeProviders.mode !== "custom") return;
    const valid = safeProviders.providerIds.filter((id) => availableProviderIds.has(id));
    if (!valid.length && safeProviders.providerIds.length) {
      setProviderPersist({ mode: "official_default", providerIds: [] });
    } else if (valid.length !== safeProviders.providerIds.length) {
      setProviderPersist({ mode: "custom", providerIds: valid });
    }
  }, [availableProviderIds, providerSupplyLoaded, safeProviders, setProviderPersist]);

  React.useEffect(() => {
    if (!providerSupplyLoaded || safeProviders.mode !== "official_default") return;
    if (defaultProviderId && availableProviderIds.has(defaultProviderId)) return;
    if (!providers.length) return;
    // No compatible default Provider is available — fall back to every host
    // owner so non-official supply remains reachable.
    setProviderPersist({
      mode: "custom",
      providerIds: providers.map((provider) => provider.providerId),
    });
  }, [
    availableProviderIds,
    defaultProviderId,
    providerSupplyLoaded,
    providers,
    safeProviders.mode,
    setProviderPersist,
  ]);

  React.useEffect(() => {
    const options = regionOptions.map((region) => region.code);
    if (!options.length) return;
    if (safeRegions.mode === "all") {
      if (safeRegions.codes.join(",") !== options.join(",")) setRegionPersist({ mode: "all", codes: options });
      return;
    }
    const intersection = safeRegions.codes.filter((code) => options.includes(code));
    if (!intersection.length) setRegionPersist({ mode: "all", codes: options });
    else if (intersection.length !== safeRegions.codes.length) setRegionPersist({ mode: "subset", codes: intersection });
  }, [regionOptions, safeRegions, setRegionPersist]);

  React.useEffect(() => {
    if (!open || phase !== "quote") return;
    const timer = window.setInterval(() => setClock((value) => value + 1), 1_000);
    return () => window.clearInterval(timer);
  }, [open, phase]);

  React.useEffect(() => {
    if (!quote || phase !== "quote" || loading || submissionUncertain || expired || secondsRemaining(quote.expiresAt) > 0) return;
    preservedDrafts.current = preserveClientDrafts(quote, drafts);
    void cancelClientMarketQuote(quote.id).catch(() => undefined);
    setExpired(true);
  }, [clock, drafts, expired, loading, phase, quote, submissionUncertain]);

  React.useEffect(() => () => {
    quoteRequestGeneration.current += 1;
    pollGeneration.current += 1;
    if (liveQuote.current) void cancelClientMarketQuote(liveQuote.current.id).catch(() => undefined);
  }, []);

  React.useEffect(() => {
    if (!open || phase !== "quote" || !quote) return;
    const generation = ++subdomainCheckGeneration.current;
    const values = quote.items.map((item) => ({
      id: item.id,
      value: normalizeDraftSubdomain(drafts[item.id]?.subdomain || ""),
    }));
    const counts = new Map<string, number>();
    for (const { value } of values) {
      if (value) counts.set(value, (counts.get(value) || 0) + 1);
    }
    const candidates: Array<{ id: string; value: string }> = [];
    const immediate: Record<string, SubdomainCheck> = {};
    for (const item of values) {
      if (!item.value) {
        immediate[item.id] = { value: item.value, status: "unavailable", message: t("createClient.subdomainRequired") };
      } else if (!isValidDraftSubdomain(item.value)) {
        immediate[item.id] = { value: item.value, status: "unavailable", message: t("createClient.subdomainInvalid") };
      } else if ((counts.get(item.value) || 0) > 1) {
        immediate[item.id] = { value: item.value, status: "unavailable", message: t("createClient.subdomainDuplicate") };
      } else {
        immediate[item.id] = { value: item.value, status: "checking" };
        candidates.push(item);
      }
    }
    setSubdomainChecks(immediate);
    if (!candidates.length) return;

    const timer = window.setTimeout(() => {
      void Promise.all(
        candidates.map(async (item) => {
          try {
            const result = await checkClientTunnelSubdomainAvailability(item.value);
            return {
              ...item,
              check: result.available
                ? ({ value: item.value, status: "available" } as SubdomainCheck)
                : ({ value: item.value, status: "unavailable", message: t("createClient.subdomainTaken") } as SubdomainCheck),
            };
          } catch {
            return {
              ...item,
              check: { value: item.value, status: "error", message: t("createClient.subdomainCheckFailed") } as SubdomainCheck,
            };
          }
        }),
      ).then((results) => {
        if (subdomainCheckGeneration.current !== generation) return;
        setSubdomainChecks((current) => {
          const next = { ...current };
          for (const result of results) next[result.id] = result.check;
          return next;
        });
      });
    }, 350);
    return () => window.clearTimeout(timer);
  }, [drafts, open, phase, quote, t]);

  const routeMarketEligibilityError = (reason: unknown) => {
    const eligibility = marketEligibilityFromError(reason);
    if (!fixedHost || !onMarketEligibilityRequired || !eligibility) {
      return false;
    }
    if (quote) void cancelClientMarketQuote(quote.id).catch(() => undefined);
    onOpenChange(false);
    onMarketEligibilityRequired(fixedHost, eligibility);
    return true;
  };

  const requestQuote = async () => {
    if (loading) return;
    if (!authed) {
      window.dispatchEvent(new Event("router-open-login"));
      return;
    }
    if (fixedHost && !fixedHost.eligibility.allowed) {
      onOpenChange(false);
      onMarketEligibilityRequired?.(fixedHost, fixedHost.eligibility);
      return;
    }
    if ((!fixedHost && (!selectedProviderIds.length || !selectedCountryCodes.length)) || capacity < quantity) {
      setError(capacity < quantity ? t("createClient.noCapacity") : t("createClient.selectionRequired"));
      return;
    }
    const generation = ++quoteRequestGeneration.current;
    if (quote) preservedDrafts.current = preserveClientDrafts(quote, drafts);
    setLoading(true);
    setError("");
    try {
      if (quote) await cancelClientMarketQuote(quote.id);
      const next = await createClientMarketQuote({
        providerIds: selectedProviderIds,
        countryCodes: selectedCountryCodes,
        count: fixedHost ? 1 : quantity,
        hostId: fixedHost?.id,
      });
      if (generation !== quoteRequestGeneration.current) {
        void cancelClientMarketQuote(next.id).catch(() => undefined);
        return;
      }
      liveQuote.current = next;
      setExpired(false);
      commitKeyRef.current = null;
      setQuote(next);
      setDrafts(
        Object.fromEntries(
          next.items.map((item) => [
            item.id,
            preservedDrafts.current[item.hostId] ?? {
              subdomain: searchParams.get("focusHost") === item.hostId && isValidDraftSubdomain(searchParams.get("draftSubdomain") || "")
                ? searchParams.get("draftSubdomain")! : randomSubdomain(),
              password: "",
              autoRenew: searchParams.get("focusHost") === item.hostId && searchParams.get("draftAutoRenew") === "1",
            },
          ]),
        ),
      );
      preservedDrafts.current = {};
      setSubdomainChecks({});
      setPhase("quote");
    } catch (reason) {
      if (generation === quoteRequestGeneration.current && !routeMarketEligibilityError(reason)) {
        setError(t("clientCheckout.loadFailed"));
      }
    } finally {
      if (generation === quoteRequestGeneration.current) setLoading(false);
    }
  };

  // Defer one tick so development Strict Mode can cancel its probe mount.
  const requestQuoteRef = React.useRef(requestQuote);
  requestQuoteRef.current = requestQuote;
  React.useEffect(() => {
    if (!open || !fixedHost || !authed) return;
    const timer = window.setTimeout(() => void requestQuoteRef.current(), 0);
    return () => { window.clearTimeout(timer); quoteRequestGeneration.current += 1; };
  }, [open, fixedHost?.id, authed]);

  const pollBatch = async (batchId: string, generation: number) => {
    for (let attempt = 0; attempt < 1_000; attempt += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 1_500));
      if (pollGeneration.current !== generation) return;
      const batch = await getClientMarketBatch(batchId).catch(() => null);
      if (!batch) continue;
      setJobs(Object.fromEntries(batch.jobs.map((job) => [job.id, job])));
      if (batch.status !== "running") {
        setPhase("complete");
        onCreated?.();
        return;
      }
    }
    setError(t("createClient.failed"));
    setPhase("complete");
  };

  const commit = async () => {
    if (!quote) return;
    if (loading || (!submissionUncertain && (expired || secondsRemaining(quote.expiresAt) <= 0))) return;
    const checkout = clientCheckoutState(quote);
    if (checkout.unsupported || checkout.shortfall) return;
    const items = quote.items.map((item) => ({
      quoteItemId: item.id,
      offerRevision: item.offerRevision,
      subdomain: normalizeDraftSubdomain(drafts[item.id]?.subdomain || ""),
      password: drafts[item.id]?.password || "",
      autoRenew: item.pricingModel === "prepaid_calendar_month"
        && drafts[item.id]?.autoRenew === true,
    }));
    if (items.some((item) => !item.subdomain.trim())) {
      setError(t("createClient.subdomainRequired"));
      return;
    }
    if (items.some((item) => !isValidDraftSubdomain(item.subdomain))) {
      setError(t("createClient.subdomainInvalid"));
      return;
    }
    if (new Set(items.map((item) => item.subdomain)).size !== items.length) {
      setError(t("createClient.subdomainDuplicate"));
      return;
    }
    if (items.some((item) => item.password.length < 8)) {
      setError(t("createClient.passwordHint"));
      return;
    }
    setLoading(true);
    setError("");
    try {
      if (!submissionUncertain) {
        setSubdomainChecks(Object.fromEntries(items.map((item) => [item.quoteItemId, { value: item.subdomain, status: "checking" }])));
        const availability = await Promise.all(
          items.map(async (item) => {
            try {
              return { item, result: await checkClientTunnelSubdomainAvailability(item.subdomain) };
            } catch {
              return { item, error: true as const };
            }
          }),
        );
        setSubdomainChecks(Object.fromEntries(availability.map(({ item, ...entry }) => [
          item.quoteItemId,
          "error" in entry
            ? { value: item.subdomain, status: "error", message: t("createClient.subdomainCheckFailed") }
            : entry.result.available
              ? { value: item.subdomain, status: "available" }
              : { value: item.subdomain, status: "unavailable", message: t("createClient.subdomainTaken") },
        ])) as Record<string, SubdomainCheck>);
        if (availability.some((entry) => "error" in entry)) {
          setError(t("createClient.subdomainCheckFailed"));
          return;
        }
        if (availability.some((entry) => entry.result?.available === false)) {
          setError(t("createClient.subdomainTaken"));
          return;
        }
      }
      if (commitKeyRef.current?.quoteId !== quote.id) {
        commitKeyRef.current = {
          quoteId: quote.id,
          key: globalThis.crypto?.randomUUID?.() || `${quote.id}:${Date.now()}`,
        };
      }
      setSubmissionUncertain(true);
      liveQuote.current = null;
      const response = await commitClientMarketQuote(
        quote.id,
        items,
        commitKeyRef.current.key,
      );
      setSubmissionUncertain(false);
      liveQuote.current = null;
      preservedDrafts.current = {};
      setDrafts({});
      setSubdomainChecks({});
      setJobs(Object.fromEntries(response.jobIds.map((id) => [id, { id, status: "pending", jobType: "create", phase: "pending", log: "", createdAt: "", updatedAt: "" } as ProvisioningJob])));
      setPhase("running");
      const generation = pollGeneration.current;
      void pollBatch(response.batchId, generation);
    } catch (reason) {
      if (reason instanceof ApiError && reason.status < 500) setSubmissionUncertain(false);
      if (!routeMarketEligibilityError(reason)) {
        if (reason instanceof ApiError && (reason.code === "MARKET_WALLET_REQUIRED" || reason.status === 410 || reason.status === 409)) {
          // The rejected transaction created no batch. Keep the form and explicitly requote.
          preservedDrafts.current = preserveClientDrafts(quote, drafts);
          setExpired(true);
          setError(t(reason.code === "MARKET_WALLET_REQUIRED" ? "clientCheckout.fundingChanged" : "clientCheckout.expired"));
        } else {
          setError(t("clientCheckout.commitFailed"));
        }
      }
    } finally {
      setLoading(false);
    }
  };

  const cancelQuote = async () => {
    if (quote) await cancelClientMarketQuote(quote.id).catch(() => undefined);
    liveQuote.current = null;
    setExpired(false);
    setQuote(null);
    setDrafts({});
    setSubdomainChecks({});
    subdomainCheckGeneration.current += 1;
    setPhase("form");
  };

  const close = (nextOpen: boolean) => {
    if (!nextOpen && (phase === "running" || (loading && submissionUncertain))) return;
    if (!nextOpen) { quoteRequestGeneration.current += 1; preservedDrafts.current = {}; }
    if (!nextOpen && phase === "quote" && quote && !submissionUncertain) void cancelClientMarketQuote(quote.id).catch(() => undefined);
    if (!nextOpen) subdomainCheckGeneration.current += 1;
    pollGeneration.current += 1;
    if (!nextOpen) { liveQuote.current = null; setDrafts({}); }
    onOpenChange(nextOpen);
  };

  const installCommand = React.useMemo(
    () =>
      buildClientInstallCommand({
        ownerEmail,
        passwordPlaceholder: t("createClient.manualPasswordPlaceholder"),
      }),
    [ownerEmail, t],
  );
  const quoteSeconds = secondsRemaining(quote?.expiresAt);
  const checkout = quote ? clientCheckoutState(quote) : null;
  const topup = async () => {
    if (!quote?.walletFunding || loading) return;
    setLoading(true);
    try {
      await cancelClientMarketQuote(quote.id);
      // Only non-sensitive configuration crosses navigation; passwords never leave memory.
      const returnTo = new URLSearchParams();
      if (fixedHost) {
        returnTo.set("focusHost", fixedHost.id);
        const draft = drafts[quote.items[0]?.id];
        if (draft?.subdomain) returnTo.set("draftSubdomain", draft.subdomain);
        if (draft?.autoRenew) returnTo.set("draftAutoRenew", "1");
      }
      liveQuote.current = null;
      const href = walletTopupHref(quote.walletFunding.shortfallUnits, `/client-market/?${returnTo}`);
      close(false);
      router.push(href);
    } catch { setError(t("clientCheckout.loadFailed")); }
    finally { setLoading(false); }
  };
  const subdomainsCanCommit = !!quote && quote.items.every((item) => {
    const value = normalizeDraftSubdomain(drafts[item.id]?.subdomain || "");
    const check = subdomainChecks[item.id];
    return isValidDraftSubdomain(value) && check?.value === value && (check.status === "available" || check.status === "error");
  });
  const jobList = Object.values(jobs);
  const successes = jobList.filter((job) => job.status === "succeeded").length;
  const failures = jobList.filter((job) => job.status === "failed").length;
  const showOfficialCapacityHint =
    providerSupplyLoaded &&
    !loading &&
    !fixedHost &&
    safeProviders.mode === "official_default" &&
    capacity === 0;

  return (
    <>
    <Modal.Backdrop isOpen={open} onOpenChange={close} isDismissable={!loading && phase !== "running"}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light min-w-0 w-[min(560px,calc(100vw-2rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{phase === "quote" ? t("clientCheckout.confirmTitle") : t("createClient.title")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid min-w-0 max-h-[min(76vh,680px)] grid-cols-[minmax(0,1fr)] gap-4 overflow-y-auto">
            {phase === "form" && !fixedHost ? (
              <SegmentedControl
                value={mode}
                onChange={setMode}
                ariaLabel={t("createClient.title")}
                size="md"
                fullWidth
                items={[
                  { id: "online", label: t("createClient.tabOnline") },
                  { id: "manual", label: t("createClient.tabManual") },
                ]}
              />
            ) : null}

            {phase === "form" && mode === "manual" && !fixedHost ? (
              <div className="grid gap-3"><p className="text-sm text-muted-foreground">{t("createClient.manualDescription")}</p><pre className="overflow-x-auto whitespace-pre-wrap break-all rounded-md border bg-slate-50 p-3 font-mono text-xs leading-6">{installCommand}</pre></div>
            ) : null}

            {phase === "form" && (mode === "online" || fixedHost) ? (
              <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-5">
                {loading ? <div className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("createClient.loadingSupply")}</div> : null}
                {freeOnly ? (
                  <p className="text-xs leading-relaxed text-muted-foreground">
                    {t("createClient.freeOnlyHint")}
                  </p>
                ) : null}
                {fixedHost ? (
                  <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 rounded-md border p-3">
                    <div className="flex flex-wrap items-center gap-2"><Server className="h-4 w-4" /><strong className="text-sm">{fixedHost.hostname || fixedHost.ip}</strong><Chip size="sm" variant="soft">{fixedHost.countryCode || "-"}</Chip></div>
                    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 text-xs text-muted-foreground"><span className="min-w-0 truncate" title={fixedHost.hostOwnerEmail}>{fixedHost.hostOwnerEmail}</span><span className="font-medium text-foreground">{formatOffer(fixedHost.dailyRateMinor, locale, fixedHost.currency, fixedHost.freeDurationDays, fixedHost.cyclePriceMinor)}</span></div>
                  </section>
                ) : (
                  <>
                    <div className="grid min-w-0 gap-5 sm:grid-cols-2">
                      <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2">
                        <span className="text-sm font-medium">{t("createClient.hostProvider")}</span>
                        <CompactRegionMultiSelect
                          values={providerFilterValues}
                          onChange={setSelectedProviders}
                          options={providerOptions}
                          allLabel={t("createClient.hostOwnersAll")}
                          moreLabel={(count) => t("createClient.hostOwnersMore", { count })}
                          clearLabel={t("createClient.clearOwners")}
                          ariaLabel={t("createClient.filterHostOwners")}
                          className="w-full"
                        />
                        {showOfficialCapacityHint ? (
                          <p className="text-xs text-amber-800">
                            {officialProviderId
                              ? t("createClient.officialUnavailable")
                              : t("createClient.officialNotConfigured")}
                          </p>
                        ) : null}
                      </section>
                      <section className="grid min-w-0 gap-2">
                        <span className="text-sm font-medium">{t("createClient.regions")}</span>
                        <div className="grid max-h-40 gap-1 overflow-y-auto rounded-md border p-2">
                          {regionOptions.map((region) => {
                            const checked = selectedCountryCodes.includes(region.code);
                            return (
                              <label
                                key={region.code}
                                className="flex cursor-pointer items-center gap-2 rounded px-1 py-1.5 text-sm hover:bg-slate-50"
                              >
                                <input
                                  type="checkbox"
                                  checked={checked}
                                  onChange={() => {
                                    const current = new Set(selectedCountryCodes);
                                    if (current.has(region.code)) current.delete(region.code);
                                    else current.add(region.code);
                                    setRegionPersist({ mode: "subset", codes: Array.from(current) });
                                  }}
                                />
                                <CountryFlag code={region.code} />
                                <span className="min-w-0 flex-1 truncate">{regionNames.of(region.code) || region.code}</span>
                                <span className="font-mono text-xs text-muted-foreground">
                                  {region.idle}/{region.total}
                                </span>
                              </label>
                            );
                          })}
                        </div>
                      </section>
                    </div>
                    <section className="flex flex-wrap items-center justify-between gap-3"><div><div className="text-sm font-medium">{t("createClient.quantity")}</div><div className="text-xs text-muted-foreground">{t(freeOnly ? "createClient.capacityFree" : "createClient.capacity", { idle: capacity })}</div></div><div className="inline-flex items-center gap-2"><Button isIconOnly size="sm" variant="outline" aria-label={t("createClient.decreaseQuantity")} isDisabled={quantity <= 1} onClick={() => setQuantity(1)}><Minus className="h-4 w-4" /></Button><span className="w-8 text-center font-mono">{quantity}</span><Button isIconOnly size="sm" variant="outline" aria-label={t("createClient.increaseQuantity")} isDisabled={quantity >= 2 || quantity >= capacity} onClick={() => setQuantity(2)}><Plus className="h-4 w-4" /></Button></div></section>
                  </>
                )}
              </div>
            ) : null}

            {phase === "quote" && quote ? (
              <ClientCheckoutForm quote={quote} drafts={drafts} checks={subdomainChecks} seconds={expired ? 0 : quoteSeconds} busy={loading || submissionUncertain}
                onDraft={(id, patch) => setDrafts((current) => ({ ...current, [id]: { ...current[id], ...patch } }))}
                onRandom={(id) => setDrafts((current) => ({ ...current, [id]: { ...current[id], subdomain: randomSubdomain() } }))} />
            ) : null}

            {(phase === "running" || phase === "complete") ? (
              <div className="grid min-w-0 gap-4">
                {phase === "complete" ? <p role="status" className="text-sm font-medium">{t("createClient.resultSummary", { successes, failures })}</p> : null}
                {jobList.map((job) => <section key={job.id} className="grid min-w-0 gap-2 border-b border-border pb-4 last:border-0">
                  <strong className="break-all text-sm">{job.subdomain || t("createClient.title")}</strong>
                  <p role="status" className="text-sm text-muted-foreground">{t(job.status === "failed" ? "createClient.failed" : job.status === "succeeded" ? "createClient.successTitle" : job.phase === "waiting_for_client" ? "clientCheckout.waiting" : job.status === "pending" ? "clientCheckout.queued" : "clientCheckout.installing")}</p>
                  {job.status === "failed" ? <p className="text-xs text-muted-foreground">{t("clientCheckout.failedHint")}</p> : null}
                  {job.status === "succeeded" && job.clientUrl && /^https?:\/\//.test(job.clientUrl) ? <a href={job.clientUrl} target="_blank" rel="noopener noreferrer" className="justify-self-start text-sm font-medium text-primary hover:underline">{t("clientCheckout.openClient")}</a> : null}
                  <details className="min-w-0 text-xs text-muted-foreground"><summary className="cursor-pointer">{t("clientCheckout.logs")}</summary><ProvisionJobLog log={job.log || ""} phase={job.status === "failed" ? "failed" : job.status === "succeeded" ? "success" : "running"} /></details>
                </section>)}
              </div>
            ) : null}
            {checkout?.shortfall && !checkout.unsupported ? <p className="text-xs text-muted-foreground">{t("clientCheckout.topupHint")}</p> : null}
            {error ? <p role="alert" className="text-sm text-destructive">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            {phase === "quote" && !fixedHost && !submissionUncertain ? <Button variant="ghost" isDisabled={loading} onClick={() => void cancelQuote()}><RotateCcw className="h-4 w-4" />{t("clientMarket.back")}</Button> : <Button variant="ghost" isDisabled={phase === "running" || loading} onClick={() => close(false)}>{t("common.close")}</Button>}
            {phase === "form" && mode === "manual" && !fixedHost ? <Button variant="outline" onClick={() => void navigator.clipboard.writeText(installCommand).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1500); })}>{copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}{copied ? t("dashboard.connectDialog.copyOk") : t("dashboard.connectDialog.copy")}</Button> : null}
            {phase === "form" && (mode === "online" || fixedHost) ? <Button variant="primary" isDisabled={loading || authLoading || (authed && capacity < quantity)} onClick={() => void requestQuote()}>{!authed ? <LogIn className="h-4 w-4" /> : loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{!authed ? t("createClient.login") : !fixedHost && safeProviders.mode === "official_default" && capacity === 0 ? t("createClient.officialNoCapacityButton") : t(fixedHost ? "clientCheckout.requote" : "createClient.selectHosts")}</Button> : null}
            {phase === "quote" && submissionUncertain ? <Button variant="primary" isDisabled={loading} onPress={() => void commit()}>{loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("common.retry")}</Button>
              : phase === "quote" && (expired || quoteSeconds <= 0) ? <Button variant="primary" isDisabled={loading} onPress={() => void requestQuote()}>{loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("clientCheckout.requote")}</Button>
              : phase === "quote" && checkout?.shortfall && !checkout.unsupported ? <Button variant="primary" isDisabled={loading} onPress={() => void topup()}>{t("clientCheckout.topup")}</Button>
              : phase === "quote" ? <Button variant="primary" isDisabled={loading || !subdomainsCanCommit || checkout?.unsupported} onPress={() => void commit()}>{loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t(checkout?.trial ? "clientCheckout.trialAction" : checkout?.paid ? "clientCheckout.paidAction" : "createClient.create")}</Button> : null}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
    </>
  );
}
