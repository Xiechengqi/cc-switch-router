"use client";

import * as React from "react";
import { Button, Chip, ListBox, Select, toast } from "@heroui/react";
import {
  BookOpen,
  Loader2,
  Plus,
  Power,
  RefreshCw,
  ReceiptText,
  Save,
  ShieldCheck,
  Trash2,
  WalletCards,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { BinanceApiKeyGuideDialog } from "@/components/dashboard/binance-api-key-guide-dialog";
import { BinanceReceiptHistoryDialog } from "@/components/dashboard/binance-receipt-history-dialog";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { AuthenticatedImage } from "@/components/common/authenticated-image";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  confirmBinanceAutoSettlement,
  deleteBinanceAutoSettlement,
  disableBinanceAutoSettlement,
  discoverBinanceAutoSettlement,
  enableBinanceAutoSettlement,
  getAccountPaymentProfile,
  getBinanceAutoSettlementStatus,
  getMarketBillingConfig,
  updateAccountPaymentProfile,
  verifyBinanceAutoSettlement,
} from "@/lib/api";
import {
  DEFAULT_USD_CNY_RATE_MICROS,
  formatUsdCnyRate,
} from "@/lib/market-money";
import { binanceAutoSettlementUiState } from "@/lib/binance-auto-settlement";
import type {
  BinanceAutoSettlementStatus,
  ClientMarketPaymentMethod,
  DiscoverBinanceAccountResponse,
  PaymentContact,
  PaymentContactChannel,
} from "@/lib/types";

type CryptoDraft = { token: "USDT" | "USDC"; chain: "bsc" | "base" | "eth" | "tron"; address: string };
type ContactDraft = { channel: PaymentContactChannel; handle: string };
type PaymentDraft = {
  alipayAccount: string;
  alipayQr: string;
  wechatQr: string;
  crypto: CryptoDraft[];
  custom: string;
  contacts: ContactDraft[];
};

const CRYPTO_TOKENS = ["USDT", "USDC"] as const;
const CONTACT_CHANNELS: { id: PaymentContactChannel; labelKey: "account.contact.channel.wechat" | "account.contact.channel.telegram" | "account.contact.channel.custom" }[] = [
  { id: "wechat", labelKey: "account.contact.channel.wechat" },
  { id: "telegram", labelKey: "account.contact.channel.telegram" },
  { id: "custom", labelKey: "account.contact.channel.custom" },
];
const CRYPTO_CHAINS = [
  { id: "bsc", label: "BSC" },
  { id: "base", label: "Base" },
  { id: "eth", label: "Ethereum" },
  { id: "tron", label: "TRON" },
] as const;

const emptyCrypto = (): CryptoDraft => ({ token: "USDT", chain: "bsc", address: "" });
const emptyContact = (): ContactDraft => ({ channel: "wechat", handle: "" });

const emptyPaymentDraft = (): PaymentDraft => ({
  alipayAccount: "",
  alipayQr: "",
  wechatQr: "",
  crypto: [emptyCrypto()],
  custom: "",
  contacts: [],
});

function normalizeCrypto(items: CryptoDraft[]): CryptoDraft[] {
  const cleaned = items
    .map((item): CryptoDraft => ({
      token: item.token === "USDC" ? "USDC" : "USDT",
      chain: (["bsc", "base", "eth", "tron"].includes(item.chain) ? item.chain : "bsc") as CryptoDraft["chain"],
      address: item.address.trim(),
    }))
    .filter((item) => item.address);
  return cleaned.length ? cleaned : [emptyCrypto()];
}

function normalizeContacts(items: ContactDraft[]): ContactDraft[] {
  return items
    .map((item): ContactDraft => ({
      channel: (["wechat", "telegram", "custom"].includes(item.channel) ? item.channel : "custom") as PaymentContactChannel,
      handle: item.handle.trim(),
    }))
    .filter((item) => item.handle);
}

function serializePaymentDraft(draft: PaymentDraft) {
  return JSON.stringify({
    alipayAccount: draft.alipayAccount.trim(),
    alipayQr: draft.alipayQr.trim(),
    wechatQr: draft.wechatQr.trim(),
    crypto: normalizeCrypto(draft.crypto),
    custom: draft.custom.trim(),
    contacts: normalizeContacts(draft.contacts),
  });
}

function chainLabel(chain: CryptoDraft["chain"]) {
  return CRYPTO_CHAINS.find((item) => item.id === chain)?.label || chain;
}

/** Payment details (Account → 收款信息). Blocked renters live on Client Market. */
export function AccountPaymentsPanel() {
  const { t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;
  const actorKey = authed
    ? session?.user?.id || session?.user?.email?.toLowerCase() || "authenticated"
    : "anonymous";
  const [loading, setLoading] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [draft, setDraft] = React.useState<PaymentDraft>(emptyPaymentDraft);
  const [baseline, setBaseline] = React.useState(() => serializePaymentDraft(emptyPaymentDraft()));
  const [previews, setPreviews] = React.useState<Record<string, string>>({});
  const [usdCnyRateMicros, setUsdCnyRateMicros] = React.useState(
    DEFAULT_USD_CNY_RATE_MICROS,
  );
  const [binanceStatus, setBinanceStatus] = React.useState<BinanceAutoSettlementStatus | null>(null);
  const [binanceDiscovery, setBinanceDiscovery] = React.useState<DiscoverBinanceAccountResponse | null>(null);
  const [binanceCredentialDraft, setBinanceCredentialDraft] = React.useState(() => ({
    actorKey,
    apiKey: "",
    apiSecret: "",
  }));
  const [binanceBusy, setBinanceBusy] = React.useState("");
  const [binanceStatusError, setBinanceStatusError] = React.useState("");
  const [binanceGuideOpen, setBinanceGuideOpen] = React.useState(false);
  const [binanceReceiptsOpen, setBinanceReceiptsOpen] = React.useState(false);
  const binanceDiscoveryAttemptRef = React.useRef("");
  const binanceDiscoveryInFlightRef = React.useRef(false);
  const actorKeyRef = React.useRef(actorKey);
  actorKeyRef.current = actorKey;
  const binanceApiKey = binanceCredentialDraft.actorKey === actorKey
    ? binanceCredentialDraft.apiKey
    : "";
  const binanceApiSecret = binanceCredentialDraft.actorKey === actorKey
    ? binanceCredentialDraft.apiSecret
    : "";
  const setBinanceApiKey = React.useCallback((apiKey: string) => {
    binanceDiscoveryAttemptRef.current = "";
    setBinanceDiscovery(null);
    setBinanceCredentialDraft((current) => ({
      actorKey,
      apiKey,
      apiSecret: current.actorKey === actorKey ? current.apiSecret : "",
    }));
  }, [actorKey]);
  const setBinanceApiSecret = React.useCallback((apiSecret: string) => {
    binanceDiscoveryAttemptRef.current = "";
    setBinanceDiscovery(null);
    setBinanceCredentialDraft((current) => ({
      actorKey,
      apiKey: current.actorKey === actorKey ? current.apiKey : "",
      apiSecret,
    }));
  }, [actorKey]);

  const dirty = serializePaymentDraft(draft) !== baseline;
  const binanceUiState = binanceAutoSettlementUiState(binanceStatus);
  const binanceConfigurationBlocked = binanceUiState === "unavailable"
    || binanceUiState === "regionRestricted";
  const binanceUiStateClass = binanceUiState === "active"
    ? "bg-emerald-100 text-emerald-700"
    : binanceUiState === "degraded" || binanceUiState === "regionRestricted"
      ? "bg-rose-100 text-rose-700"
      : binanceUiState === "actionRequired"
        ? "bg-amber-100 text-amber-800"
        : binanceUiState === "activationRequired"
          ? "bg-sky-100 text-sky-800"
          : "bg-slate-100 text-slate-600";

  const applyProfile = React.useCallback((methods: ClientMarketPaymentMethod[], contacts: PaymentContact[] = []) => {
    const alipay = methods.find((method) => method.kind === "alipay");
    const wechat = methods.find((method) => method.kind === "wechat");
    const customMethod = methods.find((method) => method.kind === "custom");
    const cryptoMethods = methods
      .filter((method) => method.kind === "crypto")
      .map((method) => ({
        token: (method.token === "USDC" ? "USDC" : "USDT") as CryptoDraft["token"],
        chain: (["bsc", "base", "eth", "tron"].includes(method.chain || "")
          ? method.chain
          : "bsc") as CryptoDraft["chain"],
        address: method.address || "",
      }));
    const next: PaymentDraft = {
      alipayAccount: alipay?.account || "",
      alipayQr: alipay?.qrImageUrl || "",
      wechatQr: wechat?.qrImageUrl || "",
      crypto: cryptoMethods.length ? cryptoMethods : [emptyCrypto()],
      custom: customMethod?.instructions || "",
      contacts: contacts
        .filter((contact) => contact.handle?.trim())
        .map((contact) => ({
          channel: (["wechat", "telegram", "custom"].includes(contact.channel)
            ? contact.channel
            : "custom") as PaymentContactChannel,
          handle: contact.handle,
        })),
    };
    setDraft(next);
    setBaseline(serializePaymentDraft(next));
    setPreviews(
      Object.fromEntries(
        methods
          .filter((method) => method.assetUrl)
          .map((method) => [`${method.kind}:${method.qrImageUrl || ""}`, method.assetUrl!]),
      ),
    );
  }, []);

  React.useEffect(() => {
    let active = true;
    setBinanceApiKey("");
    setBinanceApiSecret("");
    setBinanceDiscovery(null);
    binanceDiscoveryAttemptRef.current = "";
    setBinanceStatus(null);
    setBinanceBusy("");
    setBinanceStatusError("");
    if (!authed) {
      setLoading(false);
      return () => {
        active = false;
      };
    }
    setLoading(true);
    Promise.all([
      getAccountPaymentProfile(),
      getMarketBillingConfig(),
      getBinanceAutoSettlementStatus().catch((error) => {
        if (active) {
          setBinanceStatusError(error instanceof Error ? error.message : String(error));
        }
        return null;
      }),
    ])
      .then(([profile, billingConfig, nextBinanceStatus]) => {
        if (!active) return;
        applyProfile(profile.methods, profile.contacts || []);
        setUsdCnyRateMicros(billingConfig.usdCnyRateMicros);
        if (nextBinanceStatus) setBinanceStatus(nextBinanceStatus);
      })
      .catch((error) => {
        if (active) toast.danger(error instanceof Error ? error.message : String(error));
      })
      .finally(() => {
        if (active) setLoading(false);
      });
    return () => {
      active = false;
    };
  }, [actorKey, applyProfile, authed]);

  const save = async () => {
    if (!dirty || saving) return;
    const methods: ClientMarketPaymentMethod[] = [];
    if (draft.alipayAccount.trim() || draft.alipayQr.trim()) {
      methods.push({
        kind: "alipay",
        account: draft.alipayAccount.trim() || undefined,
        qrImageUrl: draft.alipayQr.trim() || undefined,
      });
    }
    if (draft.wechatQr.trim()) methods.push({ kind: "wechat", qrImageUrl: draft.wechatQr.trim() });
    for (const method of draft.crypto) {
      if (method.address.trim()) {
        methods.push({
          kind: "crypto",
          token: method.token,
          chain: method.chain,
          address: method.address.trim(),
        });
      }
    }
    if (draft.custom.trim()) methods.push({ kind: "custom", instructions: draft.custom.trim() });
    const contacts = normalizeContacts(draft.contacts);
    setSaving(true);
    try {
      const profile = await updateAccountPaymentProfile(methods, contacts);
      applyProfile(profile.methods, profile.contacts || []);
      toast.success(t("account.saved"));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };

  const discoverBinanceCredentials = React.useCallback(async () => {
    if (
      binanceBusy
      || binanceDiscoveryInFlightRef.current
      || binanceConfigurationBlocked
      || binanceApiKey.trim().length < 16
      || binanceApiSecret.trim().length < 16
    ) return;
    const attemptKey = `${actorKey}\u0000${binanceApiKey.trim()}\u0000${binanceApiSecret.trim()}`;
    if (binanceDiscoveryAttemptRef.current === attemptKey) return;
    binanceDiscoveryAttemptRef.current = attemptKey;
    binanceDiscoveryInFlightRef.current = true;
    const requestedActorKey = actorKey;
    setBinanceBusy("discover");
    try {
      const next = await discoverBinanceAutoSettlement({
        apiKey: binanceApiKey.trim(),
        apiSecret: binanceApiSecret.trim(),
      });
      if (
        actorKeyRef.current !== requestedActorKey
        || binanceDiscoveryAttemptRef.current !== attemptKey
      ) return;
      setBinanceDiscovery(next);
      setBinanceStatusError("");
    } catch (error) {
      if (
        actorKeyRef.current === requestedActorKey
        && binanceDiscoveryAttemptRef.current === attemptKey
      ) {
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      binanceDiscoveryInFlightRef.current = false;
      if (actorKeyRef.current === requestedActorKey) setBinanceBusy("");
    }
  }, [actorKey, binanceApiKey, binanceApiSecret, binanceBusy, binanceConfigurationBlocked]);

  const confirmBinanceCredentials = async () => {
    if (binanceBusy || !binanceDiscovery) return;
    const requestedActorKey = actorKey;
    setBinanceBusy("confirm");
    try {
      const next = await confirmBinanceAutoSettlement(binanceDiscovery.confirmationToken);
      if (actorKeyRef.current !== requestedActorKey) return;
      setBinanceStatus(next);
      setBinanceStatusError("");
      setBinanceApiKey("");
      setBinanceApiSecret("");
      setBinanceDiscovery(null);
      binanceDiscoveryAttemptRef.current = "";
      toast.success(t("account.binanceAuto.saved"));
    } catch (error) {
      if (actorKeyRef.current === requestedActorKey) {
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (actorKeyRef.current === requestedActorKey) setBinanceBusy("");
    }
  };

  const verifyBinanceCredentials = async () => {
    if (binanceBusy) return;
    const requestedActorKey = actorKey;
    setBinanceBusy("verify");
    try {
      const next = await verifyBinanceAutoSettlement();
      if (actorKeyRef.current !== requestedActorKey) return;
      setBinanceStatus(next);
      setBinanceStatusError("");
      toast.success(t("account.binanceAuto.verified"));
    } catch (error) {
      if (actorKeyRef.current === requestedActorKey) {
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (actorKeyRef.current === requestedActorKey) setBinanceBusy("");
    }
  };

  const enableBinanceCredentials = async () => {
    if (binanceBusy || !window.confirm(t("account.binanceAuto.enableConfirm"))) return;
    const requestedActorKey = actorKey;
    setBinanceBusy("enable");
    try {
      const next = await enableBinanceAutoSettlement();
      if (actorKeyRef.current !== requestedActorKey) return;
      setBinanceStatus(next);
      setBinanceStatusError("");
      toast.success(t("account.binanceAuto.enabled"));
    } catch (error) {
      if (actorKeyRef.current === requestedActorKey) {
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (actorKeyRef.current === requestedActorKey) setBinanceBusy("");
    }
  };

  const disableBinanceCredentials = async () => {
    if (binanceBusy || !window.confirm(t("account.binanceAuto.disableConfirm"))) return;
    const requestedActorKey = actorKey;
    setBinanceBusy("disable");
    try {
      const next = await disableBinanceAutoSettlement();
      if (actorKeyRef.current !== requestedActorKey) return;
      setBinanceStatus(next);
      setBinanceStatusError("");
      toast.success(t("account.binanceAuto.disabled"));
    } catch (error) {
      if (actorKeyRef.current === requestedActorKey) {
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (actorKeyRef.current === requestedActorKey) setBinanceBusy("");
    }
  };

  const deleteBinanceCredentials = async () => {
    if (binanceBusy || !window.confirm(t("account.binanceAuto.deleteConfirm"))) return;
    const requestedActorKey = actorKey;
    setBinanceBusy("delete");
    try {
      const next = await deleteBinanceAutoSettlement();
      if (actorKeyRef.current !== requestedActorKey) return;
      setBinanceStatus(next);
      setBinanceStatusError("");
      setBinanceApiKey("");
      setBinanceApiSecret("");
      toast.success(t("account.binanceAuto.deleted"));
    } catch (error) {
      if (actorKeyRef.current === requestedActorKey) {
        toast.danger(error instanceof Error ? error.message : String(error));
      }
    } finally {
      if (actorKeyRef.current === requestedActorKey) setBinanceBusy("");
    }
  };

  React.useEffect(() => {
    if (
      !authed
      || binanceBusy
      || binanceConfigurationBlocked
      || binanceApiKey.trim().length < 16
      || binanceApiSecret.trim().length < 16
    ) return;
    const timeoutId = window.setTimeout(() => {
      void discoverBinanceCredentials();
    }, 1_000);
    return () => window.clearTimeout(timeoutId);
  }, [
    authed,
    binanceApiKey,
    binanceApiSecret,
    binanceBusy,
    binanceConfigurationBlocked,
    discoverBinanceCredentials,
  ]);

  if (authLoading || loading) {
    return (
      <div className="flex items-center gap-2 py-6 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("account.loading")}
      </div>
    );
  }
  if (!authed) {
    return (
      <div className="grid justify-items-start gap-3 py-6">
        <p className="text-sm text-muted-foreground">{t("account.signInRequired")}</p>
        <Button variant="primary" onClick={() => window.dispatchEvent(new Event("router-open-login"))}>
          {t("nav.login")}
        </Button>
      </div>
    );
  }

  const qrField = (kind: string, value: string, setValue: (value: string) => void, label: string) => {
    const preview = previews[`${kind}:${value}`];
    return (
      <label className="grid gap-1.5 text-sm">
        <span className="text-muted-foreground">{label}</span>
        <input
          value={value}
          onChange={(event) => setValue(event.target.value)}
          placeholder="https://…"
          className="h-10 rounded-md border bg-white px-3 outline-none focus:ring-2 focus:ring-primary/20"
        />
        {preview ? (
          <AuthenticatedImage
            src={preview}
            alt={t("account.qrPreviewAlt", { method: label })}
            className="mt-1 h-28 w-28 rounded-md border bg-white object-contain p-1"
          />
        ) : null}
      </label>
    );
  };

  const patchDraft = (patch: Partial<PaymentDraft>) => setDraft((current) => ({ ...current, ...patch }));

  return (
    <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-5">
      <section className="grid gap-6 rounded-xl border border-border bg-card p-5 shadow-sm">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <div className="flex items-center gap-2">
              <WalletCards className="h-4 w-4 text-muted-foreground" />
              <h2 className="text-base font-semibold">{t("account.paymentDetails")}</h2>
            </div>
            <p className="mt-1 text-sm text-muted-foreground">{t("account.visibilityHint")}</p>
            <p className="mt-1 text-xs font-medium text-amber-700">{t("market.currencyNotice", {
              rate: formatUsdCnyRate(usdCnyRateMicros),
            })}</p>
          </div>
          <Button variant="primary" isDisabled={!dirty || saving} onClick={() => void save()}>
            {saving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            {t("common.save")}
          </Button>
        </div>

        <div className="grid gap-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div>
              <h3 className="text-sm font-semibold">{t("account.contact.title")}</h3>
              <p className="mt-0.5 text-xs text-muted-foreground">{t("account.contact.hint")}</p>
            </div>
            <Button
              size="sm"
              variant="outline"
              onClick={() =>
                setDraft((current) => ({
                  ...current,
                  contacts: [...current.contacts, emptyContact()],
                }))
              }
            >
              <Plus className="h-4 w-4" />
              {t("account.contact.add")}
            </Button>
          </div>
          {draft.contacts.length ? (
            <div className="grid gap-2">
              {draft.contacts.map((contact, index) => (
                <div
                  key={index}
                  className="grid grid-cols-[minmax(0,9rem)_minmax(0,1fr)_2.25rem] items-end gap-2"
                >
                  <label className="grid gap-1 text-xs text-muted-foreground">
                    {t("account.contact.channel")}
                    <Select
                      selectedKey={contact.channel}
                      aria-label={t("account.contact.channel")}
                      onSelectionChange={(key) => {
                        const channel = String(key || "wechat");
                        const next = (
                          ["wechat", "telegram", "custom"].includes(channel) ? channel : "custom"
                        ) as PaymentContactChannel;
                        setDraft((current) => ({
                          ...current,
                          contacts: current.contacts.map((item, i) =>
                            i === index ? { ...item, channel: next } : item,
                          ),
                        }));
                      }}
                    >
                      <Select.Trigger className="h-10 w-full min-h-10">
                        <Select.Value>
                          {t(
                            CONTACT_CHANNELS.find((item) => item.id === contact.channel)?.labelKey ||
                              "account.contact.channel.custom",
                          )}
                        </Select.Value>
                        <Select.Indicator />
                      </Select.Trigger>
                      <Select.Popover className="min-w-[9rem]">
                        <ListBox aria-label={t("account.contact.channel")}>
                          {CONTACT_CHANNELS.map((channel) => (
                            <ListBox.Item key={channel.id} id={channel.id} textValue={t(channel.labelKey)}>
                              {t(channel.labelKey)}
                            </ListBox.Item>
                          ))}
                        </ListBox>
                      </Select.Popover>
                    </Select>
                  </label>
                  <label className="grid min-w-0 gap-1 text-xs text-muted-foreground">
                    {t("account.contact.handle")}
                    <input
                      value={contact.handle}
                      onChange={(event) =>
                        setDraft((current) => ({
                          ...current,
                          contacts: current.contacts.map((item, i) =>
                            i === index ? { ...item, handle: event.target.value } : item,
                          ),
                        }))
                      }
                      placeholder={t("account.contact.handlePlaceholder")}
                      className="h-10 min-w-0 rounded-md border bg-white px-3 text-sm outline-none focus:ring-2 focus:ring-primary/20"
                    />
                  </label>
                  <Button
                    isIconOnly
                    size="sm"
                    variant="ghost"
                    aria-label={t("account.contact.remove")}
                    className="h-10 w-9 min-w-9"
                    onClick={() =>
                      setDraft((current) => ({
                        ...current,
                        contacts: current.contacts.filter((_, i) => i !== index),
                      }))
                    }
                  >
                    <Trash2 className="h-4 w-4" />
                  </Button>
                </div>
              ))}
            </div>
          ) : (
            <p className="text-sm text-muted-foreground">{t("account.contact.emptyEditor")}</p>
          )}
        </div>

        <div className="grid gap-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <PaymentMethodIcons kinds={["binance"]} />
              <h3 className="text-sm font-semibold">{t("account.binanceAuto.title")}</h3>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button size="sm" variant="outline" onClick={() => setBinanceReceiptsOpen(true)}>
                <ReceiptText className="h-4 w-4" aria-hidden />
                {t("account.binanceAuto.receipts.open")}
              </Button>
              <Button size="sm" variant="outline" onClick={() => setBinanceGuideOpen(true)}>
                <BookOpen className="h-4 w-4" aria-hidden />
                {t("account.binanceAuto.guide.open")}
              </Button>
              {binanceStatus && binanceUiState !== "trial" ? (
                <Chip size="sm" variant="soft" className={binanceUiStateClass}>
                  {t(`account.binanceAuto.state.${binanceUiState}`)}
                </Chip>
              ) : null}
            </div>
          </div>

          {binanceStatusError ? (
            <p className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">
              {t("account.binanceAuto.statusUnavailable")} {binanceStatusError}
            </p>
          ) : null}
          {binanceStatus && binanceUiState === "unavailable" ? (
            <p className="rounded-md border border-slate-200 bg-white px-3 py-2 text-xs leading-5 text-slate-700">
              {t("account.binanceAuto.notice.unavailable")}
            </p>
          ) : null}
          {binanceStatus && binanceUiState === "regionRestricted" ? (
            <p className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">
              {t("account.binanceAuto.notice.regionRestricted")}
            </p>
          ) : null}
          {binanceStatus?.serviceAvailability === "temporarily_unavailable" ? (
            <p className="rounded-md border border-amber-200 bg-white px-3 py-2 text-xs leading-5 text-amber-900">
              {t("account.binanceAuto.notice.temporarilyUnavailable")}
            </p>
          ) : null}
          {binanceStatus && binanceUiState === "trial" ? (
            <p className="rounded-md border border-slate-200 bg-slate-50 px-3 py-2 text-xs leading-5 text-slate-700">
              {t("account.binanceAuto.notice.trial")}
            </p>
          ) : null}
          {binanceStatus && binanceUiState === "activationRequired" ? (
            <div className="flex flex-wrap items-center gap-3 rounded-md border border-sky-200 bg-sky-50 px-3 py-2 text-xs leading-5 text-sky-950">
              <p className="min-w-[14rem] flex-1">{t("account.binanceAuto.notice.activationRequired")}</p>
              <Button
                size="sm"
                variant="primary"
                isDisabled={!!binanceBusy || binanceConfigurationBlocked}
                onClick={() => void enableBinanceCredentials()}
              >
                {binanceBusy === "enable" ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShieldCheck className="h-4 w-4" />}
                {t("account.binanceAuto.enable")}
              </Button>
            </div>
          ) : null}
          {binanceStatus && binanceUiState === "actionRequired" ? (
            <p className="rounded-md border border-amber-200 bg-white px-3 py-2 text-xs leading-5 text-amber-900">
              {t("account.binanceAuto.notice.actionRequired")}
            </p>
          ) : null}
          {binanceStatus && binanceUiState === "degraded" ? (
            <p className="rounded-md border border-rose-200 bg-rose-50 px-3 py-2 text-xs leading-5 text-rose-800">
              {t("account.binanceAuto.notice.degraded")}
            </p>
          ) : null}
          {binanceStatus && binanceUiState === "accountDisabled" ? (
            <p className="rounded-md border border-slate-200 bg-white px-3 py-2 text-xs leading-5 text-slate-700">
              {t("account.binanceAuto.notice.accountDisabled")}
            </p>
          ) : null}
          {binanceStatus?.account ? (
            <div className="grid gap-2 rounded-md border border-border bg-white p-3 text-xs sm:grid-cols-2">
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.boundUid")}</span>
                <strong className="mt-0.5 block break-all">{binanceStatus.account.binanceUid}</strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.maskedKey")}</span>
                <strong className="mt-0.5 block break-all">{binanceStatus.account.maskedApiKey || "—"}</strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.permission")}</span>
                <strong className="mt-0.5 block">
                  {binanceStatus.account.permissionsVerifiedAt
                    ? t("account.binanceAuto.permissionVerified")
                    : t("account.binanceAuto.permissionUnknown")}
                </strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.uidCheck")}</span>
                <strong className="mt-0.5 block">
                  {binanceStatus.account.uidConfirmed
                    ? t("account.binanceAuto.uidConfirmed")
                    : t("account.binanceAuto.uidPending")}
                </strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.lastPoll")}</span>
                <strong className="mt-0.5 block">
                  {binanceStatus.account.lastPollSuccessAt
                    ? new Date(binanceStatus.account.lastPollSuccessAt).toLocaleString()
                    : "—"}
                </strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.region")}</span>
                <strong className="mt-0.5 block">{binanceStatus.account.paymentHomeRegion}</strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.boundAt")}</span>
                <strong className="mt-0.5 block">{new Date(binanceStatus.account.createdAt).toLocaleString()}</strong>
              </div>
              <div>
                <span className="text-muted-foreground">{t("account.binanceAuto.lastVerified")}</span>
                <strong className="mt-0.5 block">
                  {binanceStatus.account.permissionsVerifiedAt
                    ? new Date(binanceStatus.account.permissionsVerifiedAt).toLocaleString()
                    : "—"}
                </strong>
              </div>
              {binanceStatus.account.lastPollErrorCode ? (
                <p className="sm:col-span-2 text-rose-700">
                  {t("account.binanceAuto.lastError", {
                    code: binanceStatus.account.lastPollErrorCode,
                    count: binanceStatus.account.consecutiveFailures,
                  })}
                </p>
              ) : null}
            </div>
          ) : null}

          <div className="grid gap-3 md:grid-cols-2">
            <label className="grid gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("account.binanceAuto.apiKey")}</span>
              <input
                type="text"
                value={binanceApiKey}
                onChange={(event) => setBinanceApiKey(event.target.value)}
                disabled={binanceConfigurationBlocked}
                autoComplete="off"
                spellCheck={false}
                className="h-10 rounded-md border bg-white px-3 font-mono text-xs outline-none focus:ring-2 focus:ring-primary/20 disabled:cursor-not-allowed disabled:bg-slate-100 disabled:text-slate-500"
              />
            </label>
            <label className="grid gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("account.binanceAuto.apiSecret")}</span>
              <input
                type="text"
                value={binanceApiSecret}
                onChange={(event) => setBinanceApiSecret(event.target.value)}
                disabled={binanceConfigurationBlocked}
                autoComplete="off"
                spellCheck={false}
                className="h-10 rounded-md border bg-white px-3 font-mono text-xs outline-none focus:ring-2 focus:ring-primary/20 disabled:cursor-not-allowed disabled:bg-slate-100 disabled:text-slate-500"
              />
            </label>
          </div>
          {binanceBusy === "discover" ? (
            <p className="flex items-center gap-2 text-xs text-muted-foreground">
              <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
              {t("account.binanceAuto.loadingAccount")}
            </p>
          ) : null}
          <p className="text-xs leading-5 text-muted-foreground">{t("account.binanceAuto.readOnlyWarning")}</p>
          {binanceDiscovery ? (
            <div className="grid gap-3 rounded-md border border-emerald-200 bg-emerald-50/60 p-3 text-xs">
              <div className="grid gap-2 sm:grid-cols-2">
                <div>
                  <span className="text-muted-foreground">{t("account.binanceAuto.preview.uid")}</span>
                  <strong className="mt-0.5 block break-all text-sm">{binanceDiscovery.account.binanceUid}</strong>
                </div>
                <div>
                  <span className="text-muted-foreground">{t("account.binanceAuto.maskedKey")}</span>
                  <strong className="mt-0.5 block break-all">{binanceDiscovery.account.maskedApiKey}</strong>
                </div>
                <div>
                  <span className="text-muted-foreground">{t("account.binanceAuto.permission")}</span>
                  <strong className="mt-0.5 block">{t("account.binanceAuto.permissionVerified")}</strong>
                </div>
                <div>
                  <span className="text-muted-foreground">{t("account.binanceAuto.preview.source")}</span>
                  <strong className="mt-0.5 block">
                    {t(binanceDiscovery.account.uidConfirmationSource === "receiver_history"
                      ? "account.binanceAuto.source.receiver_history"
                      : binanceDiscovery.account.uidConfirmationSource === "payer_history"
                        ? "account.binanceAuto.source.payer_history"
                        : "account.binanceAuto.source.unknown")}
                    {` · ${t("account.binanceAuto.preview.evidence", { count: binanceDiscovery.account.evidenceCount })}`}
                  </strong>
                </div>
              </div>
              {binanceDiscovery.account.previousBinanceUid ? (
                <p className="rounded-md border border-amber-200 bg-white px-3 py-2 text-amber-900">
                  {t("account.binanceAuto.preview.replace", {
                    previous: binanceDiscovery.account.previousBinanceUid,
                    next: binanceDiscovery.account.binanceUid,
                  })}
                </p>
              ) : null}
              <p className="text-muted-foreground">
                {t("account.binanceAuto.preview.expires", {
                  date: new Date(binanceDiscovery.expiresAt).toLocaleString(),
                })}
              </p>
              <div className="flex flex-wrap gap-2">
                <Button size="sm" variant="primary" isDisabled={!!binanceBusy || binanceConfigurationBlocked} onClick={() => void confirmBinanceCredentials()}>
                  {binanceBusy === "confirm" ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShieldCheck className="h-4 w-4" />}
                  {t("account.binanceAuto.confirm")}
                </Button>
                <Button
                  size="sm"
                  variant="outline"
                  isDisabled={!!binanceBusy}
                  onClick={() => {
                    binanceDiscoveryAttemptRef.current = "";
                    setBinanceDiscovery(null);
                  }}
                >
                  {t("common.cancel")}
                </Button>
              </div>
            </div>
          ) : null}
          <div className="flex flex-wrap gap-2">
            {binanceStatus?.account?.maskedApiKey ? (
              <>
                <Button
                  size="sm"
                  variant="outline"
                  isDisabled={
                    !!binanceBusy
                    || binanceConfigurationBlocked
                    || binanceStatus.account.status === "disabled"
                  }
                  onClick={() => void verifyBinanceCredentials()}
                >
                  {binanceBusy === "verify" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
                  {t("account.binanceAuto.verify")}
                </Button>
                <Button size="sm" variant="outline" isDisabled={!!binanceBusy || binanceStatus.account.status === "disabled"} onClick={() => void disableBinanceCredentials()}>
                  {binanceBusy === "disable" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Power className="h-4 w-4" />}
                  {t("account.binanceAuto.disable")}
                </Button>
                <Button size="sm" variant="outline" className="text-rose-700" isDisabled={!!binanceBusy} onClick={() => void deleteBinanceCredentials()}>
                  {binanceBusy === "delete" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Trash2 className="h-4 w-4" />}
                  {t("account.binanceAuto.delete")}
                </Button>
              </>
            ) : null}
          </div>
        </div>

        <BinanceApiKeyGuideDialog open={binanceGuideOpen} onOpenChange={setBinanceGuideOpen} />
        <BinanceReceiptHistoryDialog
          open={binanceReceiptsOpen}
          onOpenChange={setBinanceReceiptsOpen}
          actorKey={actorKey}
        />

        <div className="grid gap-4">
          <div className="flex items-center gap-2">
            <PaymentMethodIcons kinds={["alipay"]} />
            <h3 className="text-sm font-semibold">{t("billing.payment.alipay")}</h3>
          </div>
          <div className="grid gap-4 md:grid-cols-2">
            <label className="grid gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("account.phoneOrAccount")}</span>
              <input
                value={draft.alipayAccount}
                onChange={(event) => patchDraft({ alipayAccount: event.target.value })}
                className="h-10 rounded-md border bg-white px-3 outline-none focus:ring-2 focus:ring-primary/20"
              />
            </label>
            {qrField("alipay", draft.alipayQr, (value) => patchDraft({ alipayQr: value }), t("account.qrImageUrl"))}
          </div>
        </div>

        <div className="grid gap-4">
          <div className="flex items-center gap-2">
            <PaymentMethodIcons kinds={["wechat"]} />
            <h3 className="text-sm font-semibold">{t("billing.payment.wechat")}</h3>
          </div>
          <div className="max-w-xl">
            {qrField("wechat", draft.wechatQr, (value) => patchDraft({ wechatQr: value }), t("account.qrImageUrl"))}
          </div>
        </div>

        <div className="grid gap-4">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex items-center gap-2">
              <PaymentMethodIcons kinds={["crypto"]} />
              <h3 className="text-sm font-semibold">USDT / USDC</h3>
            </div>
            <Button
              size="sm"
              variant="outline"
              onClick={() => setDraft((current) => ({ ...current, crypto: [...current.crypto, emptyCrypto()] }))}
            >
              <Plus className="h-4 w-4" />
              {t("account.addAddress")}
            </Button>
          </div>
          <div className="grid gap-3">
            {draft.crypto.map((method, index) => (
              <div
                key={index}
                className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_2.25rem] items-end gap-2 sm:grid-cols-[7.5rem_8.5rem_minmax(0,1fr)_2.25rem]"
              >
                <label className="grid gap-1 text-xs text-muted-foreground">
                  {t("account.token")}
                  <Select
                    selectedKey={method.token}
                    aria-label={t("account.token")}
                    onSelectionChange={(key) => {
                      const token = String(key || "USDT") === "USDC" ? "USDC" : "USDT";
                      setDraft((current) => ({
                        ...current,
                        crypto: current.crypto.map((item, i) => (i === index ? { ...item, token } : item)),
                      }));
                    }}
                  >
                    <Select.Trigger className="h-10 w-full min-h-10">
                      <Select.Value>{method.token}</Select.Value>
                      <Select.Indicator />
                    </Select.Trigger>
                    <Select.Popover className="min-w-[7.5rem]">
                      <ListBox aria-label={t("account.token")}>
                        {CRYPTO_TOKENS.map((token) => (
                          <ListBox.Item key={token} id={token} textValue={token}>
                            {token}
                          </ListBox.Item>
                        ))}
                      </ListBox>
                    </Select.Popover>
                  </Select>
                </label>
                <label className="grid gap-1 text-xs text-muted-foreground">
                  {t("account.chain")}
                  <Select
                    selectedKey={method.chain}
                    aria-label={t("account.chain")}
                    onSelectionChange={(key) => {
                      const chain = String(key || "bsc");
                      const nextChain = (["bsc", "base", "eth", "tron"].includes(chain)
                        ? chain
                        : "bsc") as CryptoDraft["chain"];
                      setDraft((current) => ({
                        ...current,
                        crypto: current.crypto.map((item, i) => (i === index ? { ...item, chain: nextChain } : item)),
                      }));
                    }}
                  >
                    <Select.Trigger className="h-10 w-full min-h-10">
                      <Select.Value>{chainLabel(method.chain)}</Select.Value>
                      <Select.Indicator />
                    </Select.Trigger>
                    <Select.Popover className="min-w-[8.5rem]">
                      <ListBox aria-label={t("account.chain")}>
                        {CRYPTO_CHAINS.map((chain) => (
                          <ListBox.Item key={chain.id} id={chain.id} textValue={chain.label}>
                            {chain.label}
                          </ListBox.Item>
                        ))}
                      </ListBox>
                    </Select.Popover>
                  </Select>
                </label>
                <label className="order-4 col-span-3 grid min-w-0 gap-1 text-xs text-muted-foreground sm:order-none sm:col-span-1">
                  {t("account.address")}
                  <input
                    value={method.address}
                    onChange={(event) =>
                      setDraft((current) => ({
                        ...current,
                        crypto: current.crypto.map((item, i) =>
                          i === index ? { ...item, address: event.target.value } : item,
                        ),
                      }))
                    }
                    className="h-10 min-w-0 rounded-md border bg-white px-3 font-mono text-sm text-foreground"
                  />
                </label>
                <Button
                  isIconOnly
                  size="sm"
                  variant="ghost"
                  aria-label={t("account.removeAddress")}
                  className="order-3 h-10 w-9 min-w-9 sm:order-none"
                  onClick={() =>
                    setDraft((current) => ({
                      ...current,
                      crypto: current.crypto.length === 1 ? [emptyCrypto()] : current.crypto.filter((_, i) => i !== index),
                    }))
                  }
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
            ))}
          </div>
        </div>

        <div className="grid gap-3">
          <div className="flex items-center gap-2">
            <PaymentMethodIcons kinds={["custom"]} />
            <h3 className="text-sm font-semibold">{t("account.customInstructions")}</h3>
            <Chip size="sm" variant="soft">
              {t("account.plainText")}
            </Chip>
          </div>
          <textarea
            value={draft.custom}
            onChange={(event) => patchDraft({ custom: event.target.value })}
            maxLength={2000}
            rows={5}
            className="resize-y rounded-md border bg-white px-3 py-2 text-sm leading-6 outline-none focus:ring-2 focus:ring-primary/20"
          />
        </div>
      </section>
    </div>
  );
}
