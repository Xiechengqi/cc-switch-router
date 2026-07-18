"use client";

import { Badge, Button } from "@heroui/react";
import { Loader2, MessageCircle, X } from "lucide-react";
import * as React from "react";
import {
  CLOSED_POLL_MS,
  LIST_POLL_MS,
  clearRecentChatLocalCache,
  findDashboardClient,
  readAnonymousVisits,
  sortRooms,
  unreadByInstallationMap,
  upsertAnonymousVisit,
} from "@/components/chat/client-chat-helpers";
import { ClientChatRoomHeader } from "@/components/chat/client-chat-room-header";
import { ClientChatRoomList } from "@/components/chat/client-chat-room-list";
import { ClientChatRoomPanel } from "@/components/chat/client-chat-room-panel";
import { useAuth } from "@/components/auth/auth-provider";
import { useDashboardData } from "@/components/dashboard/dashboard-data";
import {
  CONSOLE_DOCK_RESERVED_HEIGHT,
  useClientConsole,
} from "@/components/dashboard/client-console/client-console-manager";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getClientChatMeta,
  getClientChatRoom,
  getVisitedClientChatRooms,
  importClientChatVisits,
  lookupClientChatRooms,
  removeClientChatVisit,
  recordClientChatVisit,
} from "@/lib/api";
import type { ClientChatRoom } from "@/lib/types";

const CHAT_DOCK_BASE_BOTTOM = 80;
const CHAT_DOCK_CONSOLE_GAP = 12;

type ClientChatContextValue = {
  openChat: (installationId: string) => Promise<void>;
  unreadByInstallation: Map<string, number>;
};

const ClientChatContext = React.createContext<ClientChatContextValue | null>(null);

function useChatDockBottom() {
  const { dockVisible } = useClientConsole();
  return dockVisible ? CONSOLE_DOCK_RESERVED_HEIGHT + CHAT_DOCK_CONSOLE_GAP : CHAT_DOCK_BASE_BOTTOM;
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
  const roomsLoadedRef = React.useRef(false);

  const unreadByInstallation = React.useMemo(() => unreadByInstallationMap(rooms), [rooms]);

  const loadRooms = React.useCallback(
    async (signal?: AbortSignal) => {
      if (session?.authenticated) {
        const response = await getVisitedClientChatRooms(signal);
        setRooms(sortRooms(response.rooms));
        setTotalUnread(response.totalUnread);
        roomsLoadedRef.current = true;
        return;
      }
      const visits = readAnonymousVisits();
      if (!visits.length) {
        setRooms([]);
        setTotalUnread(0);
        roomsLoadedRef.current = true;
        return;
      }
      const response = await lookupClientChatRooms(visits, signal);
      const nextRooms = sortRooms(response.rooms);
      setRooms(nextRooms);
      setTotalUnread(nextRooms.reduce((sum, room) => sum + room.unreadCount, 0));
      roomsLoadedRef.current = true;
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

  const backToList = React.useCallback(() => {
    setSelectedRoom(null);
    const url = new URL(window.location.href);
    url.searchParams.delete("chat");
    window.history.replaceState(window.history.state, "", url);
  }, []);

  const refreshRooms = React.useCallback(() => {
    void loadRooms().catch(console.error);
  }, [loadRooms]);

  const removeFromRecent = React.useCallback(
    async (room: ClientChatRoom) => {
      clearRecentChatLocalCache(room);
      setRooms((current) => current.filter((item) => item.id !== room.id));
      setTotalUnread((current) => Math.max(0, current - room.unreadCount));
      if (selectedRoom?.id === room.id) {
        backToList();
      }
      if (openedDeepLinkRef.current === room.installationId) {
        openedDeepLinkRef.current = null;
      }
      try {
        if (session?.authenticated) {
          await removeClientChatVisit(room.id);
        }
      } catch (cause) {
        console.error(cause);
        void loadRooms().catch(console.error);
      }
    },
    [backToList, loadRooms, selectedRoom?.id, session?.authenticated],
  );

  const minimize = React.useCallback(() => {
    setOpen(false);
  }, []);

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

  const handleFabClick = React.useCallback(() => {
    if (selectedRoom) {
      setOpen(true);
      return;
    }
    setListLoading(!roomsLoadedRef.current);
    openList();
    if (!roomsLoadedRef.current) {
      void loadRooms()
        .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
        .finally(() => setListLoading(false));
    } else {
      setListLoading(false);
    }
  }, [loadRooms, openList, selectedRoom]);

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
        localStorage.removeItem("cc_switch_router_chat_anon_visits_v1");
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

  const context = React.useMemo(
    () => ({ openChat, unreadByInstallation }),
    [openChat, unreadByInstallation],
  );

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
        onFabClick={handleFabClick}
        onSelectRoom={(room) => void openChat(room.installationId)}
        onOpenInstallation={(installationId) => void openChat(installationId)}
        onRoomChange={setSelectedRoom}
        onRoomsRefresh={refreshRooms}
        onBackToList={backToList}
        onMinimize={minimize}
        onClose={close}
        onRemoveRecent={(room) => void removeFromRecent(room)}
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
  onFabClick,
  onSelectRoom,
  onOpenInstallation,
  onRoomChange,
  onRoomsRefresh,
  onBackToList,
  onMinimize,
  onClose,
  onRemoveRecent,
}: {
  open: boolean;
  opening: boolean;
  rooms: ClientChatRoom[];
  totalUnread: number;
  selectedRoom: ClientChatRoom | null;
  listLoading: boolean;
  error: string | null;
  onFabClick: () => void;
  onSelectRoom: (room: ClientChatRoom) => void;
  onOpenInstallation: (installationId: string) => void;
  onRoomChange: (room: ClientChatRoom) => void;
  onRoomsRefresh: () => void;
  onBackToList: () => void;
  onMinimize: () => void;
  onClose: () => void;
  onRemoveRecent: (room: ClientChatRoom) => void;
}) {
  const { t } = useLocaleText();
  const { data } = useDashboardData();
  const dockBottom = useChatDockBottom();
  const panelRef = React.useRef<HTMLDivElement | null>(null);
  const previousFocusRef = React.useRef<HTMLElement | null>(null);
  const selectedClient = selectedRoom
    ? findDashboardClient(data?.clients, selectedRoom.installationId)
    : undefined;

  React.useEffect(() => {
    if (!open) return;
    previousFocusRef.current = document.activeElement as HTMLElement | null;
    const handlePointer = (event: PointerEvent) => {
      if (!(event.target instanceof Element) || panelRef.current?.contains(event.target)) return;
      if (event.target.closest("[role='dialog']") || event.target.closest("[data-rac]")) return;
      if (event.target.closest("[data-client-chat-dock]")) return;
      onMinimize();
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
  }, [onClose, onMinimize, open]);

  const dockStyle = { bottom: dockBottom };

  if (!open) {
    const button = (
      <button
        type="button"
        onClick={onFabClick}
        aria-label={t("chat.open")}
        className="flex h-11 w-11 items-center justify-center rounded-full border border-white/60 bg-white/85 text-slate-600 shadow-lg backdrop-blur-md transition hover:border-primary/30 hover:text-primary"
      >
        {opening ? <Loader2 className="h-[18px] w-[18px] animate-spin" /> : <MessageCircle className="h-[18px] w-[18px]" />}
      </button>
    );
    return (
      <div className="fixed right-5 z-40" style={dockStyle} data-client-chat-dock>
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
      className="fixed right-5 z-40 flex h-[min(640px,calc(100vh-6rem))] w-[min(420px,calc(100vw-2rem))] flex-col overflow-hidden rounded-lg border border-slate-200 bg-white shadow-2xl"
      style={dockStyle}
      data-client-chat-dock
    >
      {selectedRoom ? (
        <ClientChatRoomHeader room={selectedRoom} client={selectedClient} onBack={onBackToList} onClose={onClose} />
      ) : (
        <header className="flex h-14 shrink-0 items-center gap-2 border-b border-slate-200 px-3">
          <div className="min-w-0 flex-1">
            <h2 id="client-chat-title" className="truncate text-sm font-semibold text-slate-900">
              {t("chat.title")}
            </h2>
            <p className="truncate text-[11px] text-slate-500">{t("chat.subtitle")}</p>
          </div>
          <Button isIconOnly variant="ghost" size="sm" className="rounded-md" onClick={onClose} aria-label={t("chat.close")}>
            <X className="h-4 w-4" />
          </Button>
        </header>
      )}
      {selectedRoom ? (
        <ClientChatRoomPanel room={selectedRoom} onRoomUpdate={onRoomChange} onRoomsRefresh={onRoomsRefresh} />
      ) : (
        <ClientChatRoomList
          rooms={rooms}
          clients={data?.clients || []}
          loading={listLoading || opening}
          error={error}
          onSelectRoom={onSelectRoom}
          onOpenInstallation={onOpenInstallation}
          onRemoveRecent={onRemoveRecent}
        />
      )}
    </div>
  );
}
