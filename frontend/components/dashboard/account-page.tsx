"use client";

import * as React from "react";
import { Button, Chip, toast } from "@heroui/react";
import { Loader2, Plus, Save, Trash2, WalletCards } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { AuthenticatedImage } from "@/components/common/authenticated-image";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAccountPaymentProfile, getClientMarketProviderBlocks, liftClientMarketProviderBlock, updateAccountPaymentProfile } from "@/lib/api";
import type { ClientMarketPaymentMethod, ClientMarketProviderBlock } from "@/lib/types";

type CryptoDraft = { token: "USDT" | "USDC"; chain: "bsc" | "base" | "eth" | "tron"; address: string };

const emptyCrypto = (): CryptoDraft => ({ token: "USDT", chain: "bsc", address: "" });

function blockReasonLabel(reason: string, t: ReturnType<typeof useLocaleText>["t"]) {
  if (reason === "payment_not_received") return t("account.blockReason.paymentNotReceived");
  return reason.replaceAll("_", " ");
}

export function AccountPage() {
  const { locale, t } = useLocaleText();
  const { session, loading: authLoading } = useAuth();
  const authed = !!session?.authenticated;
  const [loading, setLoading] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [alipayAccount, setAlipayAccount] = React.useState("");
  const [alipayQr, setAlipayQr] = React.useState("");
  const [wechatQr, setWechatQr] = React.useState("");
  const [binanceAccount, setBinanceAccount] = React.useState("");
  const [binanceQr, setBinanceQr] = React.useState("");
  const [crypto, setCrypto] = React.useState<CryptoDraft[]>([emptyCrypto()]);
  const [custom, setCustom] = React.useState("");
  const [previews, setPreviews] = React.useState<Record<string, string>>({});
  const [blocks, setBlocks] = React.useState<ClientMarketProviderBlock[]>([]);
  const [liftingBlock, setLiftingBlock] = React.useState("");

  React.useEffect(() => {
    if (!authed) return;
    setLoading(true);
    Promise.all([getAccountPaymentProfile(), getClientMarketProviderBlocks()])
      .then(([profile, providerBlocks]) => {
        const alipay = profile.methods.find((method) => method.kind === "alipay");
        const wechat = profile.methods.find((method) => method.kind === "wechat");
        const binance = profile.methods.find((method) => method.kind === "binance");
        const customMethod = profile.methods.find((method) => method.kind === "custom");
        setAlipayAccount(alipay?.account || "");
        setAlipayQr(alipay?.qrImageUrl || "");
        setWechatQr(wechat?.qrImageUrl || "");
        setBinanceAccount(binance?.account || "");
        setBinanceQr(binance?.qrImageUrl || "");
        const cryptoMethods = profile.methods
          .filter((method) => method.kind === "crypto")
          .map((method) => ({
            token: (method.token === "USDC" ? "USDC" : "USDT") as CryptoDraft["token"],
            chain: (["bsc", "base", "eth", "tron"].includes(method.chain || "")
              ? method.chain
              : "bsc") as CryptoDraft["chain"],
            address: method.address || "",
          }));
        setCrypto(cryptoMethods.length ? cryptoMethods : [emptyCrypto()]);
        setCustom(customMethod?.instructions || "");
        setPreviews(
          Object.fromEntries(
            profile.methods
              .filter((method) => method.assetUrl)
              .map((method) => [`${method.kind}:${method.qrImageUrl || ""}`, method.assetUrl!]),
          ),
        );
        setBlocks(providerBlocks);
      })
      .catch((error) => toast.danger(error instanceof Error ? error.message : String(error)))
      .finally(() => setLoading(false));
  }, [authed]);

  const save = async () => {
    const methods: ClientMarketPaymentMethod[] = [];
    if (alipayAccount.trim() || alipayQr.trim()) {
      methods.push({ kind: "alipay", account: alipayAccount.trim() || undefined, qrImageUrl: alipayQr.trim() || undefined });
    }
    if (wechatQr.trim()) methods.push({ kind: "wechat", qrImageUrl: wechatQr.trim() });
    if (binanceAccount.trim() || binanceQr.trim()) {
      methods.push({ kind: "binance", account: binanceAccount.trim() || undefined, qrImageUrl: binanceQr.trim() || undefined });
    }
    for (const method of crypto) {
      if (method.address.trim()) methods.push({ kind: "crypto", token: method.token, chain: method.chain, address: method.address.trim() });
    }
    if (custom.trim()) methods.push({ kind: "custom", instructions: custom.trim() });
    setSaving(true);
    try {
      const profile = await updateAccountPaymentProfile(methods);
      setPreviews(
        Object.fromEntries(
          profile.methods
            .filter((method) => method.assetUrl)
            .map((method) => [`${method.kind}:${method.qrImageUrl || ""}`, method.assetUrl!]),
        ),
      );
      toast.success(t("account.saved"));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setSaving(false);
    }
  };

  const unblock = async (block: ClientMarketProviderBlock) => {
    setLiftingBlock(block.clientUserId);
    try {
      await liftClientMarketProviderBlock(block.clientUserId);
      setBlocks((current) => current.filter((item) => item.clientUserId !== block.clientUserId));
      toast.success(t("account.unblockedToast", { email: block.clientOwnerEmail }));
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setLiftingBlock("");
    }
  };

  if (authLoading || loading) {
    return <div className="mx-auto flex w-[calc(100%-2rem)] max-w-5xl items-center gap-2 py-12 text-sm text-muted-foreground"><Loader2 className="h-4 w-4 animate-spin" />{t("account.loading")}</div>;
  }
  if (!authed) {
    return (
      <div className="mx-auto grid w-[calc(100%-2rem)] max-w-5xl justify-items-start gap-3 py-12">
        <h1 className="text-xl font-semibold">{t("account.title")}</h1>
        <p className="text-sm text-muted-foreground">{t("account.signInRequired")}</p>
        <Button variant="primary" onClick={() => window.dispatchEvent(new Event("router-open-login"))}>{t("nav.login")}</Button>
      </div>
    );
  }

  const qrField = (kind: string, value: string, setValue: (value: string) => void, label: string) => {
    const preview = previews[`${kind}:${value}`];
    return (
      <label className="grid gap-1.5 text-sm">
        <span className="text-muted-foreground">{label}</span>
        <input value={value} onChange={(event) => setValue(event.target.value)} placeholder="https://…" className="h-10 rounded-md border bg-white px-3 outline-none focus:ring-2 focus:ring-primary/20" />
        {preview ? <AuthenticatedImage src={preview} alt={t("account.qrPreviewAlt", { method: label })} className="mt-1 h-28 w-28 rounded-md border bg-white object-contain p-1" /> : null}
      </label>
    );
  };

  return (
    <main className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-5xl grid-cols-[minmax(0,1fr)] gap-8 pb-12">
      <header className="flex flex-wrap items-start justify-between gap-4 border-b pb-5">
        <div>
          <div className="flex items-center gap-2"><WalletCards className="h-5 w-5 text-primary" /><h1 className="text-xl font-semibold">{t("account.paymentDetails")}</h1></div>
          <p className="mt-1 text-sm text-muted-foreground">{t("account.visibilityHint")}</p>
        </div>
        <Button variant="primary" isDisabled={saving} onClick={() => void save()}>{saving ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}{t("common.save")}</Button>
      </header>

      <section className="grid gap-4 border-b pb-7">
        <div className="flex items-center gap-2"><PaymentMethodIcons kinds={["alipay"]} /><h2 className="text-sm font-semibold">{t("billing.payment.alipay")}</h2></div>
        <div className="grid gap-4 md:grid-cols-2">
          <label className="grid gap-1.5 text-sm"><span className="text-muted-foreground">{t("account.phoneOrAccount")}</span><input value={alipayAccount} onChange={(event) => setAlipayAccount(event.target.value)} className="h-10 rounded-md border bg-white px-3 outline-none focus:ring-2 focus:ring-primary/20" /></label>
          {qrField("alipay", alipayQr, setAlipayQr, t("account.qrImageUrl"))}
        </div>
      </section>

      <section className="grid gap-4 border-b pb-7">
        <div className="flex items-center gap-2"><PaymentMethodIcons kinds={["wechat"]} /><h2 className="text-sm font-semibold">{t("billing.payment.wechat")}</h2></div>
        <div className="max-w-xl">{qrField("wechat", wechatQr, setWechatQr, t("account.qrImageUrl"))}</div>
      </section>

      <section className="grid gap-4 border-b pb-7">
        <div className="flex items-center gap-2"><PaymentMethodIcons kinds={["binance"]} /><h2 className="text-sm font-semibold">{t("billing.payment.binance")}</h2></div>
        <div className="grid gap-4 md:grid-cols-2">
          <label className="grid gap-1.5 text-sm"><span className="text-muted-foreground">{t("account.binanceUserId")}</span><input value={binanceAccount} onChange={(event) => setBinanceAccount(event.target.value)} className="h-10 rounded-md border bg-white px-3 outline-none focus:ring-2 focus:ring-primary/20" /></label>
          {qrField("binance", binanceQr, setBinanceQr, t("account.qrImageUrl"))}
        </div>
      </section>

      <section className="grid gap-4 border-b pb-7">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="flex items-center gap-2"><PaymentMethodIcons kinds={["crypto"]} /><h2 className="text-sm font-semibold">USDT / USDC</h2></div>
          <Button size="sm" variant="outline" onClick={() => setCrypto((current) => [...current, emptyCrypto()])}><Plus className="h-4 w-4" />{t("account.addAddress")}</Button>
        </div>
        <div className="grid gap-3">
          {crypto.map((method, index) => (
            <div key={index} className="grid grid-cols-[minmax(0,1fr)_minmax(0,1fr)_2.25rem] items-end gap-2 sm:grid-cols-[7rem_8rem_minmax(0,1fr)_2.25rem]">
              <label className="grid gap-1 text-xs text-muted-foreground">{t("account.token")}<select value={method.token} onChange={(event) => setCrypto((current) => current.map((item, i) => i === index ? { ...item, token: event.target.value as CryptoDraft["token"] } : item))} className="h-10 rounded-md border bg-white px-2 text-sm text-foreground"><option>USDT</option><option>USDC</option></select></label>
              <label className="grid gap-1 text-xs text-muted-foreground">{t("account.chain")}<select value={method.chain} onChange={(event) => setCrypto((current) => current.map((item, i) => i === index ? { ...item, chain: event.target.value as CryptoDraft["chain"] } : item))} className="h-10 rounded-md border bg-white px-2 text-sm text-foreground"><option value="bsc">BSC</option><option value="base">Base</option><option value="eth">Ethereum</option><option value="tron">TRON</option></select></label>
              <label className="order-4 col-span-3 grid min-w-0 gap-1 text-xs text-muted-foreground sm:order-none sm:col-span-1">{t("account.address")}<input value={method.address} onChange={(event) => setCrypto((current) => current.map((item, i) => i === index ? { ...item, address: event.target.value } : item))} className="h-10 min-w-0 rounded-md border bg-white px-3 font-mono text-sm text-foreground" /></label>
              <Button isIconOnly size="sm" variant="ghost" aria-label={t("account.removeAddress")} className="order-3 h-10 w-9 min-w-9 sm:order-none" onClick={() => setCrypto((current) => current.length === 1 ? [emptyCrypto()] : current.filter((_, i) => i !== index))}><Trash2 className="h-4 w-4" /></Button>
            </div>
          ))}
        </div>
      </section>

      <section className="grid gap-3">
        <div className="flex items-center gap-2"><PaymentMethodIcons kinds={["custom"]} /><h2 className="text-sm font-semibold">{t("account.customInstructions")}</h2><Chip size="sm" variant="soft">{t("account.plainText")}</Chip></div>
        <textarea value={custom} onChange={(event) => setCustom(event.target.value)} maxLength={2000} rows={5} className="resize-y rounded-md border bg-white px-3 py-2 text-sm leading-6 outline-none focus:ring-2 focus:ring-primary/20" />
      </section>

      <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-3 border-t pt-7">
        <div><h2 className="text-sm font-semibold">{t("account.blockedOwners")}</h2><p className="mt-1 text-xs text-muted-foreground">{t("account.blockedHint")}</p></div>
        {blocks.length ? (
          <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2">
            {blocks.map((block) => (
              <div key={block.clientUserId} className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border bg-white px-3 py-2 text-sm">
                <div className="min-w-0 max-w-full"><div className="truncate font-medium">{block.clientOwnerEmail}</div><div className="break-words text-xs text-muted-foreground">{blockReasonLabel(block.reason, t)} · {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(block.createdAt))}</div></div>
                <Button size="sm" variant="outline" isDisabled={!!liftingBlock} onClick={() => void unblock(block)}>{liftingBlock === block.clientUserId ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("account.unblock")}</Button>
              </div>
            ))}
          </div>
        ) : <p className="text-sm text-muted-foreground">{t("account.noneBlocked")}</p>}
      </section>
    </main>
  );
}
