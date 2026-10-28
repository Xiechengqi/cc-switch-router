import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { messages, type MessageKey } from "@/lib/i18n";
import { clientTerminationHref } from "@/lib/client-termination";

export type UserNotification = {
  id: string;
  category: "wallet" | "service" | "client" | "system";
  eventKind: string;
  objectKind: string;
  objectId: string;
  payload: Record<string, unknown>;
  readAt: string | null;
  createdAt: string;
};
export type NotificationFeed = {
  items: UserNotification[];
  nextCursor: string | null;
  unread: number;
  readThrough: string | null;
};

/** A malformed event must not take down the shared navigation. */
export function notificationFeed(value: unknown): NotificationFeed {
  if (!value || typeof value !== "object") throw new Error("Invalid notification feed");
  const page = value as Record<string, unknown>;
  if (!Array.isArray(page.items) || !Number.isSafeInteger(page.unread) || (page.unread as number) < 0) {
    throw new Error("Invalid notification feed");
  }
  const items = page.items.filter((item): item is UserNotification => !!item && typeof item === "object"
    && typeof item.id === "string" && ["wallet", "service", "client", "system"].includes(item.category)
    && typeof item.eventKind === "string" && typeof item.objectKind === "string" && typeof item.objectId === "string"
    && typeof item.createdAt === "string" && Number.isFinite(Date.parse(item.createdAt))
    && (item.readAt === null || typeof item.readAt === "string")
    && !!item.payload && typeof item.payload === "object" && !Array.isArray(item.payload));
  return { items, unread: page.unread as number,
    nextCursor: typeof page.nextCursor === "string" ? page.nextCursor : null,
    readThrough: typeof page.readThrough === "string" ? page.readThrough : null };
}

export async function getNotifications(cursor?: string) {
  return notificationFeed(await parseJson<unknown>(await authFetch(
    `/v1/notifications${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ""}`, { cache: "no-store" },
  )));
}

export async function markNotificationRead(id: string, through?: string) {
  return parseJson<{ unread: number; readAt: string }>(await authFetch(
    `/v1/notifications/${encodeURIComponent(id)}/read`, {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ through }),
    },
  ));
}

export function notificationKindKey(kind: string): MessageKey {
  const key = `notificationCenter.kind.${kind}`;
  if (key in messages.en) return key as MessageKey;
  const marketKey = `marketNotifications.kind.${kind}`;
  return (marketKey in messages.en ? marketKey : "notificationCenter.other") as MessageKey;
}

/** Links are constructed locally; never follow a URL from notification payloads. */
export function notificationHref(item: UserNotification): string {
  if (item.objectKind === "client_termination" || item.eventKind.startsWith("client_termination_")) {
    return clientTerminationHref(typeof item.payload.terminationId === "string" ? item.payload.terminationId : item.objectId);
  }
  if (item.category === "system") return "/metrics/?tab=alerts";
  if (item.category === "client") return `/clients/?focusKind=client&focusId=${encodeURIComponent(item.objectId)}&drawerKind=client&drawerId=${encodeURIComponent(item.objectId)}`;
  if (item.eventKind === "c2c_order_updated" && typeof item.payload.orderId === "string") {
    return `/account/wallet/?c2cOrder=${encodeURIComponent(item.payload.orderId)}`;
  }
  return "/account/wallet/";
}
