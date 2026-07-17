"use client";

import { Badge, Button, ScrollShadow, TextArea } from "@heroui/react";
import {
  ArrowLeft,
  Clock3,
  Globe2,
  Loader2,
  MessageCircle,
  Send,
  Trash2,
  X,
} from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  deleteClientChatMessage,
  getClientChatMessages,
  getClientChatMeta,
  getClientChatRoom,
  getVisitedClientChatRooms,
  importClientChatVisits,
  lookupClientChatRooms,
  markClientChatRead,
  postClientChatMessage,
  recordClientChatVisit,
} from "@/lib/api";
import type { ClientChatMessage, ClientChatRoom, ClientChatVisit } from "@/lib/types";

const ANON_VISITS_KEY = "cc_switch_router_chat_anon_visits_v1";
const DRAFT_PREFIX = "cc_switch_router_chat_draft_v1:";
const ROOM_POLL_MS = 5_000;
const LIST_POLL_MS = 20_000;
const CLOSED_POLL_MS = 30_000;
const MAX_BODY_LENGTH = 1_000;

type AnonymousVisit = ClientChatVisit & { lastOpenedAt: string };

type ClientChatContextValue = {
  openChat: (installationId: string) => Promise<void>;
};

const ClientChatContext = React.createContext<ClientChatContextValue | null>(null);

function readAnonymousVisits(): AnonymousVisit[] {
  if (typeof window === "undefined") return [];
  try {
    const parsed = JSON.parse(localStorage.getItem(ANON_VISITS_KEY) || "[]");
    if (!Array.isArray(parsed)) return [];
    return parsed
      .filter(
        (item): item is AnonymousVisit =>
          !!item &&
          typeof item.installationId === "string" &&
          typeof item.lastReadSeq === "number" &&
          typeof item.lastOpenedAt === "string",
      )
      .slice(0, 100);
  } catch {
    return [];
  }
}

function writeAnonymousVisits(visits: AnonymousVisit[]) {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(ANON_VISITS_KEY, JSON.stringify(visits.slice(0, 100)));
  } catch {
    // History remains readable when browser storage is unavailable.
  }
}

function upsertAnonymousVisit(installationId: string, lastReadSeq?: number) {
  const visits = readAnonymousVisits();
  const existing = visits.find((visit) => visit.installationId === installationId);
  const next: AnonymousVisit = {
    installationId,
    lastReadSeq: Math.max(existing?.lastReadSeq || 0, lastReadSeq || 0),
    lastOpenedAt: new Date().toISOString(),
  };
  writeAnonymousVisits([next, ...visits.filter((visit) => visit.installationId !== installationId)]);
  return next;
}

function messageTimestamp(value: string) {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return value;
  const parts = new Intl.DateTimeFormat("en-CA", {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  }).formatToParts(date);
  const part = (type: Intl.DateTimeFormatPartTypes) =>
    parts.find((item) => item.type === type)?.value || "";
  return `${part("year")}-${part("month")}-${part("day")} ${part("hour")}:${part("minute")}`;
}

const URL_RE = /(https?:\/\/[^\s<>"']+)/gi;

function renderMessageBody(body: string) {
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
        className="text-primary underline decoration-primary/40 underline-offset-2"
      >
        {match[0]}
      </a>,
    );
    cursor = match.index + match[0].length;
  }
  if (cursor < body.length) nodes.push(body.slice(cursor));
  return nodes.length ? nodes : body;
}

function mergeMessages(current: ClientChatMessage[], incoming: ClientChatMessage[]) {
  const safeCurrent = Array.isArray(current) ? current : [];
  const safeIncoming = Array.isArray(incoming) ? incoming : [];
  const byId = new Map(safeCurrent.map((message) => [message.id, message]));
  for (const message of safeIncoming) byId.set(message.id, message);
  return [...byId.values()].sort((left, right) => left.seq - right.seq);
}

export function ClientChatProvider({ children }: { children: React.ReactNode }) {
  const { session } = useAuth();
  const [open, setOpen] = React.useState(false);
  const [selectedRoom, setSelectedRoom] = React.useState<ClientChatRoom | null>(null);
  const [opening, setOpening] = React.useState(false);
  const [rooms, setRooms] = React.useState<ClientChatRoom[]>([]);
  const [totalUnread, setTotalUnread] = React.useState(0);
  const [listLoading, setListLoading] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const importedSessionRef = React.useRef<string | null>(null);
  const openRequestRef = React.useRef(0);
  const openedDeepLinkRef = React.useRef<string | null>(null);

  const loadRooms = React.useCallback(
    async (signal?: AbortSignal) => {
      if (session?.authenticated) {
        const response = await getVisitedClientChatRooms(signal);
        setRooms(response.rooms);
        setTotalUnread(response.totalUnread);
        return;
      }
      const visits = readAnonymousVisits();
      if (!visits.length) {
        setRooms([]);
        setTotalUnread(0);
        return;
      }
      const response = await lookupClientChatRooms(visits, signal);
      const nextRooms = response.rooms
        .sort(
          (left, right) =>
            Date.parse(right.lastMessageAt || "") - Date.parse(left.lastMessageAt || ""),
        );
      setRooms(nextRooms);
      setTotalUnread(nextRooms.reduce((sum, room) => sum + room.unreadCount, 0));
    },
    [session?.authenticated],
  );

  const loadUnread = React.useCallback(
    async (signal?: AbortSignal) => {
      if (session?.authenticated) {
        const meta = await getClientChatMeta(signal);
        setTotalUnread(meta.totalUnread);
      } else {
        await loadRooms(signal);
      }
    },
    [loadRooms, session?.authenticated],
  );

  const openChat = React.useCallback(
    async (installationId: string) => {
      openedDeepLinkRef.current = installationId;
      const requestId = ++openRequestRef.current;
      setOpening(true);
      setError(null);
      try {
        const room = await getClientChatRoom(installationId);
        if (openRequestRef.current !== requestId) return;
        if (session?.authenticated) await recordClientChatVisit(room.id);
        else upsertAnonymousVisit(installationId);
        if (openRequestRef.current !== requestId) return;
        setSelectedRoom(room);
        setOpen(true);
        const url = new URL(window.location.href);
        url.searchParams.set("chat", installationId);
        window.history.replaceState(window.history.state, "", url);
      } catch (cause) {
        if (openRequestRef.current !== requestId) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setSelectedRoom(null);
        setOpen(true);
      } finally {
        if (openRequestRef.current === requestId) setOpening(false);
      }
    },
    [session?.authenticated],
  );

  const openList = React.useCallback(() => {
    openRequestRef.current += 1;
    openedDeepLinkRef.current = null;
    setOpening(false);
    setSelectedRoom(null);
    setOpen(true);
    setError(null);
    const url = new URL(window.location.href);
    url.searchParams.delete("chat");
    window.history.replaceState(window.history.state, "", url);
  }, []);

  const refreshRooms = React.useCallback(() => {
    void loadRooms().catch(console.error);
  }, [loadRooms]);

  const close = React.useCallback(() => {
    openRequestRef.current += 1;
    openedDeepLinkRef.current = null;
    setOpening(false);
    setOpen(false);
    setSelectedRoom(null);
    const url = new URL(window.location.href);
    url.searchParams.delete("chat");
    window.history.replaceState(window.history.state, "", url);
  }, []);

  React.useEffect(() => {
    const installationId = new URL(window.location.href).searchParams.get("chat");
    if (installationId && openedDeepLinkRef.current !== installationId) {
      void openChat(installationId);
    }
  }, [openChat]);

  React.useEffect(() => {
    if (!session?.authenticated || !session.user?.id) {
      importedSessionRef.current = null;
      return;
    }
    if (importedSessionRef.current === session.user.id) return;
    importedSessionRef.current = session.user.id;
    const anonymous = readAnonymousVisits();
    if (!anonymous.length) return;
    void importClientChatVisits(
      anonymous.map(({ installationId, lastReadSeq }) => ({ installationId, lastReadSeq })),
    )
      .then(() => {
        localStorage.removeItem(ANON_VISITS_KEY);
        return loadRooms();
      })
      .catch(() => {
        importedSessionRef.current = null;
      });
  }, [loadRooms, session?.authenticated, session?.user?.id]);

  React.useEffect(() => {
    const controller = new AbortController();
    const load = open && !selectedRoom ? loadRooms : loadUnread;
    void load(controller.signal).catch((cause) => {
      if (!controller.signal.aborted) console.error(cause);
    });
    const interval = window.setInterval(
      () => void load().catch(console.error),
      open && !selectedRoom ? LIST_POLL_MS : CLOSED_POLL_MS,
    );
    return () => {
      controller.abort();
      window.clearInterval(interval);
    };
  }, [loadRooms, loadUnread, open, selectedRoom]);

  const context = React.useMemo(() => ({ openChat }), [openChat]);
  return (
    <ClientChatContext.Provider value={context}>
      {children}
      <ClientChatDock
        open={open}
        opening={opening}
        rooms={rooms}
        totalUnread={totalUnread}
        selectedRoom={selectedRoom}
        listLoading={listLoading}
        error={error}
        onOpenList={() => {
          setListLoading(true);
          openList();
          void loadRooms()
            .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
            .finally(() => setListLoading(false));
        }}
        onSelectRoom={(room) => void openChat(room.installationId)}
        onRoomChange={setSelectedRoom}
        onRoomsRefresh={refreshRooms}
        onClose={close}
      />
    </ClientChatContext.Provider>
  );
}

export function useClientChat() {
  const context = React.useContext(ClientChatContext);
  if (!context) throw new Error("useClientChat must be used inside ClientChatProvider");
  return context;
}

function ClientChatDock({
  open,
  opening,
  rooms,
  totalUnread,
  selectedRoom,
  listLoading,
  error,
  onOpenList,
  onSelectRoom,
  onRoomChange,
  onRoomsRefresh,
  onClose,
}: {
  open: boolean;
  opening: boolean;
  rooms: ClientChatRoom[];
  totalUnread: number;
  selectedRoom: ClientChatRoom | null;
  listLoading: boolean;
  error: string | null;
  onOpenList: () => void;
  onSelectRoom: (room: ClientChatRoom) => void;
  onRoomChange: (room: ClientChatRoom | null) => void;
  onRoomsRefresh: () => void;
  onClose: () => void;
}) {
  const { t } = useLocaleText();
  const panelRef = React.useRef<HTMLDivElement | null>(null);
  const previousFocusRef = React.useRef<HTMLElement | null>(null);

  React.useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    const handlePointer = (event: PointerEvent) => {
      if (!(event.target instanceof Element) || panelRef.current?.contains(event.target)) return;
      if (event.target.closest("[role='dialog']") || event.target.closest("[data-rac]")) return;
      onClose();
    };
    const handleKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    document.addEventListener("pointerdown", handlePointer);
    document.addEventListener("keydown", handleKey);
    return () => {
      document.removeEventListener("pointerdown", handlePointer);
      document.removeEventListener("keydown", handleKey);
      previousFocusRef.current?.focus?.();
      previousFocusRef.current = null;
    };
  }, [onClose, open]);

  if (!open) {
    const button = (
      <button
        type="button"
        onClick={onOpenList}
        aria-label={t("chat.open")}
        className="flex h-11 w-11 items-center justify-center rounded-full border border-white/60 bg-white/85 text-slate-600 shadow-lg backdrop-blur-md transition hover:border-primary/30 hover:text-primary"
      >
        {opening ? <Loader2 className="h-[18px] w-[18px] animate-spin" /> : <MessageCircle className="h-[18px] w-[18px]" />}
      </button>
    );
    return (
      <div className="fixed bottom-20 right-5 z-40" data-client-chat-dock>
        {totalUnread > 0 ? (
          <Badge color="danger" aria-label={t("chat.unread", { count: totalUnread })}>
            <Badge.Anchor>{button}</Badge.Anchor>
            <Badge.Label>{totalUnread > 99 ? "99+" : totalUnread}</Badge.Label>
          </Badge>
        ) : (
          button
        )}
      </div>
    );
  }

  return (
    <div
      ref={panelRef}
      role="dialog"
      aria-modal="true"
      aria-labelledby="client-chat-title"
      className="fixed bottom-20 right-5 z-40 flex h-[min(640px,calc(100vh-6rem))] w-[min(420px,calc(100vw-2rem))] flex-col overflow-hidden rounded-lg border border-slate-200 bg-white shadow-2xl"
      data-client-chat-dock
    >
      <header className="flex h-14 shrink-0 items-center gap-2 border-b border-slate-200 px-3">
        {selectedRoom ? (
          <Button
            isIconOnly
            variant="ghost"
            size="sm"
            className="rounded-md"
            onClick={() => {
              onRoomChange(null);
              onOpenList();
            }}
            aria-label={t("chat.back")}
          >
            <ArrowLeft className="h-4 w-4" />
          </Button>
        ) : null}
        <div className="min-w-0 flex-1">
          <h2 id="client-chat-title" className="truncate text-sm font-semibold text-slate-900">
            {selectedRoom?.clientLabel || t("chat.title")}
          </h2>
          {selectedRoom ? (
            <div className="mt-0.5 flex items-center gap-1 text-[11px] text-slate-500">
              {selectedRoom.status === "archived" ? <Clock3 className="h-3 w-3" /> : <Globe2 className="h-3 w-3" />}
              <span>{selectedRoom.status === "archived" ? t("chat.archived") : t("chat.public")}</span>
            </div>
          ) : null}
        </div>
        <Button
          isIconOnly
          variant="ghost"
          size="sm"
          className="rounded-md"
          onClick={onClose}
          aria-label={t("chat.close")}
        >
          <X className="h-4 w-4" />
        </Button>
      </header>
      {selectedRoom ? (
        <ClientChatRoomPanel
          room={selectedRoom}
          onRoomUpdate={onRoomChange}
          onRoomsRefresh={onRoomsRefresh}
        />
      ) : (
        <ClientChatRoomList
          rooms={rooms}
          loading={listLoading || opening}
          error={error}
          onSelectRoom={onSelectRoom}
        />
      )}
    </div>
  );
}

function ClientChatRoomList({
  rooms,
  loading,
  error,
  onSelectRoom,
}: {
  rooms: ClientChatRoom[];
  loading: boolean;
  error: string | null;
  onSelectRoom: (room: ClientChatRoom) => void;
}) {
  const { t } = useLocaleText();
  if (loading && !rooms.length) {
    return <div className="flex flex-1 items-center justify-center"><Loader2 className="h-5 w-5 animate-spin text-slate-400" /></div>;
  }
  return (
    <ScrollShadow className="min-h-0 flex-1">
      {error ? <div className="m-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700">{error}</div> : null}
      {rooms.length ? (
        <div className="divide-y divide-slate-100">
          {rooms.map((room) => (
            <button
              key={room.id}
              type="button"
              onClick={() => onSelectRoom(room)}
              className="grid w-full grid-cols-[minmax(0,1fr)_auto] gap-3 px-4 py-3 text-left transition hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/30"
            >
              <span className="min-w-0">
                <span className="flex items-center gap-2">
                  <strong className="truncate text-sm font-medium text-slate-900">{room.clientLabel}</strong>
                  {room.status === "archived" ? <span className="shrink-0 text-[10px] font-medium uppercase text-amber-700">{t("chat.archived")}</span> : null}
                </span>
                <span className="mt-1 block truncate text-xs text-slate-500">
                  {room.lastMessage ? `[${room.lastMessage.authorLabel}] ${room.lastMessage.body || t("chat.deleted")}` : t("chat.noMessages")}
                </span>
              </span>
              <span className="flex min-w-10 flex-col items-end gap-1 text-[10px] text-slate-400">
                {room.lastMessageAt ? messageTimestamp(room.lastMessageAt).slice(5) : ""}
                {room.unreadCount > 0 ? <span className="rounded-full bg-primary px-1.5 py-0.5 font-semibold text-white">{room.unreadCount > 99 ? "99+" : room.unreadCount}</span> : null}
              </span>
            </button>
          ))}
        </div>
      ) : (
        <div className="flex h-full min-h-72 flex-col items-center justify-center gap-3 px-8 text-center text-slate-500">
          <MessageCircle className="h-7 w-7 text-slate-300" />
          <p className="text-sm font-medium text-slate-700">{t("chat.empty")}</p>
        </div>
      )}
    </ScrollShadow>
  );
}

function ClientChatRoomPanel({
  room,
  onRoomUpdate,
  onRoomsRefresh,
}: {
  room: ClientChatRoom;
  onRoomUpdate: (room: ClientChatRoom) => void;
  onRoomsRefresh: () => void;
}) {
  const { session } = useAuth();
  const { t } = useLocaleText();
  const scrollRef = React.useRef<HTMLDivElement | null>(null);
  const textareaRef = React.useRef<HTMLTextAreaElement | null>(null);
  const [messages, setMessages] = React.useState<ClientChatMessage[]>([]);
  const [body, setBody] = React.useState("");
  const [loading, setLoading] = React.useState(true);
  const [loadingOlder, setLoadingOlder] = React.useState(false);
  const [hasMore, setHasMore] = React.useState(false);
  const [sending, setSending] = React.useState(false);
  const [error, setError] = React.useState<string | null>(null);
  const latestSeqRef = React.useRef(0);
  const activeRoomIdRef = React.useRef(room.id);
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
          const scroll = scrollRef.current;
          if (scroll) scroll.scrollTop = scroll.scrollHeight;
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
        // Draft persistence is optional.
      }
    }, 200);
    return () => window.clearTimeout(id);
  }, [body, room.id]);

  React.useEffect(() => {
    const controller = new AbortController();
    let inFlight = false;
    let pollCount = 0;
    const poll = async () => {
      if (inFlight || controller.signal.aborted) return;
      inFlight = true;
      try {
        const scroll = scrollRef.current;
        const nearBottom =
          !!scroll && scroll.scrollHeight - scroll.scrollTop - scroll.clientHeight < 80;
        let cursor = latestSeqRef.current;
        let incoming: ClientChatMessage[] = [];
        for (let page = 0; page < 10; page += 1) {
          const response = await getClientChatMessages(room.id, {
            afterSeq: cursor,
            limit: 100,
            signal: controller.signal,
          });
          const pageMessages = Array.isArray(response.messages) ? response.messages : [];
          if (!pageMessages.length) break;
          incoming = mergeMessages(incoming, pageMessages);
          const nextCursor = pageMessages.at(-1)?.seq || cursor;
          if (nextCursor <= cursor) break;
          cursor = nextCursor;
          if (!response.hasMore) break;
        }
        if (incoming.length && !controller.signal.aborted) {
          setMessages((current) => mergeMessages(current, incoming));
          latestSeqRef.current = Math.max(latestSeqRef.current, cursor);
          void markRead(cursor).catch(console.error);
          if (nearBottom) {
            requestAnimationFrame(() => {
              const current = scrollRef.current;
              if (current) current.scrollTop = current.scrollHeight;
            });
          }
        }
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
  }, [markRead, onRoomUpdate, room.id, room.installationId]);

  async function loadOlder() {
    const firstSeq = messageList[0]?.seq;
    if (!firstSeq || loadingOlder) return;
    const roomId = room.id;
    setLoadingOlder(true);
    const scroll = scrollRef.current;
    const previousHeight = scroll?.scrollHeight || 0;
    try {
      const response = await getClientChatMessages(roomId, { beforeSeq: firstSeq, limit: 50 });
      if (activeRoomIdRef.current !== roomId) return;
      const olderMessages = Array.isArray(response.messages) ? response.messages : [];
      setMessages((current) => mergeMessages(olderMessages, current));
      setHasMore(response.hasMore);
      requestAnimationFrame(() => {
        const current = scrollRef.current;
        if (current) current.scrollTop += current.scrollHeight - previousHeight;
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoadingOlder(false);
    }
  }

  async function send() {
    const normalized = body.trim();
    if (!normalized || sending || room.status !== "active" || !session?.authenticated) return;
    if (Array.from(normalized).length > MAX_BODY_LENGTH) {
      setError(t("chat.overLimit", { count: MAX_BODY_LENGTH }));
      return;
    }
    setSending(true);
    setError(null);
    const roomId = room.id;
    const clientMessageId = crypto.randomUUID();
    try {
      const message = await postClientChatMessage(roomId, normalized, clientMessageId);
      try {
        localStorage.removeItem(`${DRAFT_PREFIX}${roomId}`);
      } catch {
        // Draft persistence is optional.
      }
      if (activeRoomIdRef.current !== roomId) {
        onRoomsRefresh();
        return;
      }
      setMessages((current) => mergeMessages(current, [message]));
      latestSeqRef.current = Math.max(latestSeqRef.current, message.seq);
      setBody("");
      await markRead(message.seq);
      requestAnimationFrame(() => {
        const scroll = scrollRef.current;
        if (scroll) scroll.scrollTop = scroll.scrollHeight;
        textareaRef.current?.focus();
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setSending(false);
    }
  }

  async function removeMessage(message: ClientChatMessage) {
    if (!window.confirm(t("chat.confirmDelete"))) return;
    const roomId = room.id;
    try {
      const deleted = await deleteClientChatMessage(message.id);
      if (activeRoomIdRef.current !== roomId) return;
      setMessages((current) => mergeMessages(current, [deleted]));
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }

  const bodyLength = Array.from(body).length;

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <ScrollShadow ref={scrollRef as React.Ref<HTMLDivElement>} className="min-h-0 flex-1 px-4 py-3">
        {hasMore ? (
          <div className="mb-3 flex justify-center">
            <Button size="sm" variant="ghost" onClick={() => void loadOlder()} isDisabled={loadingOlder}>
              {loadingOlder ? <Loader2 className="h-3.5 w-3.5 animate-spin" /> : null}
              {t("chat.loadOlder")}
            </Button>
          </div>
        ) : null}
        {loading ? <div className="flex min-h-40 items-center justify-center"><Loader2 className="h-5 w-5 animate-spin text-slate-400" /></div> : null}
        <div className="grid gap-4">
          {messageList.map((message) => (
            <article key={message.id} className="group min-w-0">
              <div className="flex min-w-0 items-center gap-2 font-mono text-[11px] text-slate-500" title={message.createdAt}>
                <span className="shrink-0 whitespace-nowrap">[{messageTimestamp(message.createdAt)}]</span>
                <span className="min-w-0 truncate">[{message.authorLabel}]</span>
                {session?.isAdmin && message.status === "visible" ? (
                  <button
                    type="button"
                    onClick={() => void removeMessage(message)}
                    className="pointer-events-none ml-auto shrink-0 rounded p-1 text-slate-400 opacity-0 transition hover:bg-red-50 hover:text-red-600 group-hover:pointer-events-auto group-hover:opacity-100 focus:pointer-events-auto focus:opacity-100"
                    title={t("common.delete")}
                    aria-label={t("common.delete")}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </button>
                ) : null}
              </div>
              {message.status === "deleted" ? (
                <p className="mt-1 text-sm italic text-slate-400">{t("chat.deleted")}</p>
              ) : (
                <p className="mt-1 whitespace-pre-wrap break-words text-sm leading-6 text-slate-800">{renderMessageBody(message.body)}</p>
              )}
            </article>
          ))}
          {!loading && !messageList.length ? <p className="py-16 text-center text-sm text-slate-400">{t("chat.noMessages")}</p> : null}
        </div>
      </ScrollShadow>
      <div className="shrink-0 border-t border-slate-200 bg-slate-50/70 p-3">
        {error ? <p className="mb-2 break-words text-xs text-red-600">{error}</p> : null}
        {room.status === "archived" ? (
          <p className="py-2 text-center text-xs text-slate-500">{t("chat.archivedReadOnly")}</p>
        ) : !session?.authenticated ? (
          <Button
            className="w-full"
            variant="primary"
            onClick={() => window.dispatchEvent(new CustomEvent("router-open-login"))}
          >
            {t("chat.loginToSend")}
          </Button>
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
              <span className={`text-xs tabular-nums ${bodyLength > MAX_BODY_LENGTH ? "text-red-600" : "text-slate-400"}`}>
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
    </div>
  );
}
