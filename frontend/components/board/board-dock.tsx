"use client";

import { Loader2, MessageSquare, Pin, Send, Sparkles, X } from "lucide-react";
import { AlertDialog, Avatar, Badge, Button, Card, Chip, Input, ScrollShadow, Tabs, TextArea, toast } from "@heroui/react";
import * as React from "react";
import { getBoardMessages, getBoardMetaWithSignal, postBoardMessage, setBoardFeature, setBoardPin, deleteBoardMessage } from "@/lib/api";
import type { BoardMessage, BoardMeta } from "@/lib/types";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { formatRelativeTime } from "@/lib/utils";

const GUEST_NAME_KEY = "cc_switch_router_board_guest_name_v1";
const DRAFT_KEY = "cc_switch_router_board_draft_v1";
const UNREAD_KEY = "cc_switch_router_board_last_seen_total_v1";
const POLL_LIST_MS = 7000;
const POLL_META_MS = 30000;
const MAX_LEN_FALLBACK = 1000;
const SCROLL_NEAR_BOTTOM_PX = 64;

// HeroUI dialog defaults render dark surfaces / light text; force the project's light
// palette so headings/body stay legible. Same trick as drawerDialogClassName in data-tables.
const DIALOG_CLASS =
  "light !bg-white !text-slate-900 " +
  "[--foreground:rgb(var(--router-foreground))] [--muted:rgb(var(--router-muted-foreground))] " +
  "[--overlay:#fff] [--overlay-foreground:rgb(var(--router-foreground))] " +
  "[--surface:#fff] [--surface-foreground:rgb(var(--router-foreground))] " +
  "[--surface-secondary:rgb(var(--router-muted))] [--surface-secondary-foreground:rgb(var(--router-foreground))] " +
  "[--default:rgb(var(--router-muted))] [--default-foreground:rgb(var(--router-foreground))]";

type Feedback = { kind: "ok" | "err"; text: string } | null;
type AdminAction = "pin" | "unpin" | "feature" | "unfeature" | "delete";
type LocalMessage = BoardMessage & { __pending?: boolean };

function readLastSeenTotal(): number {
  if (typeof window === "undefined") return 0;
  const raw = window.localStorage.getItem(UNREAD_KEY);
  const num = raw ? Number.parseInt(raw, 10) : 0;
  return Number.isFinite(num) ? num : 0;
}

function initialsFor(label: string): string {
  const trimmed = label.trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s@._-]+/u).filter(Boolean);
  if (parts.length === 0) return trimmed.charAt(0).toUpperCase();
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0].charAt(0) + parts[1].charAt(0)).toUpperCase();
}

const URL_RE = /(https?:\/\/[^\s<>"']+)/gi;

function renderBody(body: string): React.ReactNode {
  const out: React.ReactNode[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null;
  URL_RE.lastIndex = 0;
  while ((match = URL_RE.exec(body)) !== null) {
    if (match.index > lastIndex) out.push(body.slice(lastIndex, match.index));
    const url = match[0];
    out.push(
      <a
        key={`${match.index}-${url}`}
        href={url}
        target="_blank"
        rel="noopener noreferrer nofollow"
        className="text-accent underline decoration-accent/40 underline-offset-2 hover:decoration-accent"
      >
        {url}
      </a>,
    );
    lastIndex = match.index + url.length;
  }
  if (lastIndex < body.length) out.push(body.slice(lastIndex));
  return out.length ? out : body;
}

// Mirrors the server's ORDER BY: pinned > featured > rest, each group desc by timestamp.
function sortBoardMessages<T extends BoardMessage>(items: T[]): T[] {
  const score = (m: BoardMessage) => (m.pinned ? 3 : m.featured ? 2 : 1);
  const stamp = (m: BoardMessage) =>
    m.pinned ? m.pinnedAt || m.createdAt : m.featured ? m.featuredAt || m.createdAt : m.createdAt;
  return [...items].sort((a, b) => {
    const da = score(b) - score(a);
    if (da !== 0) return da;
    const ka = stamp(a);
    const kb = stamp(b);
    return kb.localeCompare(ka);
  });
}

export function BoardDock() {
  const { session } = useAuth();
  const { locale, t } = useLocaleText();
  const dockRef = React.useRef<HTMLDivElement | null>(null);
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const tabRef = React.useRef("all");
  const lastAsOfRef = React.useRef<string | null>(null);
  const listAbortRef = React.useRef<AbortController | null>(null);
  const metaAbortRef = React.useRef<AbortController | null>(null);

  const [open, setOpen] = React.useState(false);
  const [tab, setTab] = React.useState("all");
  const [meta, setMeta] = React.useState<BoardMeta | null>(null);
  const [messages, setMessages] = React.useState<LocalMessage[]>([]);
  const [body, setBody] = React.useState("");
  const [guestName, setGuestName] = React.useState("");
  const [feedback, setFeedback] = React.useState<Feedback>(null);
  const [busy, setBusy] = React.useState(false);
  const [pendingAction, setPendingAction] = React.useState<{ id: string; action: AdminAction } | null>(null);
  const [confirmDelete, setConfirmDelete] = React.useState<BoardMessage | null>(null);
  const [lastSeenTotal, setLastSeenTotal] = React.useState(0);

  const maxLen = meta?.maxBodyLength || MAX_LEN_FALLBACK;
  const counterTone =
    body.length >= maxLen ? "text-destructive" : body.length >= Math.floor(maxLen * 0.9) ? "text-warning" : "text-muted-foreground";
  const unread = Math.max(0, (meta?.total ?? 0) - lastSeenTotal);

  React.useEffect(() => {
    setGuestName(localStorage.getItem(GUEST_NAME_KEY) || "");
    setBody(localStorage.getItem(DRAFT_KEY) || "");
    setLastSeenTotal(readLastSeenTotal());
  }, []);

  React.useEffect(() => {
    tabRef.current = tab;
  }, [tab]);

  React.useEffect(() => {
    const id = window.setTimeout(() => {
      try {
        if (body) localStorage.setItem(DRAFT_KEY, body);
        else localStorage.removeItem(DRAFT_KEY);
      } catch {
        /* ignore */
      }
    }, 200);
    return () => window.clearTimeout(id);
  }, [body]);

  const fetchMeta = React.useCallback(async () => {
    metaAbortRef.current?.abort();
    const controller = new AbortController();
    metaAbortRef.current = controller;
    try {
      const next = await getBoardMetaWithSignal(controller.signal);
      if (controller.signal.aborted) return;
      setMeta(next);
    } catch (err) {
      if (controller.signal.aborted) return;
      console.error(err);
    }
  }, []);

  const fetchList = React.useCallback(async (opts: { reset?: boolean } = {}) => {
    listAbortRef.current?.abort();
    const controller = new AbortController();
    listAbortRef.current = controller;
    const scroller = scrollRef.current;
    const wasNearBottom =
      scroller != null &&
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < SCROLL_NEAR_BOTTOM_PX;
    const prevTop = scroller?.scrollTop ?? 0;
    const since = opts.reset ? undefined : lastAsOfRef.current ?? undefined;
    try {
      const response = await getBoardMessages(tabRef.current, since, controller.signal);
      if (controller.signal.aborted) return;
      lastAsOfRef.current = response.asOf;
      const incoming = (response.messages || []) as LocalMessage[];
      const removed = new Set(response.removedIds || []);
      setMessages((prev) => {
        const pendings = prev.filter((m) => m.__pending);
        if (!response.incremental) {
          return sortBoardMessages([...pendings, ...incoming]);
        }
        const map = new Map<string, LocalMessage>();
        for (const m of prev) {
          if (m.__pending) continue;
          if (removed.has(m.id)) continue;
          map.set(m.id, m);
        }
        for (const m of incoming) map.set(m.id, m);
        return [...pendings, ...sortBoardMessages([...map.values()])];
      });
      requestAnimationFrame(() => {
        const el = scrollRef.current;
        if (!el) return;
        if (wasNearBottom) el.scrollTop = el.scrollHeight;
        else el.scrollTop = prevTop;
      });
    } catch (err) {
      if (controller.signal.aborted) return;
      console.error(err);
    }
  }, []);

  // Meta poll: always running (drives unread badge when closed).
  React.useEffect(() => {
    fetchMeta();
    const id = window.setInterval(fetchMeta, POLL_META_MS);
    return () => window.clearInterval(id);
  }, [fetchMeta]);

  // Reset cache + lastAsOf and do a full fetch whenever the panel opens or the tab changes.
  React.useEffect(() => {
    if (!open) return;
    setMessages((prev) => prev.filter((m) => m.__pending));
    lastAsOfRef.current = null;
    fetchList({ reset: true });
  }, [open, tab, fetchList]);

  // Polling tick — only while open; each tick is incremental against lastAsOfRef.
  React.useEffect(() => {
    if (!open) return;
    const id = window.setInterval(() => fetchList(), POLL_LIST_MS);
    return () => window.clearInterval(id);
  }, [open, fetchList]);

  // Mark all messages as seen when the panel opens after meta is known.
  React.useEffect(() => {
    if (!open || !meta) return;
    setLastSeenTotal(meta.total);
    try { localStorage.setItem(UNREAD_KEY, String(meta.total)); } catch { /* ignore */ }
  }, [open, meta]);

  React.useEffect(() => {
    if (!open) return;
    function handlePointerDown(event: PointerEvent) {
      // Skip outside-click handling while a portal-mounted dialog is open — its DOM
      // sits outside dockRef and would otherwise be treated as "outside".
      if (confirmDelete) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (dockRef.current?.contains(target)) return;
      // react-aria portals overlays under <body>; bail if the click is inside any of them.
      if (target.closest("[data-rac]") || target.closest("[role='dialog']") || target.closest("[role='alertdialog']")) return;
      setOpen(false);
    }
    function handleKey(event: KeyboardEvent) {
      if (event.key === "Escape") {
        if (confirmDelete) return; // let the dialog handle its own escape
        setOpen(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKey);
    };
  }, [open, confirmDelete]);

  const previousFocusRef = React.useRef<HTMLElement | null>(null);
  React.useEffect(() => {
    if (open) {
      previousFocusRef.current = (document.activeElement as HTMLElement | null) ?? null;
      requestAnimationFrame(() => textareaRef.current?.focus());
    } else {
      previousFocusRef.current?.focus?.();
      previousFocusRef.current = null;
    }
  }, [open]);

  function buildOptimistic(trimmed: string): LocalMessage {
    const fallbackLabel = session?.user?.email || guestName.trim() || t("board.guest");
    return {
      id: `temp-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
      body: trimmed,
      authorKind: session?.authenticated ? "user" : "guest",
      authorLabel: fallbackLabel,
      isMine: true,
      pinned: false,
      featured: false,
      createdAt: new Date().toISOString(),
      __pending: true,
    };
  }

  async function send() {
    const trimmed = body.trim();
    if (!trimmed) return;
    if (trimmed.length > maxLen) {
      setFeedback({ kind: "err", text: t("board.overLimit", { count: maxLen }) });
      return;
    }
    const optimistic = buildOptimistic(trimmed);
    setBusy(true);
    setFeedback(null);
    setMessages((prev) => [optimistic, ...prev]);
    setBody("");
    try { localStorage.removeItem(DRAFT_KEY); } catch { /* ignore */ }
    try {
      if (!session?.authenticated && guestName.trim()) localStorage.setItem(GUEST_NAME_KEY, guestName.trim());
      await postBoardMessage(trimmed, session?.authenticated ? undefined : guestName.trim());
      // Drop the optimistic immediately — the upcoming fetchList will bring the canonical row.
      setMessages((prev) => prev.filter((m) => m.id !== optimistic.id));
      setFeedback({ kind: "ok", text: t("board.sent") });
      await Promise.all([fetchList(), fetchMeta()]);
      window.setTimeout(() => setFeedback(null), 1600);
    } catch (err) {
      setMessages((prev) => prev.filter((m) => m.id !== optimistic.id));
      setBody(trimmed);
      setFeedback({ kind: "err", text: err instanceof Error ? err.message : String(err) });
    } finally {
      setBusy(false);
    }
  }

  async function runAdminAction(message: BoardMessage, action: AdminAction, fn: () => Promise<unknown>) {
    setPendingAction({ id: message.id, action });
    try {
      await fn();
      await Promise.all([fetchList(), fetchMeta()]);
      toast.success(t(`board.actionDone.${action}` as const));
    } catch (err) {
      toast.danger(err instanceof Error ? err.message : String(err));
    } finally {
      setPendingAction((prev) => (prev?.id === message.id && prev?.action === action ? null : prev));
    }
  }

  function handleTextareaKey(event: React.KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      send();
    }
  }

  if (!open) {
    const fabButton = (
      <button
        type="button"
        onClick={() => setOpen(true)}
        aria-label={t("board.open")}
        className="group flex h-14 w-14 items-center justify-center rounded-full text-accent-foreground shadow-[0_8px_24px_rgba(0,82,255,0.35)] transition-all duration-200 ease-out hover:-translate-y-0.5 hover:shadow-[0_14px_32px_rgba(0,82,255,0.45)] active:scale-[0.97] gradient-accent"
      >
        <MessageSquare className="h-6 w-6 transition-transform duration-200 group-hover:scale-110" />
      </button>
    );
    return (
      <div className="fixed bottom-5 right-5 z-40">
        {unread > 0 ? (
          <Badge color="danger" aria-label={t("board.unread", { count: unread })}>
            <Badge.Anchor className="block">{fabButton}</Badge.Anchor>
            <Badge.Label>{unread > 99 ? "99+" : unread}</Badge.Label>
          </Badge>
        ) : (
          fabButton
        )}
      </div>
    );
  }

  const tabCount = (key: string) => {
    if (!meta) return 0;
    if (key === "pinned") return meta.pinnedCount;
    if (key === "featured") return meta.featuredCount;
    return meta.total;
  };

  const isActionPending = (id: string, action: AdminAction) =>
    pendingAction?.id === id && pendingAction?.action === action;

  return (
    <>
      <Card
        ref={dockRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby="board-dock-title"
        className="fixed bottom-5 right-5 z-40 flex h-[min(640px,calc(100vh-2rem))] w-[min(420px,calc(100vw-2rem))] flex-col gap-0 overflow-hidden rounded-2xl border border-[rgba(0,82,255,0.18)] bg-card p-0 shadow-xl"
      >
        <Card.Header className="flex-row items-center justify-between gap-3 border-b p-4">
          <h2 id="board-dock-title" className="font-display text-xl leading-none text-foreground">{t("board.title")}</h2>
          <Button
            variant="ghost"
            isIconOnly
            className="rounded-full"
            onClick={(event) => {
              event.stopPropagation();
              setOpen(false);
            }}
            aria-label={t("board.close")}
          >
            <X className="h-4 w-4" />
          </Button>
        </Card.Header>
        <Card.Content className="min-h-0 gap-0 p-0">
          <div className="border-b p-3">
            <Tabs selectedKey={tab} onSelectionChange={(key: React.Key) => setTab(String(key))} variant="secondary" className="text-foreground">
              <Tabs.List className="grid w-full grid-cols-3 text-foreground">
                {(["all", "pinned", "featured"] as const).map((key) => (
                  <Tabs.Tab
                    key={key}
                    id={key}
                    className="flex items-center justify-center gap-2 text-muted-foreground data-[selected=true]:text-foreground"
                  >
                    <span>{t(`board.${key}` as const)}</span>
                    <span className="rounded-full bg-muted px-1.5 py-0.5 font-mono text-[0.65rem] tabular-nums text-muted-foreground/80">
                      {tabCount(key)}
                    </span>
                  </Tabs.Tab>
                ))}
              </Tabs.List>
            </Tabs>
          </div>
          <ScrollShadow ref={scrollRef as React.Ref<HTMLDivElement>} className="min-h-0 flex-1 p-4">
            <div className="grid gap-3 pr-1">
              {messages.length ? (
                messages.map((message) => (
                  <article
                    key={message.id}
                    className={`group relative overflow-hidden rounded-2xl border bg-card p-3 shadow-sm transition-all duration-200 hover:-translate-y-0.5 hover:shadow-md ${
                      message.pinned ? "border-[rgba(0,82,255,0.22)] bg-[rgba(0,82,255,0.04)]" : ""
                    } ${message.__pending ? "opacity-60" : ""}`}
                  >
                    {message.isMine ? (
                      <span
                        aria-hidden
                        className="absolute inset-y-0 left-0 w-1 rounded-r"
                        style={{ background: "linear-gradient(to bottom, rgb(var(--router-accent)), rgb(var(--router-accent-secondary)))" }}
                      />
                    ) : null}
                    <div className="flex flex-wrap items-center gap-2">
                      <Avatar
                        size="sm"
                        className={
                          message.authorKind === "admin"
                            ? "ring-2 ring-accent/50"
                            : "ring-1 ring-border"
                        }
                      >
                        <Avatar.Fallback>{initialsFor(message.authorLabel || t("board.guest"))}</Avatar.Fallback>
                      </Avatar>
                      <span className="font-medium">
                        {message.authorLabel || t("board.guest")}
                        {message.isMine ? (
                          <span className="ml-1.5 rounded-full bg-muted px-1.5 py-0.5 align-middle font-mono text-[0.6rem] uppercase tracking-wider text-muted-foreground">
                            {t("board.you")}
                          </span>
                        ) : null}
                      </span>
                      {message.authorKind === "admin" ? (
                        <Chip size="sm" variant="soft" className="!bg-[rgba(0,82,255,0.1)] !text-accent">
                          {t("board.admin")}
                        </Chip>
                      ) : null}
                      {message.pinned ? (
                        <Chip color="warning" size="sm" variant="soft" className="gap-1">
                          <Pin className="h-3 w-3" />
                          {t("board.pinned")}
                        </Chip>
                      ) : null}
                      {message.featured && !message.pinned ? (
                        <Chip size="sm" variant="soft" className="gap-1 !bg-[rgba(0,82,255,0.08)] !text-accent">
                          <Sparkles className="h-3 w-3" />
                          {t("board.featured")}
                        </Chip>
                      ) : null}
                      <span className="ml-auto flex items-center gap-1 text-xs text-muted-foreground" title={message.createdAt}>
                        {message.__pending ? <Loader2 className="h-3 w-3 animate-spin" /> : null}
                        {formatRelativeTime(message.createdAt, locale)}
                      </span>
                    </div>
                    <p className="mt-2 whitespace-pre-wrap break-words text-sm leading-6">{renderBody(message.body)}</p>
                    {!message.__pending && (meta?.canPostAsAdmin || (message.isMine && message.authorKind === "guest")) ? (
                      <div className="mt-3 flex flex-wrap gap-2">
                        {meta?.canPostAsAdmin ? (
                          <>
                            <Button
                              variant="outline"
                              size="sm"
                              isDisabled={isActionPending(message.id, message.pinned ? "unpin" : "pin")}
                              onClick={() =>
                                runAdminAction(message, message.pinned ? "unpin" : "pin", () =>
                                  setBoardPin(message.id, !message.pinned),
                                )
                              }
                            >
                              {isActionPending(message.id, message.pinned ? "unpin" : "pin") ? (
                                <Loader2 className="h-3 w-3 animate-spin" />
                              ) : null}
                              {message.pinned ? t("board.unpin") : t("board.pin")}
                            </Button>
                            <Button
                              variant="outline"
                              size="sm"
                              isDisabled={isActionPending(message.id, message.featured ? "unfeature" : "feature")}
                              onClick={() =>
                                runAdminAction(message, message.featured ? "unfeature" : "feature", () =>
                                  setBoardFeature(message.id, !message.featured),
                                )
                              }
                            >
                              {isActionPending(message.id, message.featured ? "unfeature" : "feature") ? (
                                <Loader2 className="h-3 w-3 animate-spin" />
                              ) : null}
                              {message.featured ? t("board.unfeature") : t("board.feature")}
                            </Button>
                          </>
                        ) : null}
                        <Button
                          variant="ghost"
                          size="sm"
                          className="text-destructive hover:!bg-destructive/10"
                          onClick={() => setConfirmDelete(message)}
                        >
                          {t("common.delete")}
                        </Button>
                      </div>
                    ) : null}
                  </article>
                ))
              ) : (
                <div className="flex flex-col items-center gap-3 rounded-2xl border border-dashed border-[rgba(0,82,255,0.18)] bg-[rgba(0,82,255,0.02)] py-10 text-center">
                  <div className="flex h-12 w-12 items-center justify-center rounded-full bg-[rgba(0,82,255,0.08)] text-accent">
                    <MessageSquare className="h-5 w-5" />
                  </div>
                  <div className="flex flex-col gap-1">
                    <p className="font-display text-lg leading-none text-foreground">{t("board.empty.headline")}</p>
                    <p className="text-xs text-muted-foreground">{t("board.empty.hint")}</p>
                  </div>
                </div>
              )}
            </div>
          </ScrollShadow>
          <div className="grid gap-2 border-t bg-muted/30 p-3">
            {!session?.authenticated ? (
              <Input value={guestName} onChange={(event) => setGuestName(event.target.value)} placeholder={t("board.guestName")} />
            ) : null}
            <TextArea
              ref={textareaRef as React.Ref<HTMLTextAreaElement>}
              value={body}
              onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setBody(event.target.value)}
              onKeyDown={handleTextareaKey}
              placeholder={t("board.write")}
              maxLength={maxLen}
            />
            <div className="flex items-center justify-between gap-3">
              <div className="flex min-w-0 items-center gap-3">
                <span className={`text-xs tabular-nums ${counterTone}`}>{body.length}/{maxLen}</span>
                <span aria-live="polite" className={`truncate text-xs ${feedback?.kind === "err" ? "text-destructive" : "text-emerald-600"}`}>
                  {feedback?.text}
                </span>
              </div>
              <div className="flex items-center gap-2">
                <span className="hidden font-mono text-[0.65rem] uppercase tracking-wider text-muted-foreground/70 sm:inline">
                  {t("board.sendHint")}
                </span>
                <Button
                  onClick={send}
                  isDisabled={busy || !body.trim()}
                  size="sm"
                  className="group gradient-accent !text-accent-foreground hover:brightness-110"
                >
                  {busy ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : (
                    <Send className="h-4 w-4 transition-transform duration-150 group-hover:translate-x-0.5" />
                  )}
                  {t("board.send")}
                </Button>
              </div>
            </div>
          </div>
        </Card.Content>
      </Card>
      <AlertDialog isOpen={!!confirmDelete} onOpenChange={(next) => !next && setConfirmDelete(null)}>
        <AlertDialog.Backdrop>
          <AlertDialog.Container>
            <AlertDialog.Dialog className={DIALOG_CLASS}>
              <AlertDialog.Header>
                <AlertDialog.Heading className="!text-slate-900">{t("board.confirmDeleteTitle")}</AlertDialog.Heading>
              </AlertDialog.Header>
              <AlertDialog.Body>
                <p className="text-sm !text-slate-600">{t("board.confirmDeleteBody")}</p>
                {confirmDelete ? (
                  <p className="mt-3 max-h-32 overflow-y-auto whitespace-pre-wrap break-words rounded-md border border-slate-200 bg-slate-50 p-3 text-sm !text-slate-900">
                    {confirmDelete.body}
                  </p>
                ) : null}
              </AlertDialog.Body>
              <AlertDialog.Footer>
                <Button variant="ghost" className="!text-slate-700 hover:!bg-slate-100" onClick={() => setConfirmDelete(null)}>
                  {t("board.cancel")}
                </Button>
                <Button
                  className="!bg-destructive !text-destructive-foreground hover:brightness-110"
                  isDisabled={confirmDelete ? isActionPending(confirmDelete.id, "delete") : false}
                  onClick={() => {
                    if (!confirmDelete) return;
                    const target = confirmDelete;
                    runAdminAction(target, "delete", () => deleteBoardMessage(target.id)).finally(() =>
                      setConfirmDelete(null),
                    );
                  }}
                >
                  {confirmDelete && isActionPending(confirmDelete.id, "delete") ? (
                    <Loader2 className="h-4 w-4 animate-spin" />
                  ) : null}
                  {t("common.delete")}
                </Button>
              </AlertDialog.Footer>
            </AlertDialog.Dialog>
          </AlertDialog.Container>
        </AlertDialog.Backdrop>
      </AlertDialog>
    </>
  );
}
