"use client";

import { Avatar, ScrollShadow, Tabs } from "@heroui/react";
import { ChevronDown, ChevronUp, Loader2, MessageCircle } from "lucide-react";
import Link from "next/link";
import * as React from "react";
import { initialsForLabel } from "@/components/chat/client-chat-helpers";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { MessageKey } from "@/lib/i18n";
import { DASHBOARD_CLIENTS_PATH } from "@/lib/dashboard-nav";
import type { ClientChatRoom, DashboardClient } from "@/lib/types";
import { cn, formatDateTime, formatRelativeTime } from "@/lib/utils";

function RoomListRow({
  room,
  locale,
  t,
  onSelect,
}: {
  room: ClientChatRoom;
  locale: string;
  t: (key: MessageKey, values?: Record<string, string | number>) => string;
  onSelect: () => void;
}) {
  const author = room.lastMessage?.authorLabel || "?";
  return (
    <button
      type="button"
      onClick={onSelect}
      className={cn(
        "grid w-full grid-cols-[auto_minmax(0,1fr)_auto] gap-3 px-4 py-3 text-left transition hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/30",
        room.status === "archived" && "opacity-70",
      )}
    >
      <Avatar size="sm" className="bg-slate-100 text-[10px] font-semibold text-slate-600">
        <Avatar.Fallback>{initialsForLabel(room.clientLabel)}</Avatar.Fallback>
      </Avatar>
      <span className="min-w-0">
        <span className="flex items-center gap-2">
          <strong className="truncate text-sm font-medium text-slate-900">{room.clientLabel}</strong>
          {room.status === "archived" ? (
            <span className="shrink-0 text-[10px] font-medium uppercase text-amber-700">{t("chat.archived")}</span>
          ) : null}
        </span>
        <span className="mt-1 block truncate text-xs text-slate-500">
          {room.lastMessage ? `${author}: ${room.lastMessage.body || t("chat.deleted")}` : t("chat.noMessages")}
        </span>
      </span>
      <span className="flex min-w-10 flex-col items-end gap-1 text-[10px] text-slate-400">
        {room.lastMessageAt ? (
          <span title={formatDateTime(room.lastMessageAt)}>{formatRelativeTime(room.lastMessageAt, locale)}</span>
        ) : (
          ""
        )}
        {room.unreadCount > 0 ? (
          <span className="rounded-full bg-primary px-1.5 py-0.5 font-semibold text-white">
            {room.unreadCount > 99 ? "99+" : room.unreadCount}
          </span>
        ) : null}
      </span>
    </button>
  );
}

function EmptyChatState({ t }: { t: (key: MessageKey, values?: Record<string, string | number>) => string }) {
  return (
    <div className="flex min-h-72 flex-col items-center justify-center gap-3 px-8 text-center text-slate-500">
      <MessageCircle className="h-7 w-7 text-slate-300" />
      <p className="text-sm font-medium text-slate-700">{t("chat.empty")}</p>
      <p className="text-xs leading-5 text-slate-500">{t("chat.emptyHint")}</p>
      <Link
        href={DASHBOARD_CLIENTS_PATH}
        className="inline-flex h-8 items-center rounded-md border border-slate-200 bg-white px-3 text-xs font-medium text-slate-700 transition hover:border-slate-300 hover:bg-slate-50"
      >
        {t("chat.emptyAction")}
      </Link>
    </div>
  );
}

export function ClientChatRoomList({
  rooms,
  clients,
  loading,
  error,
  onSelectRoom,
  onOpenInstallation,
}: {
  rooms: ClientChatRoom[];
  clients: DashboardClient[];
  loading: boolean;
  error: string | null;
  onSelectRoom: (room: ClientChatRoom) => void;
  onOpenInstallation: (installationId: string) => void;
}) {
  const { t, locale } = useLocaleText();
  const [tab, setTab] = React.useState<"recent" | "all">("recent");
  const [showArchived, setShowArchived] = React.useState(false);

  const chatClients = React.useMemo(
    () =>
      clients
        .filter((client) => client.chatAvailable)
        .map((client) => ({
          installationId: client.installation.id,
          label: client.clientTunnel?.subdomain || client.installation.id,
          region: client.installation.countryCode || client.installation.region || "",
        }))
        .sort((left, right) => left.label.localeCompare(right.label)),
    [clients],
  );

  const activeRooms = rooms.filter((room) => room.status !== "archived");
  const archivedRooms = rooms.filter((room) => room.status === "archived");
  const visibleRooms = showArchived ? rooms : activeRooms;

  if (loading && !rooms.length && tab === "recent") {
    return (
      <div className="flex flex-1 items-center justify-center">
        <Loader2 className="h-5 w-5 animate-spin text-slate-400" />
      </div>
    );
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <Tabs selectedKey={tab} onSelectionChange={(key) => setTab(key as "recent" | "all")} className="px-3 pt-2">
        <Tabs.ListContainer>
          <Tabs.List aria-label={t("chat.title")}>
            <Tabs.Tab id="recent">{t("chat.tabRecent")}</Tabs.Tab>
            <Tabs.Tab id="all">{t("chat.tabAll")}</Tabs.Tab>
          </Tabs.List>
        </Tabs.ListContainer>
      </Tabs>
      <ScrollShadow className="min-h-0 flex-1">
        {error ? <div className="m-3 rounded-md border border-red-200 bg-red-50 p-3 text-sm text-red-700">{error}</div> : null}
        {tab === "recent" ? (
          <>
            {visibleRooms.length ? (
              <div className="divide-y divide-slate-100">
                {visibleRooms.map((room) => (
                  <RoomListRow key={room.id} room={room} locale={locale} t={t} onSelect={() => onSelectRoom(room)} />
                ))}
              </div>
            ) : (
              <EmptyChatState t={t} />
            )}
            {archivedRooms.length ? (
              <div className="border-t border-slate-100 px-4 py-2">
                <button
                  type="button"
                  className="inline-flex items-center gap-1 text-xs font-medium text-slate-500 hover:text-slate-700"
                  onClick={() => setShowArchived((value) => !value)}
                >
                  {showArchived ? <ChevronUp className="h-3.5 w-3.5" /> : <ChevronDown className="h-3.5 w-3.5" />}
                  {showArchived ? t("chat.hideArchived") : t("chat.showArchived", { count: archivedRooms.length })}
                </button>
              </div>
            ) : null}
          </>
        ) : (
          <div className="divide-y divide-slate-100">
            {chatClients.map((client) => (
              <button
                key={client.installationId}
                type="button"
                onClick={() => onOpenInstallation(client.installationId)}
                className="grid w-full grid-cols-[auto_minmax(0,1fr)] gap-3 px-4 py-3 text-left transition hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-primary/30"
              >
                <Avatar size="sm" className="bg-slate-100 text-[10px] font-semibold text-slate-600">
                  <Avatar.Fallback>{initialsForLabel(client.label)}</Avatar.Fallback>
                </Avatar>
                <span className="min-w-0">
                  <strong className="block truncate text-sm font-medium text-slate-900">{client.label}</strong>
                  <span className="mt-0.5 block truncate text-xs text-slate-500">{client.region || t("chat.public")}</span>
                </span>
              </button>
            ))}
            {!chatClients.length ? <EmptyChatState t={t} /> : null}
          </div>
        )}
      </ScrollShadow>
    </div>
  );
}
