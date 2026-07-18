"use client";

import { Button, ScrollShadow, TextArea } from "@heroui/react";
import { Loader2, Send, Trash2 } from "lucide-react";
import * as React from "react";
import {
  DRAFT_PREFIX,
  LocalChatMessage,
  MAX_BODY_LENGTH,
  mergeMessages,
  upsertAnonymousVisit,
} from "@/components/chat/client-chat-helpers";
import { useAuth } from "@/components/auth/auth-provider";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  deleteClientChatMessage,
  getClientChatMessages,
  getClientChatRoom,
  markClientChatRead,
  postClientChatMessage,
} from "@/lib/api";
import type { ClientChatMessage, ClientChatRoom } from "@/lib/types";
import { cn, formatDateTime, formatRelativeTime } from "@/lib/utils";
import type { MessageKey } from "@/lib/i18n";

const ROOM_POLL_MS = 30_000;

function renderBodyNodes(body: string, mine: boolean) {
  const URL_RE = /(https?:\/\/[^\s<>"']+)/gi;
  const nodes: React.ReactNode[] = [];
  let cursor = 0;
  let match: RegExpExecArray | null;
  URL_RE.lastIndex = 0;
  while ((match = URL_RE.exec(body)) !== null) {
    if (match.index > cursor) nodes.push(body.slice(cursor, match.index));
    nodes.push(
      <a
        key={`${match.index}-${match[0]}`}
        href={match[0]}
        target="_blank"
        rel="noopener noreferrer nofollow"
        className={cn("underline underline-offset-2", mine ? "text-primary-foreground" : "text-primary")}
      >
        {match[0]}
      </a>,
    );
    cursor = match.index + match[0].length;
  }
  if (cursor < body.length) nodes.push(body.slice(cursor));
  return nodes;
}

function MessageBubble({
  message,
  locale,
  t,
  canDelete,
  onDelete,
  onRetry,
}: {
  message: LocalChatMessage;
  locale: string;
  t: (key: MessageKey, values?: Record<string, string | number>) => string;
  canDelete: boolean;
  onDelete: () => void;
  onRetry: () => void;
}) {
  const mine = message.isMine;
  return (
    <article className={cn("group flex min-w-0", mine ? "justify-end" : "justify-start")}>
      <div className={cn("max-w-[88%]", mine ? "items-end" : "items-start")}>
        <div
          className={cn(
            "mb-1 flex items-center gap-2 text-[10px] text-slate-400",
            mine ? "justify-end" : "justify-start",
          )}
        >
          <span title={formatDateTime(message.createdAt)}>{formatRelativeTime(message.createdAt, locale)}</span>
          <span className="font-medium text-slate-500">{mine ? t("chat.you") : message.authorLabel}</span>
          {canDelete ? (
            <button
              type="button"
              onClick={onDelete}
              className="rounded p-1 text-slate-400 opacity-0 transition hover:bg-red-50 hover:text-red-600 group-hover:opacity-100 focus:opacity-100"
              title={t("common.delete")}
              aria-label={t("common.delete")}
            >
              <Trash2 className="h-3.5 w-3.5" />
            </button>
          ) : null}
        </div>
        {message.status === "deleted" ? (
          <p className="text-sm italic text-slate-400">{t("chat.deleted")}</p>
        ) : (
          <div
            className={cn(
              "rounded-2xl px-3 py-2 text-sm leading-6 shadow-sm",
              mine ? "rounded-br-md bg-primary text-primary-foreground" : "rounded-bl-md border border-slate-200 bg-white text-slate-800",
              message.__failed && "border border-red-200 bg-red-50 text-red-800",
              message.__pending && "opacity-70",
            )}
          >
            <p className="whitespace-pre-wrap break-words">{renderBodyNodes(message.body, mine)}</p>
          </div>
        )}
        {message.__failed ? (
          <button type="button" className="mt-1 text-xs font-medium text-red-600 hover:underline" onClick={onRetry}>
            {t("chat.pendingFailed")}
          </button>
        ) : null}
      </div>
    </article>
  );
}

export function ClientChatRoomPanel({
  room,
  onRoomUpdate,
  onRoomsRefresh,
}: {
  room: ClientChatRoom;
  onRoomUpdate: (room: ClientChatRoom) => void;
  onRoomsRefresh: () => void;
}) {
  const { session } = useAuth();
  const { t, locale } = useLocaleText();
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const [messages, setMessages] = React.useState<LocalChatMessage[]>([]);
  const [body, setBody] = React.useState("");
  const [loading, setLoading] = React.useState(true);
  const [loadingOlder, setLoadingOlder] = React.useState(false);
  const [hasMore, setHasMore] = React.useState(false);
  const [sending, setSending] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = React.useState<LocalChatMessage | null>(null);
  const [deleting, setDeleting] = React.useState(false);
  const latestSeqRef = React.useRef(0);
  const activeRoomIdRef = React.useRef(room.id);
  const sseConnectedRef = React.useRef(false);
  activeRoomIdRef.current = room.id;
  const messageList = Array.isArray(messages) ? messages : [];

  const markRead = React.useCallback(
    async (latestSeq: number) => {
      if (latestSeq <= 0) return;
      if (session?.authenticated) await markClientChatRead(room.id, latestSeq);
      else upsertAnonymousVisit(room.installationId, latestSeq);
      onRoomsRefresh();
    },
    [onRoomsRefresh, room.id, room.installationId, session?.authenticated],
  );

  const pullAfter = React.useCallback(
    async (signal?: AbortSignal) => {
      const scroll = scrollRef.current;
      const nearBottom = !!scroll && scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 80;
      let cursor = latestSeqRef.current;
      let incoming: ClientChatMessage[] = [];
      for (let page = 0; page < 10; page += 1) {
        const response = await getClientChatMessages(room.id, {
          afterSeq: cursor,
          limit: 100,
          signal,
        });
        const pageMessages = Array.isArray(response.messages) ? response.messages : [];
        if (!pageMessages.length) break;
        incoming = mergeMessages(incoming, pageMessages);
        const nextCursor = pageMessages.at(-1)?.seq || cursor;
        if (nextCursor <= cursor) break;
        cursor = nextCursor;
        if (!response.hasMore) break;
      }
      if (!incoming.length || signal?.aborted) return;
      setMessages((current) => mergeMessages(current, incoming));
      latestSeqRef.current = Math.max(latestSeqRef.current, cursor);
      void markRead(cursor).catch(console.error);
      if (nearBottom) {
        requestAnimationFrame(() => {
          const node = scrollRef.current;
          if (node) node.scrollTop = node.scrollHeight;
        });
      }
    },
    [markRead, room.id],
  );

  React.useEffect(() => {
    try {
      setBody(localStorage.getItem(`${DRAFT_PREFIX}${room.id}`) || "");
    } catch {
      setBody("");
    }
    setMessages([]);
    setLoading(true);
    setError(null);
    const controller = new AbortController();
    void getClientChatMessages(room.id, { limit: 50, signal: controller.signal })
      .then((response) => {
        setMessages(Array.isArray(response.messages) ? response.messages : []);
        setHasMore(response.hasMore);
        latestSeqRef.current = response.latestSeq;
        requestAnimationFrame(() => {
          const node = scrollRef.current;
          if (node) node.scrollTop = node.scrollHeight;
          textareaRef.current?.focus();
        });
        return markRead(response.latestSeq);
      })
      .catch((cause) => {
        if (!controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => {
        if (!controller.signal.aborted) setLoading(false);
      });
    return () => controller.abort();
  }, [markRead, room.id]);

  React.useEffect(() => {
    const id = window.setTimeout(() => {
      try {
        if (body) localStorage.setItem(`${DRAFT_PREFIX}${room.id}`, body);
        else localStorage.removeItem(`${DRAFT_PREFIX}${room.id}`);
      } catch {
        // Optional.
      }
    }, 200);
    return () => window.clearTimeout(id);
  }, [body, room.id]);

  React.useEffect(() => {
    if (typeof window === "undefined" || typeof EventSource === "undefined") return;
    const source = new EventSource(
      `/v1/chat/rooms/${encodeURIComponent(room.id)}/stream?afterSeq=${latestSeqRef.current}`,
    );
    sseConnectedRef.current = true;
    const onUpdate = () => {
      void pullAfter().catch(console.error);
    };
    source.addEventListener("update", onUpdate);
    source.addEventListener("ready", onUpdate);
    source.onerror = () => {
      sseConnectedRef.current = false;
    };
    return () => {
      sseConnectedRef.current = false;
      source.removeEventListener("update", onUpdate);
      source.removeEventListener("ready", onUpdate);
      source.close();
    };
  }, [pullAfter, room.id]);

  React.useEffect(() => {
    const controller = new AbortController();
    let inFlight = false;
    let pollCount = 0;
    const poll = async () => {
      if (inFlight || controller.signal.aborted) return;
      inFlight = true;
      try {
        if (!sseConnectedRef.current) await pullAfter(controller.signal);
        pollCount += 1;
        if (pollCount % 4 === 0 && !controller.signal.aborted) {
          const latestRoom = await getClientChatRoom(room.installationId, controller.signal);
          if (!controller.signal.aborted) onRoomUpdate(latestRoom);
        }
      } catch (cause) {
        if (!controller.signal.aborted) console.error(cause);
      } finally {
        inFlight = false;
      }
    };
    const interval = window.setInterval(() => void poll(), ROOM_POLL_MS);
    return () => {
      controller.abort();
      window.clearInterval(interval);
    };
  }, [onRoomUpdate, pullAfter, room.id, room.installationId]);

  async function loadOlder() {
    const firstSeq = messageList.find((message) => !message.__pending)?.seq;
    if (!firstSeq || loadingOlder) return;
    const roomId = room.id;
    setLoadingOlder(true);
    const scroll = scrollRef.current;
    const previousHeight = scroll?.scrollHeight || 0;
    try {
      const response = await getClientChatMessages(roomId, { beforeSeq: firstSeq, limit: 50 });
      if (activeRoomIdRef.current !== roomId) return;
      setMessages((current) => mergeMessages(response.messages || [], current));
      setHasMore(response.hasMore);
      requestAnimationFrame(() => {
        const node = scrollRef.current;
        if (node) node.scrollTop += node.scrollHeight - previousHeight;
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoadingOlder(false);
    }
  }

  async function send(retryMessage?: LocalChatMessage) {
    const normalized = (retryMessage?.body || body).trim();
    if (!normalized || sending || room.status !== "active" || !session?.authenticated) return;
    if (Array.from(normalized).length > MAX_BODY_LENGTH) {
      setError(t("chat.overLimit", { count: MAX_BODY_LENGTH }));
      return;
    }
    setSending(true);
    setError(null);
    const roomId = room.id;
    const clientMessageId = retryMessage?.clientMessageId || crypto.randomUUID();
    const optimisticId = retryMessage?.id || `pending-${clientMessageId}`;
    const optimistic: LocalChatMessage = {
      id: optimisticId,
      seq: retryMessage?.seq || latestSeqRef.current + 1,
      body: normalized,
      authorLabel: session.user?.email?.split("@")[0] || t("chat.you"),
      isMine: true,
      status: "visible",
      createdAt: new Date().toISOString(),
      __pending: true,
      clientMessageId,
    };
    if (!retryMessage) {
      setMessages((current) => mergeMessages(current, [optimistic]));
      setBody("");
    } else {
      setMessages((current) =>
        current.map((message) =>
          message.id === optimisticId ? { ...message, __pending: true, __failed: false } : message,
        ),
      );
    }
    try {
      const message = await postClientChatMessage(roomId, normalized, clientMessageId);
      try {
        localStorage.removeItem(`${DRAFT_PREFIX}${roomId}`);
      } catch {
        // Optional.
      }
      if (activeRoomIdRef.current !== roomId) {
        onRoomsRefresh();
        return;
      }
      setMessages((current) =>
        mergeMessages(
          current.filter((item) => item.id !== optimisticId),
          [message],
        ),
      );
      latestSeqRef.current = Math.max(latestSeqRef.current, message.seq);
      await markRead(message.seq);
      requestAnimationFrame(() => {
        const node = scrollRef.current;
        if (node) node.scrollTop = node.scrollHeight;
        textareaRef.current?.focus();
      });
    } catch (cause) {
      setMessages((current) =>
        current.map((message) =>
          message.id === optimisticId ? { ...message, __pending: false, __failed: true } : message,
        ),
      );
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSending(false);
    }
  }

  async function confirmDelete() {
    if (!deleteTarget) return;
    const roomId = room.id;
    setDeleting(true);
    try {
      const deleted = await deleteClientChatMessage(deleteTarget.id);
      if (activeRoomIdRef.current !== roomId) return;
      setMessages((current) => mergeMessages(current, [deleted]));
      setDeleteTarget(null);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setDeleting(false);
    }
  }

  const bodyLength = Array.from(body).length;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ScrollShadow ref={scrollRef as React.Ref<HTMLDivElement>} className="min-h-0 flex-1 px-3 py-3">
        {hasMore ? (
          <div className="mb-3 flex justify-center">
            <Button size="sm" variant="ghost" onClick={() => void loadOlder()} isDisabled={loadingOlder}>
              {loadingOlder ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
              {t("chat.loadOlder")}
            </Button>
          </div>
        ) : null}
        {loading ? (
          <div className="flex min-h-40 items-center justify-center">
            <Loader2 className="h-5 w-5 animate-spin text-slate-400" />
          </div>
        ) : null}
        <div className="grid gap-3">
          {messageList.map((message) => (
            <MessageBubble
              key={message.id}
              message={message}
              locale={locale}
              t={t}
              canDelete={!!session?.isAdmin && message.status === "visible" && !message.__pending}
              onDelete={() => setDeleteTarget(message)}
              onRetry={() => void send(message)}
            />
          ))}
          {!loading && !messageList.length ? (
            <p className="py-16 text-center text-sm text-slate-400">{t("chat.noMessages")}</p>
          ) : null}
        </div>
      </ScrollShadow>
      <div className="shrink-0 border-t border-slate-200 bg-slate-50/70 p-3">
        {error ? <p className="mb-2 break-words text-xs text-red-600">{error}</p> : null}
        {room.status === "archived" ? (
          <p className="py-2 text-center text-xs text-slate-500">{t("chat.archivedReadOnly")}</p>
        ) : !session?.authenticated ? (
          <div className="grid gap-2">
            <p className="text-center text-xs leading-5 text-slate-500">{t("chat.loginHint")}</p>
            <Button className="w-full" variant="primary" onClick={() => window.dispatchEvent(new CustomEvent("router-open-login"))}>
              {t("chat.loginToSend")}
            </Button>
          </div>
        ) : (
          <div className="grid gap-2">
            <TextArea
              ref={textareaRef as React.Ref<HTMLTextAreaElement>}
              value={body}
              onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) => setBody(event.target.value)}
              onKeyDown={(event: React.KeyboardEvent<HTMLTextAreaElement>) => {
                if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {
                  event.preventDefault();
                  void send();
                }
              }}
              placeholder={t("chat.write")}
              className="min-h-20"
            />
            <div className="flex items-center justify-between gap-3">
              <span className={cn("text-xs tabular-nums", bodyLength > MAX_BODY_LENGTH ? "text-red-600" : "text-slate-400")}>
                {bodyLength}/{MAX_BODY_LENGTH}
              </span>
              <Button size="sm" variant="primary" onClick={() => void send()} isDisabled={sending || !body.trim() || bodyLength > MAX_BODY_LENGTH}>
                {sending ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
                {t("chat.send")}
              </Button>
            </div>
          </div>
        )}
      </div>
      <ConfirmAlertDialog
        open={!!deleteTarget}
        title={t("chat.confirmDeleteTitle")}
        description={t("chat.confirmDelete")}
        confirmLabel={t("common.delete")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        busy={deleting}
        onConfirm={() => void confirmDelete()}
        onOpenChange={(next) => !next && !deleting && setDeleteTarget(null)}
      />
    </div>
  );
}
