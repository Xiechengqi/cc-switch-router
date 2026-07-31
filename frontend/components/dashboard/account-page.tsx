"use client";

import * as React from "react";
import { Button, Chip, ListBox, Select, toast } from "@heroui/react";
import { ExternalLink, Loader2, Plus, Save, Trash2, WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { AuthenticatedImage } from "@/components/common/authenticated-image";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountPaymentProfile, updateAccountPaymentProfile } from "@/lib/api";
import type { ClientMarketPaymentMethod, PaymentContact, PaymentContactChannel } from "@/lib/types";

type CryptoDraft = { token: "USDT" | "USDC"; chain: "bsc" | "base" | "eth" | "tron"; address: string };
type ContactDraft = { channel: PaymentContactChannel; handle: string };
type PaymentDraft = {
  alipayAccount: string;
  alipayQr: string;
  wechatQr: string;
  binanceAccount: string;
  binanceQr: string;
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
  binanceAccount: "",
  binanceQr: "",
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
    binanceAccount: draft.binanceAccount.trim(),
    binanceQr: draft.binanceQr.trim(),
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
  const [loading, setLoading] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [draft, setDraft] = React.useState<PaymentDraft>(emptyPaymentDraft);
  const [baseline, setBaseline] = React.useState(() => serializePaymentDraft(emptyPaymentDraft()));
  const [previews, setPreviews] = React.useState<Record<string, string>>({});

  const dirty = serializePaymentDraft(draft) !== baseline;

  const applyProfile = React.useCallback((methods: ClientMarketPaymentMethod[], contacts: PaymentContact[] = []) => {
    const alipay = methods.find((method) => method.kind === "alipay");
    const wechat = methods.find((method) => method.kind === "wechat");
    const binance = methods.find((method) => method.kind === "binance");
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
      binanceAccount: binance?.account || "",
      binanceQr: binance?.qrImageUrl || "",
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
    if (!authed) return;
    setLoading(true);
    getAccountPaymentProfile()
      .then((profile) => applyProfile(profile.methods, profile.contacts || []))
      .catch((error) => toast.danger(error instanceof Error ? error.message : String(error)))
      .finally(() => setLoading(false));
  }, [applyProfile, authed]);

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
    if (draft.binanceAccount.trim() || draft.binanceQr.trim()) {
      methods.push({
        kind: "binance",
        account: draft.binanceAccount.trim() || undefined,
        qrImageUrl: draft.binanceQr.trim() || undefined,
      });
    }
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
          <div className="grid gap-1">
            <div className="flex items-center gap-2">
              <PaymentMethodIcons kinds={["binance"]} />
              <h3 className="text-sm font-semibold">{t("billing.payment.binance")}</h3>
            </div>
            <p className="text-xs text-muted-foreground">
              {t("account.binanceRecommend")}
              <a
                href="https://www.bsmkweb.cc/register?ref=310371521"
                target="_blank"
                rel="noopener noreferrer"
                className="inline-flex items-center gap-0.5 font-medium text-primary hover:underline"
              >
                {t("account.binanceRegister")}
                <ExternalLink className="h-3 w-3 shrink-0" aria-hidden />
              </a>
            </p>
          </div>
          <div className="grid gap-4 md:grid-cols-2">
            <label className="grid gap-1.5 text-sm">
              <span className="text-muted-foreground">{t("account.binanceUserId")}</span>
              <input
                value={draft.binanceAccount}
                onChange={(event) => patchDraft({ binanceAccount: event.target.value })}
                className="h-10 rounded-md border bg-white px-3 outline-none focus:ring-2 focus:ring-primary/20"
              />
            </label>
            {qrField("binance", draft.binanceQr, (value) => patchDraft({ binanceQr: value }), t("account.qrImageUrl"))}
          </div>
        </div>

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
