"use client";

import * as React from "react";
import { Bell, Check, CheckCheck, Loader2, RefreshCw } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getNotifications, markNotificationRead, notificationKindKey, notificationHref,
  type UserNotification } from "@/lib/user-notifications";
import { formatMinorUsd, formatWalletUnits } from "@/lib/market-wallet-money";
import { cn } from "@/lib/utils";

const REFRESH_MS = 60_000;

function payloadAmount(payload: Record<string, unknown>) {
  if (typeof payload.amountMinor === "number" && Number.isSafeInteger(payload.amountMinor)) return formatMinorUsd(payload.amountMinor);
  if (typeof payload.amountUnits === "string" && /^-?\d+$/.test(payload.amountUnits)) return formatWalletUnits(payload.amountUnits);
  return null;
}

/** A business-event feed, independent from email delivery and chat unread state. */
export function NotificationsBell() {
  const { session } = useAuth();
  const { locale, t } = useLocaleText();
  const panelId = React.useId();
  const [open, setOpen] = React.useState(false);
  const [items, setItems] = React.useState<UserNotification[]>([]);
  const [cursor, setCursor] = React.useState<string | null>(null);
  const [readThrough, setReadThrough] = React.useState<string | null>(null);
  const [unread, setUnread] = React.useState(0);
  const [loading, setLoading] = React.useState(false);
  const [busyId, setBusyId] = React.useState<string | null>(null);
  const [error, setError] = React.useState(false);
  const [panelOffset, setPanelOffset] = React.useState(0);
  const rootRef = React.useRef<HTMLDivElement>(null);
  const triggerRef = React.useRef<HTMLButtonElement>(null);
  const listRef = React.useRef<HTMLUListElement>(null);
  const versionRef = React.useRef(0);
  const busyRef = React.useRef(false);
  const loadingRef = React.useRef(false);
  const openRef = React.useRef(false);

  const load = React.useCallback(async () => {
    if (busyRef.current) return;
    const version = ++versionRef.current;
    loadingRef.current = true;
    setLoading(true);
    try {
      const page = await getNotifications();
      if (version !== versionRef.current) return;
      setItems(page.items);
      setCursor(page.nextCursor);
      setReadThrough(page.readThrough);
      setUnread(page.unread);
      setError(false);
    } catch {
      if (version === versionRef.current) setError(true);
    } finally {
      if (version === versionRef.current) { loadingRef.current = false; setLoading(false); }
    }
  }, []);

  React.useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      // Do not collapse paginated history while the user is reading it.
      if (document.visibilityState === "visible" && !openRef.current) void load();
    }, REFRESH_MS);
    return () => { window.clearInterval(timer); versionRef.current += 1; };
  }, [load]);

  React.useEffect(() => {
    if (!open) return;
    const placePanel = () => {
      const right = rootRef.current?.getBoundingClientRect().right ?? window.innerWidth;
      const width = Math.min(320, window.innerWidth - 32);
      setPanelOffset(Math.max(0, width + 16 - right));
    };
    placePanel();
    window.addEventListener("resize", placePanel);
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") { openRef.current = false; setOpen(false); triggerRef.current?.focus(); }
    };
    const onPointer = (event: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(event.target as Node)) { openRef.current = false; setOpen(false); }
    };
    document.addEventListener("keydown", onKey);
    document.addEventListener("pointerdown", onPointer);
    return () => {
      document.removeEventListener("keydown", onKey);
      document.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("resize", placePanel);
    };
  }, [open]);

  const toggle = () => {
    openRef.current = !openRef.current;
    setOpen(openRef.current);
    if (openRef.current) void load();
  };

  const markRead = async (id: string | "all") => {
    if (busyRef.current || (id === "all" && !readThrough)) return;
    busyRef.current = true;
    const version = ++versionRef.current;
    let reload = false;
    loadingRef.current = false;
    setLoading(false);
    setBusyId(id);
    setError(false);
    try {
      const result = await markNotificationRead(id, id === "all" ? readThrough! : undefined);
      if (version !== versionRef.current) return;
      if (id !== "all") {
        setItems((current) => current.map((item) => item.id === id ? { ...item, readAt: item.readAt ?? result.readAt } : item));
      }
      setUnread(result.unread);
      // A new or delayed event may have arrived since our read snapshot.
      // Do not guess its read state or refresh an unmounted account's inbox.
      reload = id === "all";
    } catch {
      if (version === versionRef.current) setError(true);
    } finally {
      if (version === versionRef.current) {
        busyRef.current = false;
        setBusyId(null);
        if (reload) void load();
      }
    }
  };

  const loadMore = async () => {
    if (!cursor || loadingRef.current || busyRef.current) return;
    const version = ++versionRef.current;
    loadingRef.current = true;
    setLoading(true);
    try {
      const page = await getNotifications(cursor);
      if (version !== versionRef.current) return;
      setItems((current) => [...current, ...page.items.filter((item) => !current.some((prior) => prior.id === item.id))]);
      setCursor(page.nextCursor);
      setUnread(page.unread);
      setError(false);
    } catch {
      if (version === versionRef.current) setError(true);
    } finally {
      if (version === versionRef.current) { loadingRef.current = false; setLoading(false); }
    }
  };

  React.useEffect(() => {
    const list = listRef.current;
    if (!open || !cursor || !list) return;
    const onScroll = () => {
      if (list.scrollTop + list.clientHeight < list.scrollHeight - 96) return;
      void loadMore();
    };
    list.addEventListener("scroll", onScroll, { passive: true });
    return () => list.removeEventListener("scroll", onScroll);
  }, [cursor, loadMore, open]);

  const label = unread > 0 ? t("notificationCenter.bellUnread", { count: unread }) : t("notificationCenter.bell");

  return (
    <div ref={rootRef} className="relative">
      <button ref={triggerRef} type="button" aria-label={label} title={label} aria-expanded={open} aria-controls={panelId} onClick={toggle}
        className="relative inline-flex size-11 items-center justify-center rounded-md text-slate-500 transition-colors hover:bg-slate-100 hover:text-slate-700 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:bg-slate-200">
        <Bell className="size-4" aria-hidden="true" />
        {unread > 0 ? <span className="absolute right-1 top-1 inline-flex min-w-4 items-center justify-center rounded-full bg-rose-600 px-1 text-[0.65rem] font-semibold leading-4 text-white" aria-hidden="true">{unread > 99 ? "99+" : unread}</span> : null}
      </button>
      <div id={panelId} role="region" aria-label={t("notificationCenter.title")} hidden={!open} style={{ right: -panelOffset }}
        className="absolute right-0 top-full z-40 mt-2 w-[min(20rem,calc(100vw-2rem))] overflow-hidden rounded-xl border border-border bg-card shadow-[0_16px_24px_rgba(0,0,0,0.1)] animate-fade-in-up">
        <div className="flex items-center gap-1 border-b border-border px-3 py-1.5">
          <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{t("notificationCenter.title")}</h2>
          <button type="button" disabled={loading || !!busyId} onClick={() => void load()} aria-label={t("common.reload")} title={t("common.reload")}
            className="ml-auto inline-flex size-11 shrink-0 items-center justify-center rounded-lg text-muted-foreground hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-40">
            <RefreshCw className="size-3.5" aria-hidden="true" />
          </button>
          <button type="button" disabled={!unread || !readThrough || loading || !!busyId} onClick={() => void markRead("all")}
            className="inline-flex min-h-11 items-center gap-1.5 rounded-lg px-2 text-xs font-medium text-muted-foreground transition-colors hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-40">
            {busyId === "all" ? <Loader2 className="size-3.5 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <CheckCheck className="size-3.5" aria-hidden="true" />}
            {t("marketNotifications.markAllRead")}
          </button>
        </div>
        <p className="sr-only" aria-live="polite">{loading ? t("common.loading") : ""}</p>
        {error ? <p role="alert" className="px-4 py-2 text-xs text-destructive">{t("notificationCenter.unavailable")}</p> : null}
        <ul ref={listRef} className="max-h-[min(24rem,calc(100vh-8rem))] divide-y divide-border overflow-y-auto overscroll-contain">
          {items.map((item) => {
            const amount = payloadAmount(item.payload);
            return <li key={item.id} className={cn("flex items-start gap-3 px-4 py-3 text-sm", !item.readAt && "bg-accent/[0.03]")}>
              <span className={cn("mt-1.5 size-2 shrink-0 rounded-full", item.readAt ? "bg-transparent" : "bg-accent")} aria-hidden="true" />
              <div className="min-w-0 flex-1">
                <p className="mb-0.5 text-[11px] text-muted-foreground">{t(`notificationCenter.category.${item.category}`)}</p>
                <p className={cn("break-words", !item.readAt && "font-semibold")}>
                  {t(notificationKindKey(item.eventKind))}
                  {!item.readAt ? <span className="sr-only"> · {t("marketNotifications.unread")}</span> : null}
                </p>
                {item.category === "client" && typeof item.payload.clientLabel === "string" ? <p className="mt-1 break-words text-xs text-muted-foreground">{item.payload.clientLabel}</p> : null}
                {item.category === "system" && typeof item.payload.title === "string" ? <p className="mt-1 break-words text-xs text-muted-foreground">{item.payload.title}</p> : null}
                <p className="mt-0.5 text-xs text-muted-foreground">
                  {amount ? <span className="tabular-nums">{amount} · </span> : null}
                  {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.createdAt))}
                </p>
                {item.eventKind === "wallet_transfer_sent" || item.eventKind === "wallet_transfer_received" ? <p className="mt-1 break-all text-xs text-muted-foreground">{String(item.payload[item.eventKind === "wallet_transfer_sent" ? "recipientEmail" : "senderEmail"] ?? "")}</p> : null}
                {item.eventKind === "commission_policy_changed" && typeof item.payload.rateBps === "number" && typeof item.payload.effectiveAt === "string" && Number.isFinite(Date.parse(item.payload.effectiveAt)) ? <p className="mt-1 text-xs leading-5 text-muted-foreground">{t("commission.scheduled", { rate: (item.payload.rateBps / 100).toString(), time: new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.payload.effectiveAt)) })}</p> : null}
                {item.category !== "system" || session?.isAdmin ? (
                  <a href={notificationHref(item)} className="mt-1 inline-flex min-h-8 items-center text-xs text-primary underline underline-offset-2">{t("notificationCenter.view")}</a>
                ) : null}
              </div>
              {!item.readAt ? <button type="button" aria-label={t("marketNotifications.markRead")} title={t("marketNotifications.markRead")} disabled={!!busyId} onClick={() => void markRead(item.id)}
                className="inline-flex size-11 shrink-0 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:opacity-40">
                {busyId === item.id ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : <Check className="size-4" aria-hidden="true" />}
              </button> : null}
            </li>;
          })}
          {!loading && !items.length ? <li className="px-4 py-6 text-center text-sm text-muted-foreground">{t("marketNotifications.empty")}</li> : null}
          {cursor && loading ? <li className="flex min-h-8 items-center justify-center" aria-hidden="true"><Loader2 className="size-3.5 animate-spin motion-reduce:animate-none text-muted-foreground" /></li> : null}
        </ul>
      </div>
    </div>
  );
}
