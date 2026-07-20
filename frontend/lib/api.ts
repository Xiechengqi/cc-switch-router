import { authFetch } from "@/lib/auth";
import type {
  BoardListResponse,
  BoardMessage,
  BoardMeta,
  DashboardResponse,
  MarketShare,
  ShareSessionLoad,
  ClearMetricsResponse,
  SettingsSchema,
  SettingsUpdateResponse,
  SettingsValuesResponse,
  ShareSettingsPatch,
  ShareEditView,
  ShareConnectionTestRequest,
  ShareConnectionTestResponse,
  ShareUsageRefreshRequest,
  ShareUsageRefreshResponse,
  ImageGenerationRequestLog,
  ShareUsageByEmailResponse,
  UserApiTokenResponse,
  UserApiTokenResetResponse,
  VersionResponse,
  MetricsSnapshot,
  HostMetricsInfo,
  HostMetricsStatus,
  MetricsSeriesResponse,
  LlmMetricsSnapshot,
  LlmTopResponse,
  LlmReliabilityResponse,
  MetricEvent,
  MapDisplaySettings,
  MapDisplaySettingsUpdate,
  AnnouncementSettings,
  AnnouncementSettingsUpdate,
  AnnouncementResponse,
  ClientNotificationDeliveriesResponse,
  ClientChatDeliveriesResponse,
  ClientChatMessage,
  ClientChatMessageListResponse,
  ClientChatRoom,
  ClientChatRoomListResponse,
  ClientChatVisit,
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

export async function getMapDisplay() {
  return parseJson<MapDisplaySettings>(await authFetch("/v1/map-display", { cache: "no-store" }));
}

export async function updateMapDisplay(update: MapDisplaySettingsUpdate) {
  return parseJson<MapDisplaySettings>(
    await authFetch("/v1/admin/map-display", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(update),
    }),
  );
}

export async function getAnnouncement() {
  const response = await fetch("/v1/announcement", { cache: "no-store" });
  return parseJson<AnnouncementResponse>(response);
}

export async function updateAnnouncement(update: AnnouncementSettingsUpdate) {
  return parseJson<AnnouncementSettings>(
    await authFetch("/v1/admin/announcement", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(update),
    }),
  );
}

export type DashboardUxEvent = {
  eventType: string;
  source?: string;
  targetType?: "request" | "client" | "share" | "market" | "country";
  stepCount?: number;
  elapsedMs?: number;
  keyboard?: boolean;
};

export function recordDashboardUxEvent(event: DashboardUxEvent) {
  return fetch("/v1/dashboard/ux-events", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(event),
    keepalive: true,
  }).catch(() => undefined);
}

export async function updateShareSettings(
  shareId: string,
  patch: ShareSettingsPatch,
  baseConfigRevision?: number,
) {
  return parseJson<{ ok: boolean; edit: ShareEditView; appliedSynchronously: boolean }>(
    await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/settings`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ patch, baseConfigRevision }),
    }),
  );
}

export async function getShareUsageByEmail(
  shareId: string,
  app: "claude" | "codex" | "gemini",
  period: "24h" | "1w" | "30d",
) {
  const params = new URLSearchParams({ app, period });
  return parseJson<ShareUsageByEmailResponse>(
    await fetch(`/v1/shares/${encodeURIComponent(shareId)}/usage-by-email?${params}`, {
      cache: "no-store",
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

export async function getMarketSharePriority(marketEmail: string, app?: string) {
  const query = app ? `?${new URLSearchParams({ app }).toString()}` : "";
  return parseJson<MarketShare[]>(
    await fetch(`/v1/markets/${encodeURIComponent(marketEmail)}/share-priority${query}`, {
      cache: "no-store",
    }),
  );
}

export async function getMarketShareSessionLoads(publicBaseUrl: string, app?: string) {
  const base = publicBaseUrl.trim().replace(/\/+$/, "");
  if (!base) return [] as ShareSessionLoad[];
  const query = app ? `?${new URLSearchParams({ app }).toString()}` : "";
  return parseJson<ShareSessionLoad[]>(
    await fetch(`${base}/v1/public/share-session-loads${query}`, {
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

export async function releaseMarketShareState(
  marketEmail: string,
  input: {
    routerId: string;
    shareId: string;
    kind: string;
    appType?: string;
    modelId?: string;
  },
) {
  return parseJson<{ ok: boolean; released: number; synced: number }>(
    await authFetch(`/v1/admin/markets/${encodeURIComponent(marketEmail)}/share-states/release`, {
      method: "POST",
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

export async function getClientNotificationDeliveries() {
  return parseJson<ClientNotificationDeliveriesResponse>(
    await authFetch("/v1/admin/client-notifications/deliveries", { cache: "no-store" }),
  );
}

export async function getClientChatDeliveries() {
  return parseJson<ClientChatDeliveriesResponse>(
    await authFetch("/v1/admin/chat/deliveries", { cache: "no-store" }),
  );
}

export async function requeueClientChatDelivery(deliveryId: string) {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/admin/chat/deliveries/${encodeURIComponent(deliveryId)}/requeue`, {
      method: "POST",
    }),
  );
}

export async function saveSettings(updates: Record<string, string | null>) {
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

export async function upgradeClientInstallation(
  installationId: string,
  restartAfter = true,
  signal?: AbortSignal,
) {
  return parseJson<{ ok: boolean; taskId: string }>(
    await authFetch(`/v1/installations/${installationId}/upgrade`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ restartAfter }),
      signal,
    }),
  );
}

export type ClientInstallationUpgradeLog = {
  taskId: string;
  step: number;
  totalSteps: number;
  level: "info" | "progress" | "success" | "warn" | "error";
  message: string;
  progress: number | null;
  at: string;
};

export type ClientInstallationUpgradeStatus = {
  taskId: string;
  status: "running" | "success" | "failed";
  restartPending: boolean;
  targetCommitId: string | null;
  logs: ClientInstallationUpgradeLog[];
};

export async function getClientInstallationUpgradeStatus(
  installationId: string,
  taskId: string,
  signal?: AbortSignal,
) {
  const params = new URLSearchParams({ taskId });
  return parseJson<ClientInstallationUpgradeStatus>(
    await authFetch(`/v1/installations/${installationId}/upgrade/status?${params}`, {
      cache: "no-store",
      signal,
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

export async function getLlmMetricsFailover(range = "1h", limit = 10) {
  const params = new URLSearchParams({ range, limit: String(limit) });
  return parseJson<LlmReliabilityResponse>(await authFetch(`/v1/admin/metrics/llm/failover?${params}`, { cache: "no-store" }));
}

export async function getMetricEvents(limit = 100) {
  const params = new URLSearchParams({ limit: String(limit) });
  return parseJson<MetricEvent[]>(await authFetch(`/v1/admin/metrics/events?${params}`, { cache: "no-store" }));
}

export async function clearMetrics() {
  return parseJson<ClearMetricsResponse>(await authFetch("/v1/admin/metrics", { method: "DELETE" }));
}

export async function getClientChatRoom(installationId: string, signal?: AbortSignal) {
  const data = await parseJson<{ room: ClientChatRoom }>(
    await authFetch(`/v1/chat/clients/${encodeURIComponent(installationId)}/room`, {
      cache: "no-store",
      signal,
    }),
  );
  return data.room;
}

export async function lookupClientChatRooms(visits: ClientChatVisit[], signal?: AbortSignal) {
  const lastReadSeqByInstallation = Object.fromEntries(
    visits.map((visit) => [visit.installationId, visit.lastReadSeq]),
  );
  return parseJson<ClientChatRoomListResponse>(
    await authFetch("/v1/chat/rooms/lookup", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        installationIds: visits.map((visit) => visit.installationId),
        lastReadSeqByInstallation,
      }),
      cache: "no-store",
      signal,
    }),
  );
}

export async function getVisitedClientChatRooms(signal?: AbortSignal) {
  return parseJson<ClientChatRoomListResponse>(
    await authFetch("/v1/chat/rooms", { cache: "no-store", signal }),
  );
}

export async function getClientChatMeta(signal?: AbortSignal) {
  return parseJson<{ totalUnread: number }>(
    await authFetch("/v1/chat/meta", { cache: "no-store", signal }),
  );
}

export async function recordClientChatVisit(roomId: string) {
  const data = await parseJson<{ room: ClientChatRoom }>(
    await authFetch(`/v1/chat/rooms/${encodeURIComponent(roomId)}/visit`, {
      method: "PUT",
    }),
  );
  return data.room;
}

export async function removeClientChatVisit(roomId: string) {
  await authFetch(`/v1/chat/rooms/${encodeURIComponent(roomId)}/visit`, {
    method: "DELETE",
  });
}

export async function importClientChatVisits(visits: ClientChatVisit[]) {
  return parseJson<{ imported: number }>(
    await authFetch("/v1/chat/visits/import", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ visits }),
    }),
  );
}

export async function getClientChatMessages(
  roomId: string,
  options: { beforeSeq?: number; afterSeq?: number; limit?: number; signal?: AbortSignal } = {},
) {
  const params = new URLSearchParams({ limit: String(options.limit || 50) });
  if (options.beforeSeq != null) params.set("beforeSeq", String(options.beforeSeq));
  if (options.afterSeq != null) params.set("afterSeq", String(options.afterSeq));
  return parseJson<ClientChatMessageListResponse>(
    await authFetch(`/v1/chat/rooms/${encodeURIComponent(roomId)}/messages?${params}`, {
      cache: "no-store",
      signal: options.signal,
    }),
  );
}

export async function postClientChatMessage(
  roomId: string,
  body: string,
  clientMessageId: string,
) {
  return parseJson<ClientChatMessage>(
    await authFetch(`/v1/chat/rooms/${encodeURIComponent(roomId)}/messages`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ body, clientMessageId }),
    }),
  );
}

export async function markClientChatRead(roomId: string, lastReadSeq: number) {
  return parseJson<{ ok: boolean; lastReadSeq: number }>(
    await authFetch(`/v1/chat/rooms/${encodeURIComponent(roomId)}/read`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ lastReadSeq }),
    }),
  );
}

export async function deleteClientChatMessage(messageId: string) {
  return parseJson<ClientChatMessage>(
    await authFetch(`/v1/admin/chat/messages/${encodeURIComponent(messageId)}`, {
      method: "DELETE",
    }),
  );
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

// P18: test-connection
export async function testShareConnection(
  shareId: string,
  req: ShareConnectionTestRequest,
): Promise<ShareConnectionTestResponse> {
  return parseJson<ShareConnectionTestResponse>(
    await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/test-connection`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }),
  );
}

export async function refreshShareUsage(
  shareId: string,
  req: ShareUsageRefreshRequest,
): Promise<ShareUsageRefreshResponse> {
  return parseJson<ShareUsageRefreshResponse>(
    await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/refresh-usage`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(req),
    }),
  );
}

export async function getShareImageGenerationRequestLogs(
  shareId: string,
  limit = 50,
): Promise<ImageGenerationRequestLog[]> {
  const params = new URLSearchParams({ limit: String(limit) });
  const data = await parseJson<{ logs: ImageGenerationRequestLog[] }>(
    await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/image-request-logs?${params}`, {
      cache: "no-store",
    }),
  );
  return data.logs || [];
}
