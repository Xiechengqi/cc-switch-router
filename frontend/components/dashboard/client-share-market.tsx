"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Loader2, Settings2, ShoppingCart } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientChat } from "@/components/chat/client-chat";
import { MarketAccessDialog } from "@/components/common/seller-approval-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ShareRentalDetail } from "./account-share-rentals";
import { ClientShareOwnerManagement } from "./client-share-owner-management";
import { useShareMarketRentalActions } from "./share-market/rental-controls";
import { useShareSeatCheckout, type ShareCheckoutSeat } from "./share-market/share-seat-checkout";
import { useQuotaActor } from "./share-market/seat-value-quota";
import { formatSeatPrice, formatSeatQuota, subscriptionStatusKey } from "./share-market/market-utils";
import { clientSeatAction, clientSeatGroups, clientShareEntryLabel, clientShareHref, getClientRentalContexts, getClientSeatContext, getClientShareEntries,
  type ClientShareEntry, type ClientShareSeatContext } from "@/lib/client-share-market";
import { readAuthState } from "@/lib/auth";
import { valueReadinessActor } from "@/lib/use-value-quota-readiness";
import { readShareCheckoutRecovery } from "@/lib/share-checkout-recovery";
import type { MarketEligibility } from "@/lib/types";

type Selection = { shareId: string; seatId?: string; subscriptionId?: string };
type Context = {
  entries: Map<string, ClientShareEntry>;
  recovery: ClientShareEntry[];
  recoveryError: boolean;
  summaryError: boolean;
  more: boolean;
  loading: boolean;
  loadMore(): void;
  retry(): void;
  register(id: string): () => void;
  open(selection: Selection): void;
};
const MarketContext = React.createContext<Context | null>(null);
const POLL_MS = 15_000;
const SEAT_ACTION_KEYS = {
  login: "clientShares.login", rent: "clientShares.rent", approval: "clientShares.approval",
  occupied: "shareMarket.occupied", unavailable: "shareMarket.unavailable",
} as const;

function fromLocation(): Selection | null {
  const q = new URLSearchParams(window.location.search);
  const shareId = q.get("marketShare");
  if (!shareId || !/^[A-Za-z0-9_-]{1,256}$/.test(shareId)) return null;
  const valid = (v: string | null) => v && /^[A-Za-z0-9_-]{1,256}$/.test(v) ? v : undefined;
  return { shareId, seatId: valid(q.get("seatId")), subscriptionId: valid(q.get("subscriptionId")) };
}
function replaceSelection(selection: Selection | null) {
  const url = new URL(window.location.href);
  for (const key of ["marketShare", "seatId", "subscriptionId"]) url.searchParams.delete(key);
  if (selection) {
    url.searchParams.set("marketShare", selection.shareId);
    if (selection.seatId) url.searchParams.set("seatId", selection.seatId);
    if (selection.subscriptionId) url.searchParams.set("subscriptionId", selection.subscriptionId);
  }
  window.history.replaceState(window.history.state, "", `${url.pathname}${url.search}${url.hash}`);
}

/** Key the private subtree by the login context, not just the user ID. */
export function ClientShareMarketProvider({ children, onChanged }: { children: React.ReactNode; onChanged: () => Promise<void> | void }) {
  const { session } = useAuth();
  const actor = useQuotaActor();
  const localEmail = readAuthState().email?.toLowerCase();
  // Cookie-only sessions are valid too. If a browser login is changing,
  // however, never reuse the previous account's still-loading session.
  const authenticated = !!session?.authenticated && !!session.user && (!localEmail || session.user.email.toLowerCase() === localEmail);
  const userId = authenticated ? session!.user!.id : "";
  return <ClientShareMarketSession key={`${userId}:${actor}`} actor={actor} userId={userId} onChanged={onChanged}>{children}</ClientShareMarketSession>;
}

function ClientShareMarketSession({ children, actor, userId, onChanged }: {
  children: React.ReactNode; actor: string; userId: string; onChanged: () => Promise<void> | void;
}) {
  const { session, passwordSettingsOpen } = useAuth();
  const { t, locale } = useLocaleText();
  const chat = useClientChat();
  const registrations = React.useRef(new Map<string, number>());
  const [ids, setIds] = React.useState<string[]>([]);
  const [entries, setEntries] = React.useState(new Map<string, ClientShareEntry>());
  const [recovery, setRecovery] = React.useState<ClientShareEntry[]>([]);
  const [recoveryError, setRecoveryError] = React.useState(false);
  const [summaryError, setSummaryError] = React.useState(false);
  const [pages, setPages] = React.useState(1);
  const [more, setMore] = React.useState(false);
  const [loading, setLoading] = React.useState(false);
  const [tick, retry] = React.useReducer(n => n + 1, 0);
  const [selection, setSelection] = React.useState<Selection | null>(null);
  const [detail, setDetail] = React.useState<ClientShareSeatContext | null>(null);
  const [detailPages, setDetailPages] = React.useState(1);
  const [detailError, setDetailError] = React.useState(false);
  const [detailLoading, setDetailLoading] = React.useState(false);
  const [access, setAccess] = React.useState<(ShareCheckoutSeat & { eligibility: MarketEligibility }) | null>(null);
  const [loginWaiting, setLoginWaiting] = React.useState(false);
  const current = React.useCallback(() => actor === valueReadinessActor(), [actor]);
  const register = React.useCallback((id: string) => {
    const map = registrations.current;
    map.set(id, (map.get(id) ?? 0) + 1);
    setIds([...map.keys()].sort());
    return () => {
      const count = (map.get(id) ?? 1) - 1;
      if (count) map.set(id, count); else map.delete(id);
      setIds([...map.keys()].sort());
    };
  }, []);
  // Include deep-linked Shares even when their card has not mounted yet.
  const idsKey = [...new Set([...ids, ...(selection ? [selection.shareId] : [])])].sort().join(",");
  const ownerMode = !!selection && !selection.subscriptionId && entries.get(selection.shareId)?.entryKind === "manage";
  const waitingForEntry = !!selection && !entries.has(selection.shareId) && !summaryError;
  const changed = async () => {
    if (!current()) return;
    retry();
    await onChanged();
  };
  const rentals = useShareMarketRentalActions(changed);
  const checkout = useShareSeatCheckout({ onChanged: changed, onAccessRequired: (item, eligibility) => setAccess({ ...item, eligibility }),
    returnTo: item => clientShareHref(item.listing.shareId, item.seat.id) });
  const blocked = checkout.interactionActive || rentals.interactionActive || !!access || passwordSettingsOpen || loginWaiting;

  React.useEffect(() => {
    const read = () => { if (current()) { setSelection(fromLocation()); setDetail(null); setDetailPages(1); } };
    read(); window.addEventListener("popstate", read);
    return () => window.removeEventListener("popstate", read);
  }, [current]);
  React.useEffect(() => {
    const changed = (event: Event) => setLoginWaiting(!!(event as CustomEvent<{ open: boolean }>).detail?.open);
    window.addEventListener("router-login-dialog-changed", changed);
    return () => window.removeEventListener("router-login-dialog-changed", changed);
  }, []);
  React.useEffect(() => {
    const timer = window.setInterval(() => { if (document.visibilityState !== "hidden" && !blocked && !ownerMode && current()) retry(); }, POLL_MS);
    const visible = () => { if (document.visibilityState !== "hidden" && !blocked && !ownerMode && current()) retry(); };
    document.addEventListener("visibilitychange", visible);
    return () => { window.clearInterval(timer); document.removeEventListener("visibilitychange", visible); };
  }, [blocked, ownerMode, current]);
  React.useEffect(() => {
    const controller = new AbortController();
    // Batch the mounted cards, rather than one request for each card.
    const timer = window.setTimeout(() => {
      const requested = idsKey ? idsKey.split(",") : [];
      void Promise.all(Array.from({ length: Math.ceil(requested.length / 100) }, (_, i) => getClientShareEntries(requested.slice(i * 100, (i + 1) * 100), controller.signal)))
        .then(pages => { if (!controller.signal.aborted && current()) { setEntries(new Map(pages.flatMap(p => p.items).map(e => [e.shareId, e]))); setSummaryError(false); } })
        .catch(() => { if (!controller.signal.aborted && current()) setSummaryError(true); });
    }, 80);
    return () => { controller.abort(); window.clearTimeout(timer); };
  }, [idsKey, tick, current]);
  React.useEffect(() => {
    if (!userId) return;
    const controller = new AbortController();
    setLoading(true);
    void (async () => {
      let cursor: string | null = null;
      const items: ClientShareEntry[] = [];
      const seen = new Set<string>();
      for (let i = 0; i < pages; i++) {
        const page = await getClientRentalContexts(cursor, controller.signal);
        items.push(...page.items); cursor = page.nextCursor;
        if (!cursor) break;
        if (seen.has(cursor)) throw new Error("Repeated rental cursor");
        seen.add(cursor);
      }
      if (!controller.signal.aborted && current()) { setRecovery([...new Map(items.map(entry => [entry.shareId, entry])).values()]); setMore(!!cursor); setRecoveryError(false); }
    })().catch(() => { if (!controller.signal.aborted && current()) setRecoveryError(true); })
      .finally(() => { if (!controller.signal.aborted && current()) setLoading(false); });
    return () => controller.abort();
  }, [userId, pages, tick, current]);
  const shareId = selection?.shareId;
  const selectedSub = selection?.subscriptionId;
  React.useEffect(() => {
    if (!shareId || ownerMode || waitingForEntry) { setDetail(null); return; }
    const controller = new AbortController();
    setDetailLoading(true); setDetailError(false);
    // Never display the preceding Share's records during a new request.
    setDetail(previous => previous?.shareId === shareId ? previous : null);
    void (async () => {
      let cursor: string | null = null;
      let result: ClientShareSeatContext | null = null;
      const seen = new Set<string>();
      for (let i = 0; i < detailPages; i++) {
        const page = await getClientSeatContext(shareId, { cursor, subscriptionId: selectedSub }, controller.signal);
        result = result ? { ...page, subscriptions: [...new Map([...result.subscriptions, ...page.subscriptions].map(s => [s.id, s])).values()] } : page;
        cursor = page.nextCursor;
        if (!cursor) break;
        if (seen.has(cursor)) throw new Error("Repeated seat cursor");
        seen.add(cursor);
      }
      if (!controller.signal.aborted && current()) setDetail(result);
    })().catch(() => { if (!controller.signal.aborted && current()) setDetailError(true); })
      .finally(() => { if (!controller.signal.aborted && current()) setDetailLoading(false); });
    return () => controller.abort();
  }, [shareId, selectedSub, detailPages, tick, userId, current, ownerMode, waitingForEntry]);
  // Restore only a non-sensitive top-up draft; always obtain a fresh quote,
  // and never submit merely because a return link was opened.
  const restored = React.useRef("");
  React.useEffect(() => {
    if (!userId || !detail?.listing || detailLoading || detailError || blocked || !selection?.seatId) return;
    const recovery = readShareCheckoutRecovery(`${userId}:${actor}`);
    const draft = recovery?.draft;
    const seat = detail.listing.seats.find(s => s.id === selection.seatId);
    const key = `${detail.shareId}:${seat?.id}`;
    if (!recovery?.pending && draft?.shareId === detail.shareId && draft.seatId === seat?.id && seat?.canRent && restored.current !== key) {
      restored.current = key;
      void checkout.open({ listing: detail.listing, seat });
    }
  }, [userId, actor, detail, selection, detailLoading, detailError, blocked, checkout]);
  const open = (next: Selection) => {
    if (!current() || blocked) return;
    if (selection?.shareId !== next.shareId) setDetail(null);
    setDetailPages(1); setSelection(next); replaceSelection(next);
  };
  const close = () => { if (!blocked) { setSelection(null); setDetail(null); replaceSelection(null); } };
  const showRental = (id: string, seatId: string) => {
    if (!selection) return;
    const next = { ...selection, subscriptionId: id, seatId };
    setSelection(next); replaceSelection(next);
  };
  const trigger = (item: ShareCheckoutSeat) => {
    checkout.setError("");
    if (!userId) {
      const next = { shareId: item.listing.shareId, seatId: item.seat.id };
      setSelection(next); replaceSelection(next); setLoginWaiting(true);
      window.dispatchEvent(new CustomEvent("router-open-login"));
    } else if (clientSeatAction(item.seat, true) === "approval") {
      setAccess({ ...item, eligibility: item.seat.eligibility });
    } else if (item.seat.canRent && !detailError) void checkout.open(item);
  };
  const groups = detail ? clientSeatGroups(detail) : null;
  const sub = detail?.subscriptions.find(s => s.id === selection?.subscriptionId);
  const ownLive = !!groups?.mine.length;
  const entryMap = new Map([...recovery.map(e => [e.shareId, e] as const), ...entries]);
  const state = detail?.resourceState;
  return <MarketContext.Provider value={{ entries: entryMap, recovery, recoveryError, summaryError, more, loading, loadMore: () => setPages(n => n + 1), retry, register, open }}>
    {children}
    {ownerMode && selection && !blocked && current() ? <ClientShareOwnerManagement key={`${actor}:${selection.shareId}`} shareId={selection.shareId} actor={actor} onClose={close} onChanged={changed} /> : null}
    <Modal.Backdrop isOpen={!!selection && !ownerMode && !blocked && current()} onOpenChange={value => !value && close()}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light max-h-[calc(100dvh-1rem)] w-[min(620px,calc(100vw-1rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
          <Modal.Header><div className="min-w-0"><Modal.Heading>{t("clientShares.title")}</Modal.Heading><p className="mt-1 break-all text-xs text-slate-500">{detail?.listing?.shareName || entryMap.get(shareId ?? "")?.subdomain || shareId}</p></div></Modal.Header>
          <Modal.Body className="grid min-h-0 gap-4 overflow-y-auto">
            {detailLoading || waitingForEntry ? <p role="status" className="flex gap-2 text-sm"><Loader2 className="h-4 w-4 animate-spin" />{t("common.loading")}</p> : null}
            {detailError || checkout.error || rentals.error ? <p role="alert" className="text-sm text-danger">{detailError ? t("clientShares.loadError") : checkout.error || rentals.error}</p> : null}
            {state && state !== "listed" ? <p className="text-xs text-muted-foreground">{t(state === "missing" ? "clientShares.missing" : "clientShares.unlisted")}</p> : null}
            {sub ? <ShareRentalDetail subscription={sub} busy={detailLoading || detailError || rentals.busyId === sub.id} {...(!detailError ? rentals.rowActions(sub) : {})} onChanged={changed} /> : groups ? <>
              {groups.mine.length ? <section className="grid gap-2"><h3 className="text-sm font-semibold">{t("clientShares.mine")}</h3>{groups.mine.map(s => <Button key={s.id} variant="outline" className="min-h-11 justify-between whitespace-normal" onPress={() => showRental(s.id, s.seatId)}><span>#{s.seatPosition}</span><span>{t(subscriptionStatusKey(s.status) ?? "shareMarket.unavailable")}</span></Button>)}</section> : null}
              {detail?.listing ? <>{[["clientShares.available", groups.available], ["clientShares.occupied", groups.occupied]].map(([heading, items]) => {
                const seats = items as typeof groups.available;
                return seats.length ? <section key={heading as string} className="grid gap-2"><h3 className="text-sm font-semibold">{t(heading as "clientShares.available")}</h3>{seats.map(seat => {
                  const action = clientSeatAction(seat, !!userId);
                  const canAct = !ownLive && !detailLoading && !detailError && ["login", "rent", "approval"].includes(action);
                  return <div key={seat.id} data-client-share-seat={seat.id} className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md border border-border p-3">
                    <div className="min-w-0"><p className="text-sm font-medium">#{seat.position} · {formatSeatPrice(seat, locale, t("shareMarket.free"), t("market.day"))}</p><p className="mt-1 break-words text-xs text-muted-foreground">{formatSeatQuota(seat, locale, t("common.unlimited"), p => t(`shareMarket.period.${p}`), amount => t("shareMarket.quota.usdAmount", { amount }))}</p></div>
                    <Button size="sm" variant="primary" isDisabled={!canAct} onPress={() => detail?.listing && trigger({ listing: detail.listing, seat })}>{t(SEAT_ACTION_KEYS[action])}</Button>
                  </div>;
                })}</section> : null;
              })}</> : null}
              {ownLive && groups.available.length ? <p className="text-xs text-muted-foreground">{t("clientShares.otherRental")}</p> : null}
              {groups.history.length ? <section className="grid gap-2"><h3 className="text-sm font-semibold">{t("clientShares.history")}</h3>{groups.history.map(s => <Button key={s.id} variant="outline" className="min-h-11 justify-between whitespace-normal" onPress={() => showRental(s.id, s.seatId)}><span>#{s.seatPosition}</span><span>{t("clientShares.settlement")}</span></Button>)}</section> : null}
              {!groups.mine.length && !groups.available.length && !groups.occupied.length && !groups.history.length && !detailLoading ? <p className="text-sm text-muted-foreground">{t("clientShares.empty")}</p> : null}
              {detail?.nextCursor ? <Button variant="outline" isDisabled={detailLoading} onPress={() => setDetailPages(n => n + 1)}>{t("clientShares.more")}</Button> : null}
            </> : null}
          </Modal.Body>
          <Modal.Footer className="flex-wrap">
            {sub ? <Button variant="outline" onPress={() => { const next = { shareId: shareId! }; setSelection(next); replaceSelection(next); }}>{t("clientShares.previous")}</Button> : null}
            <Button variant="ghost" onPress={close}>{t("common.close")}</Button>
            <Button variant="outline" isDisabled={detailLoading} onPress={retry}>{t("common.reload")}</Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
    {checkout.dialog}{rentals.dialog}
    <MarketAccessDialog key={access?.seat.id ?? "none"} open={!!access && current()} product="share" ownerEmail={access?.listing.ownerEmail ?? ""} buyerEmail={session?.user?.email ?? ""} contacts={access?.listing.contacts} targetKind="share_seat" targetId={access?.seat.id ?? ""} eligibility={access?.eligibility ?? { allowed: false, status: "access_required" }} onOpenChange={value => !value && setAccess(null)} onOpenChat={() => access ? chat.openClientChat(access.listing.installationId) : undefined} onRequestChanged={changed} />
  </MarketContext.Provider>;
}

export function ClientShareMarketEntryButton({ shareId }: { shareId: string }) {
  const context = React.useContext(MarketContext);
  const register = context?.register;
  const { t } = useLocaleText();
  React.useEffect(() => register?.(shareId), [register, shareId]);
  const entry = context?.entries.get(shareId);
  if (!entry || entry.entryKind === "none") return null;
  const Icon = entry.entryKind === "manage" ? Settings2 : ShoppingCart;
  return <button type="button" data-no-row-drawer className="inline-flex min-h-9 items-center gap-1.5 rounded-md px-3 text-xs font-semibold text-primary hover:bg-primary/5 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" onClick={event => { event.stopPropagation(); context?.open({ shareId }); }}><Icon className="size-3.5" />{t(clientShareEntryLabel(entry.entryKind))}</button>;
}

export function useClientShareRecoveryCount(knownShareIds: string[]) {
  const context = React.useContext(MarketContext);
  return context?.recovery.filter(entry => !knownShareIds.includes(entry.shareId)).length ?? 0;
}

export function ClientShareRecoveryCards({ knownShareIds }: { knownShareIds: string[] }) {
  const context = React.useContext(MarketContext);
  const { t } = useLocaleText();
  if (!context) return null;
  const missing = context.recovery.filter(entry => !knownShareIds.includes(entry.shareId));
  if (!missing.length && !context.recoveryError && !context.summaryError && !context.more) return null;
  return <section className="grid gap-3" aria-label={t("clientShares.recovery")}>
    {context.recoveryError || context.summaryError ? <div role="alert" className="flex flex-wrap items-center justify-between gap-2 text-sm text-danger"><span>{t("clientShares.contextError")}</span><Button size="sm" variant="outline" onPress={context.retry}>{t("common.retry")}</Button></div> : null}
    <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">{missing.map(entry => <article key={entry.shareId} className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-lg border border-border bg-background p-4"><div className="min-w-0"><p className="break-all text-sm font-medium">{entry.subdomain || entry.shareId}</p><p className="mt-1 text-xs text-muted-foreground">{t(entry.resourceState === "missing" ? "clientShares.missing" : entry.resourceState === "listed" ? "clientShares.recovery" : "clientShares.unlisted")}</p></div><Button variant="outline" size="sm" onPress={() => context.open({ shareId: entry.shareId })}>{t(clientShareEntryLabel(entry.entryKind))}</Button></article>)}</div>
    {context.more ? <Button variant="outline" isDisabled={context.loading} onPress={context.loadMore}>{context.loading ? t("common.loading") : t("clientShares.more")}</Button> : null}
  </section>;
}
