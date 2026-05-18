import { authFetch } from "@/lib/auth";
import type {
  BoardListResponse,
  BoardMessage,
  BoardMeta,
  DashboardResponse,
  SettingsSchema,
  SettingsUpdateResponse,
  SettingsValuesResponse,
  ShareSettingsPatch,
  ShareEditView,
  VersionResponse,
} from "@/lib/types";

export type { BoardListResponse, BoardMessage, BoardMeta };

export async function parseJson<T>(response: Response): Promise<T> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(data?.message || `HTTP ${response.status}`);
  }
  return data as T;
}

export async function getDashboard() {
  return parseJson<DashboardResponse>(await authFetch("/v1/dashboard", { cache: "no-store" }));
}

export async function updateShareSettings(shareId: string, patch: ShareSettingsPatch) {
  return parseJson<{ ok: boolean; edit: ShareEditView }>(
    await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/settings`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ patch }),
    }),
  );
}

export async function getSettingsSchema() {
  return parseJson<SettingsSchema>(await authFetch("/v1/admin/settings/schema", { cache: "no-store" }));
}

export async function getSettingsValues() {
  return parseJson<SettingsValuesResponse>(await authFetch("/v1/admin/settings/values", { cache: "no-store" }));
}

export async function saveSettings(updates: Record<string, string | null | boolean>) {
  return parseJson<SettingsUpdateResponse>(
    await authFetch("/v1/admin/settings/values", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ updates }),
    }),
  );
}

export async function getVersion() {
  return parseJson<VersionResponse>(await authFetch("/v1/admin/version", { cache: "no-store" }));
}

export async function restartService() {
  return parseJson<{ ok: boolean; strategy: string }>(
    await authFetch("/v1/admin/restart", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    }),
  );
}

export async function startUpgrade() {
  return parseJson<{ taskId: string }>(await authFetch("/v1/admin/upgrade", { method: "POST" }));
}

export async function testTelegram() {
  return parseJson<{ ok: boolean }>(await authFetch("/v1/admin/telegram/test", { method: "POST" }));
}

const BOARD_GUEST_KEY = "cc_switch_router_board_guest_v1";

export function boardGuestId() {
  let id = localStorage.getItem(BOARD_GUEST_KEY);
  if (id && /^[a-z0-9-]{8,80}$/i.test(id)) return id;
  id = crypto.randomUUID ? crypto.randomUUID() : `guest-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  localStorage.setItem(BOARD_GUEST_KEY, id);
  return id;
}

export async function boardFetch(input: RequestInfo | URL, init: RequestInit = {}) {
  const headers = new Headers(init.headers || {});
  headers.set("X-Board-Guest-Id", boardGuestId());
  if (init.body && !headers.has("Content-Type")) headers.set("Content-Type", "application/json");
  return authFetch(input, { ...init, headers });
}

export async function getBoardMeta() {
  return parseJson<BoardMeta>(await boardFetch("/v1/board/meta", { cache: "no-store" }));
}

export async function getBoardMessages(tab = "all", since?: string, signal?: AbortSignal) {
  const params = new URLSearchParams({ tab, limit: "50" });
  if (since) params.set("since", since);
  return parseJson<BoardListResponse>(await boardFetch(`/v1/board/messages?${params}`, { cache: "no-store", signal }));
}

export async function getBoardMetaWithSignal(signal?: AbortSignal) {
  return parseJson<BoardMeta>(await boardFetch("/v1/board/meta", { cache: "no-store", signal }));
}

export async function postBoardMessage(body: string, guestName?: string) {
  return parseJson<BoardMessage>(
    await boardFetch("/v1/board/messages", {
      method: "POST",
      body: JSON.stringify({ body, guestName: guestName || undefined }),
    }),
  );
}

export async function setBoardPin(id: string, value: boolean) {
  return parseJson<unknown>(
    await boardFetch(`/v1/board/messages/${encodeURIComponent(id)}/pin`, {
      method: "POST",
      body: JSON.stringify({ value }),
    }),
  );
}

export async function setBoardFeature(id: string, value: boolean) {
  return parseJson<unknown>(
    await boardFetch(`/v1/board/messages/${encodeURIComponent(id)}/feature`, {
      method: "POST",
      body: JSON.stringify({ value }),
    }),
  );
}

export async function deleteBoardMessage(id: string) {
  return parseJson<unknown>(
    await boardFetch(`/v1/board/messages/${encodeURIComponent(id)}`, {
      method: "DELETE",
    }),
  );
}
