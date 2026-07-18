import type { ClientChatMessage, ClientChatRoom, DashboardClient } from "@/lib/types";

export const ANON_VISITS_KEY = "cc_switch_router_chat_anon_visits_v1";
export const DRAFT_PREFIX = "cc_switch_router_chat_draft_v1:";
export const ROOM_POLL_MS = 30_000;
export const LIST_POLL_MS = 20_000;
export const CLOSED_POLL_MS = 30_000;
export const MAX_BODY_LENGTH = 1_000;

export type AnonymousVisit = { installationId: string; lastReadSeq: number; lastOpenedAt: string };
export type LocalChatMessage = ClientChatMessage & {
  __pending?: boolean;
  __failed?: boolean;
  clientMessageId?: string;
};

export function readAnonymousVisits(): AnonymousVisit[] {
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

export function writeAnonymousVisits(visits: AnonymousVisit[]) {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(ANON_VISITS_KEY, JSON.stringify(visits.slice(0, 100)));
  } catch {
    // Optional persistence.
  }
}

export function upsertAnonymousVisit(installationId: string, lastReadSeq?: number) {
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

export function initialsForLabel(label: string) {
  const trimmed = label.trim();
  if (!trimmed) return "?";
  const parts = trimmed.split(/[\s@._-]+/u).filter(Boolean);
  if (parts.length === 0) return trimmed.charAt(0).toUpperCase();
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase();
  return (parts[0].charAt(0) + parts[1].charAt(0)).toUpperCase();
}

export function maskEmail(email?: string | null) {
  const value = String(email || "").trim();
  if (!value || !value.includes("@")) return "-";
  const [local, domain] = value.split("@");
  if (!local || !domain) return value;
  const visible = local.slice(0, Math.min(2, local.length));
  return `${visible}***@${domain}`;
}

export function sortRooms(rooms: ClientChatRoom[]) {
  return [...rooms].sort((left, right) => {
    const unreadDelta = right.unreadCount - left.unreadCount;
    if (unreadDelta !== 0) return unreadDelta;
    return Date.parse(right.lastMessageAt || "") - Date.parse(left.lastMessageAt || "");
  });
}

export function unreadByInstallationMap(rooms: ClientChatRoom[]) {
  const map = new Map<string, number>();
  for (const room of rooms) map.set(room.installationId, room.unreadCount);
  return map;
}

export function findDashboardClient(clients: DashboardClient[] | undefined, installationId: string) {
  return clients?.find((client) => client.installation.id === installationId);
}

export function clientLabelForInstallation(clients: DashboardClient[] | undefined, installationId: string) {
  const client = findDashboardClient(clients, installationId);
  return client?.clientTunnel?.subdomain || client?.installation.id || installationId;
}

const URL_RE = /(https?:\/\/[^\s<>"']+)/gi;

export function renderMessageBody(body: string) {
  const nodes: Array<string | { key: string; href: string; text: string }> = [];
  let cursor = 0;
  let match: RegExpExecArray | null;
  URL_RE.lastIndex = 0;
  while ((match = URL_RE.exec(body)) !== null) {
    if (match.index > cursor) nodes.push(body.slice(cursor, match.index));
    nodes.push({ key: `${match.index}-${match[0]}`, href: match[0], text: match[0] });
    cursor = match.index + match[0].length;
  }
  if (cursor < body.length) nodes.push(body.slice(cursor));
  return nodes;
}

export function mergeMessages(current: LocalChatMessage[], incoming: ClientChatMessage[]) {
  const byId = new Map(current.map((message) => [message.id, message]));
  for (const message of incoming) byId.set(message.id, message);
  return [...byId.values()].sort((left, right) => left.seq - right.seq);
}

export function chatRoomUrl(installationId: string) {
  const url = new URL(window.location.href);
  url.pathname = "/clients/";
  url.search = "";
  url.searchParams.set("chat", installationId);
  return url.toString();
}
