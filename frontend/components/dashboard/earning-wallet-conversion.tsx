"use client";

/* Hallmark · component: earnings-to-wallet confirmation · genre: modern-minimal · theme: existing Router tokens
 * audience: Provider account holders · use: deliberate irreversible conversion · tone: utilitarian
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: pass (40–41), computed-style browser checks · mobile: 320 / 375 / 414 / 768
 * pre-emit critique: P5 H5 E4 S5 R5 V4 · component-scope: no macrostructure or global styling
 */
import * as React from "react";
import Link from "next/link";
import { ArrowRightLeft, CheckCircle2, CircleAlert, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { ApiError } from "@/lib/api";
import type { MessageKey } from "@/lib/i18n";
import { getMarketProviderFinance, type MarketEarningConversionCapacity } from "@/lib/market-wallet-api";
import { formatWalletUnits, parseNonNegativeUsdMinor, safeMarketReturnPath } from "@/lib/market-wallet-money";
import {
  confirmEarningConversion, earningConversionDefinitelyRejected, earningConversionResultMatches, EARNING_CONVERSION_PENDING_PREFIX,
  getEarningConversions, notifyMarketBalanceChanged, quoteEarningConversion,
  readPendingEarningConversion, subscribeMarketBalanceChanged, writePendingEarningConversion,
  type EarningConversionQuote, type EarningWalletConversion as Conversion, type PendingEarningConversion,
} from "@/lib/earning-wallet-conversion";

export function ConversionAction({ children, phase = "default", previewState, ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  phase?: "default" | "loading" | "error" | "success"; previewState?: "hover" | "focus" | "active";
}) {
  return <button {...props} disabled={props.disabled || phase === "loading"} data-state={phase} data-preview={previewState}
    className="inline-flex min-h-11 min-w-0 items-center justify-center gap-2 whitespace-nowrap rounded-md bg-primary px-4 py-2 text-sm font-semibold text-primary-foreground hover:bg-primary/90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-50 data-[preview=hover]:bg-primary/90 data-[preview=focus]:outline-2 data-[preview=focus]:outline-offset-2 data-[preview=focus]:outline-ring data-[preview=active]:translate-y-px data-[state=error]:outline-2 data-[state=error]:outline-destructive motion-reduce:transform-none">
    {phase === "loading" ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" />
      : phase === "success" ? <CheckCircle2 className="size-4" aria-hidden="true" />
        : phase === "error" ? <CircleAlert className="size-4" aria-hidden="true" /> : <ArrowRightLeft className="size-4" aria-hidden="true" />}
    {children}
  </button>;
}

type Props = {
  capacity?: MarketEarningConversionCapacity;
  onConverted?: () => Promise<unknown> | void;
  disabled?: boolean;
  showEmpty?: boolean;
};
export function EarningWalletConversion(props: Props) {
  const { session } = useAuth();
  const actor = session?.user?.id;
  return session?.authenticated && actor ? <ConversionControl key={actor} {...props} actor={actor} /> : null;
}

function ConversionControl({ capacity: supplied, onConverted, disabled, showEmpty, actor }: Props & { actor: string }) {
  const { t, locale } = useLocaleText();
  const [capacity, setCapacity] = React.useState(supplied);
  const [open, setOpen] = React.useState(false);
  const [all, setAll] = React.useState(true);
  const [amount, setAmount] = React.useState("");
  const [touched, setTouched] = React.useState(false);
  const [quote, setQuote] = React.useState<EarningConversionQuote | null>(null);
  const [pending, setPending] = React.useState<PendingEarningConversion | null>(null);
  const [hasPending, setHasPending] = React.useState(false);
  const [acknowledged, setAcknowledged] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState<Conversion | null>(null);
  const [refreshFailed, setRefreshFailed] = React.useState(false);
  const [batchOffered, setBatchOffered] = React.useState(false);
  const [now, setNow] = React.useState(Date.now());
  const [returnPath, setReturnPath] = React.useState<string | null>(null);
  const dialog = React.useRef<HTMLDialogElement>(null);
  const busyRef = React.useRef(false);
  const mounted = React.useRef(true);
  const titleId = React.useId();
  const helperId = React.useId();

  React.useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  React.useEffect(() => { if (supplied) setCapacity(supplied); }, [supplied]);
  const refresh = React.useCallback(async () => {
    const value = await getMarketProviderFinance();
    if (mounted.current) setCapacity(value);
  }, []);
  React.useEffect(() => {
    if (!supplied) void refresh().catch(() => {});
    try { setHasPending(!!readPendingEarningConversion(sessionStorage, actor)); } catch { setHasPending(true); }
    return subscribeMarketBalanceChanged(actor, () => { void refresh().catch(() => {}); });
  }, [actor, supplied, refresh]);
  React.useEffect(() => {
    if (open && dialog.current && !dialog.current.open) dialog.current.showModal();
    if (!open && dialog.current?.open) dialog.current.close();
    if (!open) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [open]);

  const available = capacity?.convertibleUnits ?? "0";
  const hasAmount = /^[1-9]\d*$/.test(available);
  const minor = parseNonNegativeUsdMinor(amount);
  const amountValid = all ? hasAmount : minor != null && minor > 0 && BigInt(minor) * 86_400n <= BigInt(available);
  const expired = !!quote && Date.parse(quote.expiresAt) <= now;
  const blocker = capacity?.conversionBlockers?.[0];
  const blockerKey: MessageKey = blocker === "identity" ? "earningConversion.identity"
    : blocker === "finance_hold" ? "earningConversion.hold" : "earningConversion.unavailable";
  const close = () => { if (!busyRef.current) setOpen(false); };
  const begin = () => {
    setSuccess(null); setError(""); setRefreshFailed(false); setBatchOffered(false); setAcknowledged(false); setNow(Date.now());
    try {
      const previous = readPendingEarningConversion(sessionStorage, actor);
      setPending(previous); setQuote(previous?.quote ?? null); setHasPending(!!previous);
      if (previous) setError(t("earningConversion.uncertain"));
    } catch { setError(t("earningConversion.storage")); setHasPending(true); }
    const path = window.location.pathname.startsWith("/account/wallet")
      ? new URLSearchParams(window.location.search).get("returnTo") : null;
    setReturnPath(safeMarketReturnPath(path));
    setOpen(true);
    void refresh().catch(() => setError(t("earningConversion.changed")));
  };
  const failure = (cause: unknown) => {
    const code = cause instanceof ApiError ? cause.code : null;
    return t(code === "EARNING_CONVERSION_QUOTE_EXPIRED" ? "earningConversion.expired"
      : code === "EARNING_CONVERSION_BATCH_LIMIT" ? "earningConversion.batch"
        : code === "MARKET_FINANCE_HOLD" ? "earningConversion.hold"
          : cause instanceof ApiError && cause.status === 429 ? "earningConversion.limited" : "earningConversion.changed");
  };
  const run = async (action: () => Promise<void>) => {
    if (busyRef.current) return;
    busyRef.current = true; setBusy(true); setError("");
    try { await action(); } finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };
  const review = (batch = false) => void run(async () => {
    if ((!batch && !amountValid) || hasPending) return;
    setBatchOffered(false);
    try {
      const value = await quoteEarningConversion(batch || all ? null : minor, !batch && all, batch);
      if (mounted.current) { setQuote(value); setAcknowledged(false); setNow(Date.now()); }
    } catch (cause) { if (mounted.current) {
      setError(failure(cause));
      setBatchOffered(cause instanceof ApiError && cause.code === "EARNING_CONVERSION_BATCH_LIMIT");
    } }
  });
  const forgetPending = (value: PendingEarningConversion) => {
    try {
      if (readPendingEarningConversion(sessionStorage, actor)?.key === value.key) sessionStorage.removeItem(EARNING_CONVERSION_PENDING_PREFIX + actor);
    } catch { /* An already committed conversion is still a success. */ }
    setPending(null); setHasPending(false);
  };
  const complete = async (value: Conversion, attempt: PendingEarningConversion) => {
    if (!earningConversionResultMatches(value, attempt)) throw new Error("Conversion response mismatch");
    if (!mounted.current) return;
    forgetPending(attempt); setSuccess(value); setError("");
    notifyMarketBalanceChanged(actor);
    try { await refresh(); await onConverted?.(); } catch { if (mounted.current) setRefreshFailed(true); }
  };
  const check = () => void run(async () => {
    if (!pending) return;
    try {
      const page = await getEarningConversions(undefined, pending.quote.id);
      if (page.items[0]) await complete(page.items[0], pending);
      else setError(t("earningConversion.notFound"));
    } catch { setError(t("earningConversion.uncertain")); }
  });
  const confirm = () => void run(async () => {
    if (!quote || !acknowledged || (!pending && expired)) return;
    const attempt = pending ?? { actor, key: crypto.randomUUID(), quote };
    try { writePendingEarningConversion(sessionStorage, attempt); }
    catch { setError(t("earningConversion.storage")); return; }
    setPending(attempt); setHasPending(true);
    try { await complete(await confirmEarningConversion(attempt), attempt); }
    catch (cause) {
      if (!mounted.current) return;
      if (earningConversionDefinitelyRejected(cause)) {
        forgetPending(attempt); setQuote(null); setAcknowledged(false); setError(failure(cause));
        void refresh().catch(() => {});
      } else setError(t("earningConversion.uncertain"));
    }
  });

  if (!hasAmount && !hasPending && !showEmpty) return null;
  return <div className="grid min-w-0 gap-2" data-testid="earning-conversion">
    <div className="flex min-w-0 flex-wrap items-center justify-between gap-3 text-sm">
      <span className="min-w-0 break-words tabular-nums">{t("earningConversion.available", { amount: formatWalletUnits(available, "$", locale) })}</span>
      <ConversionAction type="button" disabled={disabled || (!hasPending && (!hasAmount || !capacity?.canConvert))} onClick={begin}>
        {t(hasPending ? "earningConversion.check" : "earningConversion.action")}
      </ConversionAction>
    </div>
    {showEmpty ? <p className="text-sm text-muted-foreground">{!capacity?.canConvert && blocker ? t(blockerKey) : !hasAmount ? t("earningConversion.none") : t("earningConversion.help")}</p> : null}
    <dialog ref={dialog} aria-labelledby={titleId} onCancel={(event) => { event.preventDefault(); close(); }} onClose={() => { if (!busyRef.current) setOpen(false); }}
      onClick={(event) => { const box = event.currentTarget.getBoundingClientRect(); if (event.target === event.currentTarget && (event.clientX < box.left || event.clientX > box.right || event.clientY < box.top || event.clientY > box.bottom)) close(); }}
      className="fixed inset-0 m-auto max-h-[85dvh] w-[calc(100%_-_2rem)] max-w-md overflow-y-auto rounded-xl bg-card p-5 text-foreground shadow-lg backdrop:bg-foreground/40 sm:p-6">
      <h2 id={titleId} className="min-w-0 break-words text-lg font-semibold">{t("earningConversion.title")}</h2>
      <form className="mt-5 grid min-w-0 gap-4" onSubmit={(event) => { event.preventDefault(); if (!success) { if (quote) confirm(); else review(); } }}>
        {success ? <div className="grid min-w-0 gap-3" role="status">
          <CheckCircle2 className="size-6 text-primary" aria-hidden="true" />
          <p className="break-words font-medium tabular-nums">{t("earningConversion.success", { amount: formatWalletUnits(success.amountUnits, "$", locale) })}</p>
          <p className="text-sm text-muted-foreground">{t("earningConversion.consent")}</p>
          <code className="break-all text-xs text-muted-foreground">{success.id}</code>
          {refreshFailed ? <p role="alert" className="text-sm text-destructive">{t("earningConversion.refreshFailed")}</p> : null}
        </div> : quote ? <>
          <div className="grid gap-1">
            <p className="text-sm text-muted-foreground">{t("earningConversion.review")}</p>
            <p className="break-all text-3xl font-semibold tabular-nums">{formatWalletUnits(quote.amountUnits, "$", locale)}</p>
            <p className="text-sm text-muted-foreground">{t("earningConversion.fee")}</p>
            {BigInt(quote.amountUnits) % 86_400n !== 0n ? <p className="break-all text-sm text-muted-foreground">{t("earningConversion.exact", { units: quote.amountUnits })}</p> : null}
          </div>
          <label className="flex min-h-11 items-start gap-3 text-sm leading-6">
            <input type="checkbox" className="mt-1 size-4 shrink-0 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" checked={acknowledged} onChange={(event) => setAcknowledged(event.target.checked)} disabled={busy} />
            {t("earningConversion.consent")}
          </label>
          {expired && !pending ? <p role="status" className="text-sm text-destructive">{t("earningConversion.expired")}</p> : null}
        </> : <>
          <p className="text-sm text-muted-foreground">{t("earningConversion.help")}</p>
          <fieldset className="grid min-w-0 gap-2 text-sm" disabled={busy || hasPending}>
            <legend className="mb-2 font-medium">{t("earningConversion.amount")}</legend>
            <label className="flex min-h-11 items-center gap-2"><input type="radio" name={titleId} checked={all} onChange={() => { setAll(true); setTouched(false); }} className="size-4 accent-primary" />{t("earningConversion.all")}</label>
            <label className="flex min-h-11 items-center gap-2"><input type="radio" name={titleId} checked={!all} onChange={() => setAll(false)} className="size-4 accent-primary" />{t("earningConversion.custom")}</label>
            {!all ? <input aria-label={t("earningConversion.amount")} aria-describedby={helperId} aria-invalid={touched && !amountValid} autoFocus inputMode="decimal" className={WALLET_INPUT} value={amount} onChange={(event) => setAmount(event.target.value)} onBlur={() => setTouched(true)} /> : null}
            <p id={helperId} className={`min-h-6 text-sm ${touched && !amountValid ? "text-destructive" : "text-muted-foreground"}`}>{touched && !amountValid ? t("earningConversion.invalid") : t("earningConversion.available", { amount: formatWalletUnits(available, "$", locale) })}</p>
          </fieldset>
        </>}
        {error ? <p role="alert" className="flex min-w-0 items-start gap-2 text-sm text-destructive"><CircleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />{error}</p> : null}
        <div className="flex min-w-0 flex-wrap items-center justify-end gap-2">
          <button type="button" className="min-h-11 whitespace-nowrap rounded-md px-3 text-sm hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:bg-muted disabled:opacity-50" disabled={busy} onClick={close}>{t("common.close")}</button>
          {success ? <Link href={returnPath ?? "/account/wallet/"} className="inline-flex min-h-11 items-center whitespace-nowrap rounded-md bg-primary px-4 text-sm font-semibold text-primary-foreground hover:bg-primary/90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">{t(returnPath ? "earningConversion.returnMarket" : "earningConversion.returnWallet")}</Link>
            : <>
              {quote && !pending ? <button type="button" className="min-h-11 whitespace-nowrap rounded-md px-3 text-sm hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring" disabled={busy} onClick={() => { setQuote(null); setAcknowledged(false); setError(""); }}>{t("earningConversion.back")}</button> : null}
              {pending ? <button type="button" className="min-h-11 whitespace-nowrap rounded-md px-3 text-sm hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring" disabled={busy} onClick={check}>{t("earningConversion.check")}</button> : null}
              {batchOffered && !pending && !quote ? <button type="button" className="min-h-11 whitespace-nowrap rounded-md px-3 text-sm hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50" disabled={busy} onClick={() => review(true)}>{t("earningConversion.reviewBatch")}</button> : null}
              <ConversionAction type="submit" phase={busy ? "loading" : error ? "error" : "default"} disabled={busy || (quote ? !acknowledged || (!pending && expired) : !amountValid || hasPending || !capacity?.canConvert)}>
                {t(busy ? "common.loading" : quote ? "earningConversion.confirm" : "earningConversion.preview")}
              </ConversionAction>
            </>}
        </div>
      </form>
    </dialog>
  </div>;
}

export function EarningConversionHistory({ convertedUnits }: { convertedUnits: string }) {
  const { session } = useAuth();
  const actor = session?.user?.id;
  return session?.authenticated && actor ? <ConversionHistory key={actor} convertedUnits={convertedUnits} /> : null;
}

function ConversionHistory({ convertedUnits }: { convertedUnits: string }) {
  const { t, locale } = useLocaleText();
  const [items, setItems] = React.useState<Conversion[]>([]);
  const [cursor, setCursor] = React.useState<string | null>(null);
  const [loaded, setLoaded] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [expanded, setExpanded] = React.useState(false);
  const loading = React.useRef(false);
  const generation = React.useRef(0);
  const load = React.useCallback(async (more = false) => {
    if (loading.current) return;
    const request = ++generation.current;
    loading.current = true; setBusy(true); setError("");
    try {
      const page = await getEarningConversions(more ? cursor ?? undefined : undefined);
      if (request !== generation.current) return;
      setItems((current) => more ? [...current, ...page.items.filter((value) => !current.some((old) => old.id === value.id))] : page.items);
      setCursor(page.nextCursor); setLoaded(true);
    } catch {
      if (request === generation.current) setError(t("earningConversion.historyFailed"));
    } finally {
      if (request === generation.current) { loading.current = false; setBusy(false); }
    }
  }, [cursor, t]);
  React.useEffect(() => {
    ++generation.current; loading.current = false; setBusy(false);
    setLoaded(false); setItems([]); setCursor(null); setError("");
    return () => { ++generation.current; };
  }, [convertedUnits]);
  React.useEffect(() => { if (expanded && !loaded && !error) void load(); }, [expanded, loaded, error, load]);
  return <details className="min-w-0 text-sm" data-testid="earning-conversion-history" onToggle={(event) => setExpanded(event.currentTarget.open)}>
    <summary className="min-h-11 cursor-pointer rounded-md py-3 font-medium hover:text-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">{t("earningConversion.history")} · {formatWalletUnits(convertedUnits, "$", locale)}</summary>
    <ul className="grid max-h-80 gap-3 overflow-y-auto overscroll-contain py-2">
      {items.map((item) => <li key={item.id} className="grid min-w-0 gap-1 rounded-md bg-muted/50 px-3 py-2">
        <div className="flex flex-wrap items-center justify-between gap-2"><strong className="tabular-nums">{formatWalletUnits(item.amountUnits, "$", locale)}</strong><time className="text-muted-foreground" dateTime={item.createdAt}>{new Date(item.createdAt).toLocaleString(locale)}</time></div>
        <code className="break-all text-xs text-muted-foreground">{item.id}</code>
      </li>)}
    </ul>
    {busy ? <p role="status" className="text-muted-foreground">{t("common.loading")}</p> : loaded && items.length === 0 ? <p className="text-muted-foreground">{t("earningConversion.historyEmpty")}</p> : null}
    {error ? <p role="alert" className="text-destructive">{error}</p> : null}
    {cursor || error ? <button type="button" disabled={busy} onClick={() => void load(loaded && !!cursor)} className="min-h-11 rounded-md px-3 hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-50">{t(busy ? "common.loading" : error ? "common.retry" : "marketWallet.more")}</button> : null}
  </details>;
}
