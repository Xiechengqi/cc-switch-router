import { authFetch } from "@/lib/auth";
import type {
  BoardListResponse,
  BoardMessage,
  BoardMeta,
  DashboardResponse,
  MarketShare,
  ClearMetricsResponse,
  SettingsSchema,
  SettingsUpdateResponse,
  SettingsValuesResponse,
  ShareSettingsPatch,
  ShareEditView,
  UserApiTokenResponse,
  UserApiTokenResetResponse,
  VersionResponse,
  MetricsSnapshot,
  HostMetricsInfo,
  HostMetricsStatus,
  MetricsSeriesResponse,
  LlmMetricsSnapshot,
  LlmTopResponse,
  MetricEvent,
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

export async function getUserApiToken() {
  return parseJson<UserApiTokenResponse>(await authFetch("/v1/me/api-token", { cache: "no-store" }));
}

export async function resetUserApiToken() {
  return parseJson<UserApiTokenResetResponse>(
    await authFetch("/v1/me/api-token/reset", { method: "POST" }),
  );
}

export async function getMarketLinkedShares(marketEmail: string) {
  return parseJson<MarketShare[]>(
    await authFetch(`/v1/admin/markets/${encodeURIComponent(marketEmail)}/linked-shares`, {
      cache: "no-store",
    }),
  );
}

export async function updateMarketDisabledShares(marketEmail: string, disabledShareIds: string[]) {
  return parseJson<{ ok: boolean; disabledShareIds: string[] }>(
    await authFetch(`/v1/admin/markets/${encodeURIComponent(marketEmail)}/disabled-shares`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ disabledShareIds }),
    }),
  );
}

export async function updateMarketMaintenance(
  marketEmail: string,
  input: { maintenanceEnabled: boolean; maintenanceMessage?: string | null },
) {
  return parseJson<{ ok: boolean; maintenanceEnabled: boolean; maintenanceMessage?: string }>(
    await authFetch(`/v1/admin/markets/${encodeURIComponent(marketEmail)}/maintenance`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
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

export async function rollbackService() {
  return parseJson<{ ok: boolean; strategy: string; backupPath: string }>(
    await authFetch("/v1/admin/rollback", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    }),
  );
}

export async function startUpgrade() {
  return parseJson<{ taskId: string }>(await authFetch("/v1/admin/upgrade", { method: "POST" }));
}

export async function downloadRouterLog() {
  const response = await authFetch("/v1/admin/logs/router/download", { cache: "no-store" });
  if (!response.ok) {
    const data = await response.json().catch(() => ({}));
    throw new Error(data?.message || `HTTP ${response.status}`);
  }
  const blob = await response.blob();
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = "cc-switch-router.log";
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
  URL.revokeObjectURL(url);
}

export async function testTelegram() {
  return parseJson<{ ok: boolean }>(await authFetch("/v1/admin/telegram/test", { method: "POST" }));
}

export async function getMetricsSnapshot() {
  return parseJson<MetricsSnapshot>(await authFetch("/v1/admin/metrics/snapshot", { cache: "no-store" }));
}

export async function getMetricsHostInfo() {
  return parseJson<HostMetricsInfo>(await authFetch("/v1/admin/metrics/host/info", { cache: "no-store" }));
}

export async function getMetricsHostStatus() {
  return parseJson<HostMetricsStatus>(await authFetch("/v1/admin/metrics/host/status", { cache: "no-store" }));
}

export async function getMetricsSeries(range: string, step?: string) {
  const params = new URLSearchParams({ range });
  if (step) params.set("step", step);
  return parseJson<MetricsSeriesResponse>(await authFetch(`/v1/admin/metrics/series?${params}`, { cache: "no-store" }));
}

export async function getLlmMetricsSnapshot(range = "5m") {
  const params = new URLSearchParams({ range });
  return parseJson<LlmMetricsSnapshot>(await authFetch(`/v1/admin/metrics/llm/snapshot?${params}`, { cache: "no-store" }));
}

export async function getLlmMetricsTop(range = "1h", by = "tokens") {
  const params = new URLSearchParams({ range, by });
  return parseJson<LlmTopResponse>(await authFetch(`/v1/admin/metrics/llm/top?${params}`, { cache: "no-store" }));
}

export async function getMetricEvents(limit = 100) {
  const params = new URLSearchParams({ limit: String(limit) });
  return parseJson<MetricEvent[]>(await authFetch(`/v1/admin/metrics/events?${params}`, { cache: "no-store" }));
}

export async function clearMetrics() {
  return parseJson<ClearMetricsResponse>(await authFetch("/v1/admin/metrics", { method: "DELETE" }));
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
