"use client";

/* Wallet & trading notification bell: unread count, a disclosure panel with the
 * feed, per-item and mark-all read. Escape or an outside click closes it and
 * returns focus to the trigger.
 */

import * as React from "react";
import { Bell, Check, CheckCheck, Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WalletSectionLabel } from "@/components/market/wallet-primitives";
import { messages, type MessageKey } from "@/lib/i18n";
import {
  getMarketNotifications,
  markMarketNotificationRead,
  type MarketNotification,
} from "@/lib/market-wallet-api";
import { formatMinorUsd, formatWalletUnits } from "@/lib/market-wallet-money";
import { cn } from "@/lib/utils";

const REFRESH_MS = 60_000;

function kindKey(kind: string): MessageKey {
  const key = `marketNotifications.kind.${kind}`;
  return (key in messages.en ? key : "marketNotifications.kind.other") as MessageKey;
}

function payloadAmount(payload: Record<string, unknown>) {
  if (typeof payload.amountMinor === "number" && Number.isSafeInteger(payload.amountMinor)) return formatMinorUsd(payload.amountMinor);
  if (typeof payload.amountUnits === "string" && /^-?\d+$/.test(payload.amountUnits)) return formatWalletUnits(payload.amountUnits);
  return null;
}

export function MarketNotificationsBell() {
  const { locale, t } = useLocaleText();
  const panelId = React.useId();
  const [open, setOpen] = React.useState(false);
  const [items, setItems] = React.useState<MarketNotification[]>([]);
  const [cursor, setCursor] = React.useState<string | null>(null);
  const [unread, setUnread] = React.useState(0);
  const [loading, setLoading] = React.useState(false);
  const [busyId, setBusyId] = React.useState<string | null>(null);
  const [error, setError] = React.useState("");
  const rootRef = React.useRef<HTMLDivElement>(null);
  const triggerRef = React.useRef<HTMLButtonElement>(null);
  const versionRef = React.useRef(0);

  const load = React.useCallback(async () => {
    const version = ++versionRef.current;
    setLoading(true);
    try {
      const page = await getMarketNotifications();
      if (version !== versionRef.current) return;
      // The bell sits beside wallet content; a malformed feed must leave it
      // empty rather than take the whole wallet page down.
      setItems(Array.isArray(page?.items) ? page.items : []);
      setCursor(typeof page?.nextCursor === "string" ? page.nextCursor : null);
      setUnread(Number.isSafeInteger(page?.unread) ? Math.max(0, page.unread) : 0);
      setError("");
    } catch (cause) {
      if (version === versionRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (version === versionRef.current) setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void load();
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  React.useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") { setOpen(false); triggerRef.current?.focus(); }
    };
    const onPointer = (event: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
    };
  }, [open]);

  const toggle = () => {
    setOpen((current) => {
      if (!current) void load();
      return !current;
    });
  };

  const markRead = async (id: string | "all") => {
    if (busyId) return;
    setBusyId(id);
    setError("");
    try {
      await markMarketNotificationRead(id);
      const readAt = new Date().toISOString();
      versionRef.current += 1;
      setItems((current) => current.map((item) => id === "all" || item.id === id ? { ...item, readAt: item.readAt ?? readAt } : item));
      setUnread((current) => id === "all" ? 0 : Math.max(0, current - 1));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusyId(null);
    }
  };

  const loadMore = async () => {
    if (!cursor || loading) return;
    const version = versionRef.current;
    setLoading(true);
    try {
      const page = await getMarketNotifications(cursor);
      if (version !== versionRef.current) return;
      const more = Array.isArray(page?.items) ? page.items : [];
      setItems((current) => [...current, ...more.filter((item) => !current.some((prior) => prior.id === item.id))]);
      setCursor(typeof page?.nextCursor === "string" ? page.nextCursor : null);
      setUnread(page.unread);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  };

  const label = unread > 0 ? t("marketNotifications.bellUnread", { count: unread }) : t("marketNotifications.bell");

  return (
    <div ref={rootRef} className="relative">
      <button
        ref={triggerRef}
        type="button"
        aria-label={label}
        title={label}
        aria-expanded={open}
        aria-controls={panelId}
        onClick={toggle}
        className="relative inline-flex size-11 items-center justify-center rounded-xl border border-border bg-card text-foreground transition-all duration-200 ease-out hover:-translate-y-0.5 hover:border-accent/30 hover:shadow-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:scale-[0.98] motion-reduce:transition-none motion-reduce:hover:translate-y-0"
      >
        <Bell className="size-4" aria-hidden="true" />
        {unread > 0 ? (
          <span className="absolute -right-1.5 -top-1.5 inline-flex min-w-5 items-center justify-center rounded-full bg-gradient-to-r from-[#0052FF] to-[#4D7CFF] px-1.5 text-[0.68rem] font-semibold leading-5 text-white shadow-[0_4px_14px_rgba(0,82,255,0.25)]" aria-hidden="true">
            {unread > 99 ? "99+" : unread}
          </span>
        ) : null}
      </button>
      <div
        id={panelId}
        role="region"
        aria-label={t("marketNotifications.title")}
        hidden={!open}
        className="absolute right-0 top-full z-40 mt-2 w-[min(24rem,calc(100vw-2rem))] overflow-hidden rounded-2xl border border-border bg-card shadow-[0_20px_25px_rgba(0,0,0,0.1)] animate-fade-in-up"
      >
        <div className="flex items-center justify-between gap-2 border-b border-border px-4 py-3">
          <WalletSectionLabel pulse={unread > 0}>{t("marketNotifications.title")}</WalletSectionLabel>
          <button
            type="button"
            disabled={!unread || !!busyId}
            onClick={() => void markRead("all")}
            className="inline-flex min-h-11 items-center gap-1.5 rounded-lg px-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-40"
          >
            {busyId === "all" ? <Loader2 className="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <CheckCheck className="size-3.5" aria-hidden="true" />}
            {t("marketNotifications.markAllRead")}
          </button>
        </div>
        <p className="sr-only" aria-live="polite">{loading ? t("common.loading") : ""}</p>
        {error ? <p role="alert" className="px-4 py-2 text-xs text-destructive">{error}</p> : null}
        <ul className="max-h-[60vh] divide-y divide-border overflow-y-auto">
          {items.map((item) => {
            const amount = payloadAmount(item.payload ?? {});
            return (
              <li key={item.id} className={cn("flex items-start gap-3 px-4 py-3 text-sm", !item.readAt && "bg-accent/[0.03]")}>
                <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", item.readAt ? "bg-transparent" : "bg-accent")} aria-hidden="true" />
                <div className="min-w-0 flex-1">
                  <p className={cn("break-words", !item.readAt && "font-semibold")}>
                    {t(kindKey(item.eventKind))}
                    {!item.readAt ? <span className="sr-only"> · {t("marketNotifications.unread")}</span> : null}
                  </p>
                  <p className="mt-0.5 text-xs text-muted-foreground">
                    {amount ? <span className="tabular-nums">{amount} · </span> : null}
                    {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.createdAt))}
                  </p>
                  {item.eventKind === "wallet_transfer_sent" || item.eventKind === "wallet_transfer_received" ? <a href="/account/wallet/" className="mt-1 block break-all text-xs text-accent underline underline-offset-2">{String(item.payload?.[item.eventKind === "wallet_transfer_sent" ? "recipientEmail" : "senderEmail"] ?? "")}</a> : null}
                  {item.eventKind === "c2c_order_updated" && typeof item.payload?.orderId === "string" ? <a href={`/account/wallet/?c2cOrder=${encodeURIComponent(item.payload.orderId)}`} className="mt-1 inline-flex min-h-11 items-center text-xs text-primary underline underline-offset-2">{t("c2c.detail")}</a> : null}
                  {item.eventKind === "commission_policy_changed" && typeof item.payload?.rateBps === "number" && typeof item.payload?.effectiveAt === "string" && Number.isFinite(Date.parse(item.payload.effectiveAt)) ? <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("commission.scheduled", { rate: (item.payload.rateBps / 100).toString(), time: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.payload.effectiveAt)) })}</p> : null}
                </div>
                {!item.readAt ? (
                  <button
                    type="button"
                    aria-label={t("marketNotifications.markRead")}
                    title={t("marketNotifications.markRead")}
                    disabled={!!busyId}
                    onClick={() => void markRead(item.id)}
                    className="inline-flex size-11 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-40"
                  >
                    {busyId === item.id ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Check className="size-4" aria-hidden="true" />}
                  </button>
                ) : null}
              </li>
            );
          })}
          {!loading && !items.length ? <li className="px-4 py-6 text-center text-sm text-muted-foreground">{t("marketNotifications.empty")}</li> : null}
        </ul>
        {cursor ? (
          <div className="border-t border-border px-4 py-2">
            <button type="button" disabled={loading} onClick={() => void loadMore()} className="inline-flex min-h-11 w-full items-center justify-center rounded-lg text-xs font-medium text-accent hover:bg-accent/5 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-50">
              {t("marketWallet.more")}
            </button>
          </div>
        ) : null}
      </div>
    </div>
  );
}
