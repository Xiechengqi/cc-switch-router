import { authFetch } from "@/lib/auth";
import type {
  DashboardResponse,
  ClearMetricsResponse,
  SettingsSnapshot,
  SettingsValidationResponse,
  SettingsUpdateResponse,
  ShareSettingsPatch,
  ShareEditView,
  ShareConnectionTestRequest,
  ShareConnectionTestResponse,
  ShareUsageRefreshRequest,
  ShareUsageRefreshResponse,
  ImageGenerationRequestLog,
  ShareUsageByEmailResponse,
  ShareUserLimitStatusResponse,
  UserApiTokenResponse,
  UserApiTokenResetResponse,
  NotificationSettings,
  TelegramBindLink,
  AccountUsagePeriod,
  AccountUsageResponse,
  ProviderUsageResponse,
  UsageCardSettingsResponse,
  UpdateUsageCardSettingsRequest,
  VersionResponse,
  MetricsSnapshot,
  HostMetricsInfo,
  HostMetricsStatus,
  MetricsSeriesResponse,
  LlmMetricsSnapshot,
  LlmTopResponse,
  LlmReliabilityResponse,
  MetricEvent,
  AlertChannelState,
  AlertChannelTestResponse,
  UserNotificationChannelState,
  UserNotificationChannelTestResponse,
  AlertIncident,
  AlertingOverview,
  MapDisplaySettings,
  MapDisplaySettingsUpdate,
  AnnouncementSettings,
  AnnouncementSettingsUpdate,
  AnnouncementResponse,
  ClientNotificationDeliveriesResponse,
  ClientChatDeliveriesResponse,
  AdminAuditResponse,
  ClientChatMessage,
  ClientChatMessageListResponse,
  ClientChatRoom,
  ClientChatRoomListResponse,
  ClientChatVisit,
  ClientMarketHost,
  ClientMarketSshHostKeyInspection,
  ClientMarketSshHostKeyRotationResponse,
  HostIpIntel,
  SupplySummaryEntry,
  ProvisionSshKey,
  ProvisioningJob,
  CreateClientMarketClientResponse,
  ClientTunnelSubdomainAvailability,
  AccountPaymentProfile,
  ClientMarketPaymentMethod,
  PaymentContact,
  ClientMarketProviderSupply,
  ClientMarketAllocationQuote,
  ClientMarketCommitQuoteResponse,
  ClientMarketBatch,
  ClientMarketRental,
  ClientMarketHostUsageHistoryEntry,
  ClientMarketHostTransferDocument,
  ClientMarketHostImportResponse,
  ShareMarketCatalog,
  ShareMarketOwnedListings,
  ShareMarketSubscriptions,
  ShareMarketOwnedShare,
  ShareMarketSeatInput,
  ShareMarketRentQuote,
  AdminMarketBillingDispute,
  MarketBillingDashboard,
  MarketBillingConfig,
  MarketBillingInvoiceHistory,
  MarketAccessDashboard,
  MarketAccessInboxSummary,
  MarketAccessRequest,
  MarketCounterparty,
  MarketAccessDecision,
  MarketAccessPricingKind,
  MarketAccessProductKind,
  MarketCreditKind,
  ClientSubdomainTakeoverRequest,
  ClientSubdomainTakeoverResponse,
  ClientLogsResponse,
  ServerLogEventsResponse,
  ServerLogMeta,
  ServerLogScope,
  ShareRequestLogsPage,
} from "@/lib/types";


export class ApiError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly details?: Record<string, unknown>;

  constructor(
    status: number,
    message: string,
    code?: string,
    details?: Record<string, unknown>,
  ) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export async function parseJson<T>(response: Response): Promise<T> {
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new ApiError(
      response.status,
      data?.message || `HTTP ${response.status}`,
      typeof data?.code === "string" ? data.code : undefined,
      data?.details && typeof data.details === "object" ? data.details : undefined,
    );
  }
  return data as T;
}

export async function getDashboard(signal?: AbortSignal) {
  return parseJson<DashboardResponse>(
    await authFetch("/v1/dashboard", { cache: "no-store", signal }),
  );
}

export async function getClientLogs(installationId: string, signal?: AbortSignal) {
  return parseJson<ClientLogsResponse>(
    await authFetch(`/v1/clients/${encodeURIComponent(installationId)}/logs`, {
      cache: "no-store",
      signal,
    }),
  );
}

export type ServerLogQuery = {
  scope: ServerLogScope;
  installationId?: string;
  clientAlias?: string;
  search?: string;
  cursor?: string;
  limit?: number;
};

function serverLogQueryString(query: ServerLogQuery) {
  const params = new URLSearchParams({ scope: query.scope });
  if (query.installationId) params.set("installationId", query.installationId);
  if (query.clientAlias) params.set("clientAlias", query.clientAlias);
  if (query.search) params.set("search", query.search);
  if (query.cursor) params.set("cursor", query.cursor);
  if (query.limit) params.set("limit", String(query.limit));
  return params.toString();
}

export async function getServerLogMeta() {
  return parseJson<ServerLogMeta>(
    await authFetch("/v1/server-logs/meta", { cache: "no-store" }),
  );
}

export async function getServerLogs(query: ServerLogQuery) {
  return parseJson<ServerLogEventsResponse>(
    await authFetch(`/v1/server-logs/events?${serverLogQueryString(query)}`, {
      cache: "no-store",
    }),
  );
}

export async function exportServerLogs(query: Omit<ServerLogQuery, "cursor" | "limit">) {
  const response = await authFetch(`/v1/server-logs/export?${serverLogQueryString(query)}`, {
    cache: "no-store",
  });
  if (!response.ok) {
    await parseJson(response);
  }
  return response.blob();
}

export async function takeOverClientSubdomain(input: ClientSubdomainTakeoverRequest) {
  return parseJson<ClientSubdomainTakeoverResponse>(
    await authFetch("/v1/installations/client-subdomain-takeover", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
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
  targetType?: "request" | "client" | "share" | "country";
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
  period: "24h" | "1w" | "30d",
) {
  const params = new URLSearchParams({ period });
  return parseJson<ShareUsageByEmailResponse>(
    await fetch(`/v1/shares/${encodeURIComponent(shareId)}/usage-by-email?${params}`, {
      cache: "no-store",
    }),
  );
}

export async function getShareUserLimitStatus(shareId: string) {
  return parseJson<ShareUserLimitStatusResponse>(
    await fetch(`/v1/shares/${encodeURIComponent(shareId)}/user-limit-status`, {
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

export async function getMyNotificationSettings() {
  return parseJson<NotificationSettings>(
    await authFetch("/v1/me/notifications", { cache: "no-store" }),
  );
}

export async function updateMyNotificationSettings(channel: string) {
  return parseJson<NotificationSettings>(
    await authFetch("/v1/me/notifications", {
      method: "PATCH",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ channel }),
    }),
  );
}

export async function createTelegramBindLink() {
  return parseJson<TelegramBindLink>(
    await authFetch("/v1/me/notifications/telegram/bind-link", { method: "POST" }),
  );
}

export async function unbindMyTelegramChat() {
  return parseJson<NotificationSettings>(
    await authFetch("/v1/me/notifications/telegram", { method: "DELETE" }),
  );
}

export async function getMyUsageConsumer(period: AccountUsagePeriod | string) {
  const params = new URLSearchParams({ period });
  return parseJson<AccountUsageResponse>(
    await authFetch(`/v1/me/usage/consumer?${params}`, { cache: "no-store" }),
  );
}

export async function getMyUsageProvider(period: AccountUsagePeriod | string) {
  const params = new URLSearchParams({ period });
  return parseJson<ProviderUsageResponse>(
    await authFetch(`/v1/me/usage/provider?${params}`, { cache: "no-store" }),
  );
}

export async function getMyUsageCardSettings() {
  return parseJson<UsageCardSettingsResponse>(
    await authFetch("/v1/me/usage-card", { cache: "no-store" }),
  );
}

export async function updateMyUsageCardSettings(patch: UpdateUsageCardSettingsRequest) {
  return parseJson<UsageCardSettingsResponse>(
    await authFetch("/v1/me/usage-card", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    }),
  );
}

export async function getSettings() {
  return parseJson<SettingsSnapshot>(
    await authFetch("/v1/admin/settings", { cache: "no-store" }),
  );
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

export async function getAdminAudit(limit = 100) {
  const params = new URLSearchParams({ limit: String(limit) });
  return parseJson<AdminAuditResponse>(
    await authFetch(`/v1/admin/audit?${params}`, { cache: "no-store" }),
  );
}

export async function requeueClientChatDelivery(deliveryId: string) {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/admin/chat/deliveries/${encodeURIComponent(deliveryId)}/requeue`, {
      method: "POST",
    }),
  );
}

export async function validateSettings(
  expectedRevision: string,
  updates: Record<string, string | null>,
) {
  return parseJson<SettingsValidationResponse>(
    await authFetch("/v1/admin/settings/validate", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedRevision, updates }),
    }),
  );
}

export async function saveSettings(
  expectedRevision: string,
  updates: Record<string, string | null>,
) {
  return parseJson<SettingsUpdateResponse>(
    await authFetch("/v1/admin/settings", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedRevision, updates }),
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
  statusSync: "reported" | "pending" | "unavailable" | "lost";
  updatedAt: string;
};

export async function getClientInstallationUpgradeStatus(
  installationId: string,
  taskId?: string,
  signal?: AbortSignal,
) {
  const params = new URLSearchParams();
  if (taskId) params.set("taskId", taskId);
  const query = params.toString();
  return parseJson<ClientInstallationUpgradeStatus>(
    await authFetch(`/v1/installations/${installationId}/upgrade/status${query ? `?${query}` : ""}`, {
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

export async function getMetricsSnapshot() {
  return parseJson<MetricsSnapshot>(await authFetch("/v1/admin/metrics/snapshot", { cache: "no-store" }));
}

export async function getMetricsHostInfo() {
  return parseJson<HostMetricsInfo>(await authFetch("/v1/admin/metrics/host/info", { cache: "no-store" }));
}

export async function getMetricsSeries(range: string, step?: string) {
  const params = new URLSearchParams({ range });
  if (step) params.set("step", step);
  return parseJson<MetricsSeriesResponse>(await authFetch(`/v1/admin/metrics/series?${params}`, { cache: "no-store" }));
}

export async function getLlmMetricsTop(range = "1h", by = "tokens", limit?: number) {
  const params = new URLSearchParams({ range, by });
  if (limit != null) params.set("limit", String(limit));
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

export async function getAlertingOverview(limit = 100) {
  const params = new URLSearchParams({ limit: String(limit) });
  return parseJson<AlertingOverview>(
    await authFetch(`/v1/admin/alerting/overview?${params}`, { cache: "no-store" }),
  );
}

export async function getAlertingChannels() {
  return parseJson<AlertChannelState[]>(
    await authFetch("/v1/admin/alerting/channels", { cache: "no-store" }),
  );
}

export async function testAlertingChannel(channel: string) {
  return parseJson<AlertChannelTestResponse>(
    await authFetch(`/v1/admin/alerting/channels/${encodeURIComponent(channel)}/test`, {
      method: "POST",
    }),
  );
}

export async function getUserNotificationChannels() {
  return parseJson<UserNotificationChannelState[]>(
    await authFetch("/v1/admin/user-notifications/channels", { cache: "no-store" }),
  );
}

export async function testUserNotificationChannel(channel: string) {
  return parseJson<UserNotificationChannelTestResponse>(
    await authFetch(`/v1/admin/user-notifications/channels/${encodeURIComponent(channel)}/test`, {
      method: "POST",
    }),
  );
}

export async function acknowledgeAlertIncident(incidentId: string, note?: string) {
  return parseJson<AlertIncident>(
    await authFetch(`/v1/admin/alerting/incidents/${encodeURIComponent(incidentId)}/acknowledge`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ note: note || null }),
    }),
  );
}

export async function silenceAlertIncident(incidentId: string, durationSecs: number, note?: string) {
  return parseJson<AlertIncident>(
    await authFetch(`/v1/admin/alerting/incidents/${encodeURIComponent(incidentId)}/silence`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ durationSecs, note: note || null }),
    }),
  );
}

export async function resumeAlertIncident(incidentId: string, note?: string) {
  return parseJson<AlertIncident>(
    await authFetch(`/v1/admin/alerting/incidents/${encodeURIComponent(incidentId)}/resume`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ note: note || null }),
    }),
  );
}

export async function retryAlertDelivery(deliveryId: string) {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/admin/alerting/deliveries/${encodeURIComponent(deliveryId)}/retry`, {
      method: "POST",
    }),
  );
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

export async function getShareRequestLogs(
  shareId: string,
  options: { app?: "claude" | "codex" | "gemini"; cursor?: string; limit?: number } = {},
): Promise<ShareRequestLogsPage> {
  const params = new URLSearchParams();
  if (options.app) params.set("app", options.app);
  if (options.cursor) params.set("cursor", options.cursor);
  params.set("limit", String(options.limit || 50));
  return parseJson<ShareRequestLogsPage>(
    await authFetch(`/v1/shares/${encodeURIComponent(shareId)}/request-logs?${params}`, {
      cache: "no-store",
    }),
  );
}

export async function getProvisionSshKey() {
  return parseJson<ProvisionSshKey>(
    await authFetch("/v1/client-market/provision-ssh-key", { cache: "no-store" }),
  );
}

export async function getClientMarketHosts(
  params?: {
    ownerEmail?: string;
    country?: string;
    status?: string;
  },
  signal?: AbortSignal,
) {
  const search = new URLSearchParams();
  if (params?.ownerEmail) search.set("ownerEmail", params.ownerEmail);
  if (params?.country) search.set("country", params.country);
  if (params?.status) search.set("status", params.status);
  const query = search.toString();
  return parseJson<ClientMarketHost[]>(
    await authFetch(`/v1/client-market/hosts${query ? `?${query}` : ""}`, {
      cache: "no-store",
      signal,
    }),
  );
}

export async function createClientMarketHost(body: {
  ip: string;
  port?: number;
  note?: string;
  rootPassword?: string;
  dailyRateMinor?: number;
  currency?: string;
  freeDurationDays?: number;
}) {
  return parseJson<ClientMarketHost>(
    await authFetch("/v1/client-market/hosts", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function testClientMarketHostSsh(body: {
  ip: string;
  port?: number;
  rootPassword?: string;
}) {
  return parseJson<{ ok: boolean }>(
    await authFetch("/v1/client-market/hosts/test-ssh", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function lookupClientMarketHostIpInfo(body: { ip: string }) {
  return parseJson<HostIpIntel>(
    await authFetch("/v1/client-market/hosts/ip-info", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function deleteClientMarketHost(id: string) {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(id)}`, { method: "DELETE" }),
  );
}

export async function retireUnreachableClientMarketHost(id: string) {
  return parseJson<{
    hostId: string;
    installationId: string;
    previousSubscriptionStatus: string;
    status: string;
  }>(
    await authFetch(
      `/v1/client-market/hosts/${encodeURIComponent(id)}/retire-unreachable`,
      { method: "POST" },
    ),
  );
}

export async function reverifyClientMarketHost(id: string) {
  return parseJson<ClientMarketHost>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(id)}/reverify`, {
      method: "POST",
    }),
  );
}

export async function scanClientMarketHostSshKey(id: string, signal?: AbortSignal) {
  return parseJson<ClientMarketSshHostKeyInspection>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(id)}/ssh-host-key/scan`, {
      method: "POST",
      cache: "no-store",
      signal,
    }),
  );
}

export async function rotateClientMarketHostSshKey(
  id: string,
  body: {
    expectedCurrentFingerprint?: string;
    confirmedFingerprint: string;
    verifiedFromHostConsole: boolean;
  },
) {
  return parseJson<ClientMarketSshHostKeyRotationResponse>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(id)}/ssh-host-key/rotate`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function createClientMarketTerminalSession(hostId: string) {
  return parseJson<{ ticket: string; expiresInSec: number }>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(hostId)}/terminal-session`, {
      method: "POST",
    }),
  );
}

export async function getClientMarketJob(id: string) {
  return parseJson<ProvisioningJob>(
    await authFetch(`/v1/client-market/jobs/${encodeURIComponent(id)}`, { cache: "no-store" }),
  );
}

export async function cleanupClientMarketProviderRental(
  installationId: string,
  body: {
    reason:
      | "provider_release"
      | "host_maintenance"
      | "service_terminated"
      | "other";
    denyClientAccess?: boolean;
  },
) {
  return parseJson<CreateClientMarketClientResponse>(
    await authFetch(`/v1/client-market/clients/${encodeURIComponent(installationId)}/provider-cleanup`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function releaseClientMarketRental(installationId: string) {
  return parseJson<CreateClientMarketClientResponse>(
    await authFetch(`/v1/client-market/clients/${encodeURIComponent(installationId)}/release`, {
      method: "POST",
    }),
  );
}

export async function finalizeClientMarketFailedRental(installationId: string) {
  return parseJson<{
    installationId: string;
    previousStatus: string;
    status: string;
    hostId?: string;
    hostStatus?: string;
  }>(
    await authFetch(
      `/v1/client-market/clients/${encodeURIComponent(installationId)}/finalize-release`,
      { method: "POST" },
    ),
  );
}

export async function getAccountPaymentProfile() {
  return parseJson<AccountPaymentProfile>(
    await authFetch("/v1/account/payment-profile", { cache: "no-store" }),
  );
}

export async function updateAccountPaymentProfile(
  methods: ClientMarketPaymentMethod[],
  contacts?: PaymentContact[],
) {
  return parseJson<AccountPaymentProfile>(
    await authFetch("/v1/account/payment-profile", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ methods, contacts }),
    }),
  );
}

export async function getMarketBillingDashboard(signal?: AbortSignal) {
  return parseJson<MarketBillingDashboard>(
    await authFetch("/v1/market-billing/dashboard", { cache: "no-store", signal }),
  );
}

export async function getMarketBillingConfig(signal?: AbortSignal) {
  return parseJson<MarketBillingConfig>(
    await authFetch("/v1/market-billing/config", { cache: "no-store", signal }),
  );
}

export async function updateMarketBillingSupplierProfile(
  currency: "USD",
  settlementGraceHours: number,
) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/supplier-profiles/${currency}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ settlementGraceHours }),
    }),
  );
}

export async function settleMarketBillingAccount(accountId: string) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/accounts/${encodeURIComponent(accountId)}/settle`, {
      method: "POST",
    }),
  );
}

export async function requestMarketBillingSettlement(accountId: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/market-billing/accounts/${encodeURIComponent(accountId)}/request-settlement`, {
      method: "POST",
    }),
  );
}

export async function closeMarketBillingAccount(accountId: string) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/accounts/${encodeURIComponent(accountId)}/close`, {
      method: "POST",
    }),
  );
}

export async function getMarketBillingInvoiceHistory(
  accountId: string,
  beforeSequence?: number,
) {
  const query = new URLSearchParams({ limit: "20" });
  if (beforeSequence != null) query.set("beforeSequence", String(beforeSequence));
  return parseJson<MarketBillingInvoiceHistory>(
    await authFetch(
      `/v1/market-billing/accounts/${encodeURIComponent(accountId)}/invoices?${query.toString()}`,
      { cache: "no-store" },
    ),
  );
}

export async function declareMarketBillingPayment(
  invoiceId: string,
  body: {
    paymentMethodKind?: string;
    paymentReference?: string;
    note?: string;
    evidenceUrl?: string;
  },
) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/invoices/${encodeURIComponent(invoiceId)}/declare-payment`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function confirmMarketBillingPayment(invoiceId: string) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/invoices/${encodeURIComponent(invoiceId)}/confirm`, {
      method: "POST",
    }),
  );
}

export async function rejectMarketBillingPayment(invoiceId: string, reason: string) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/invoices/${encodeURIComponent(invoiceId)}/reject`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ reason }),
    }),
  );
}

export async function disputeMarketBillingInvoice(invoiceId: string, reason: string) {
  return parseJson<MarketBillingDashboard>(
    await authFetch(`/v1/market-billing/invoices/${encodeURIComponent(invoiceId)}/disputes`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ reason }),
    }),
  );
}

export async function getAdminMarketBillingDisputes(signal?: AbortSignal) {
  return parseJson<AdminMarketBillingDispute[]>(
    await authFetch("/v1/admin/market-billing/disputes", { cache: "no-store", signal }),
  );
}

export async function resolveAdminMarketBillingDispute(
  disputeId: string,
  resolution: "uphold" | "void",
  note?: string,
) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/admin/market-billing/disputes/${encodeURIComponent(disputeId)}/resolve`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ resolution, note }),
    }),
  );
}

export async function voidAdminMarketBillingInvoice(invoiceId: string, reason: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/admin/market-billing/invoices/${encodeURIComponent(invoiceId)}/void`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ reason }),
    }),
  );
}

export async function getMarketAccessDashboard(signal?: AbortSignal) {
  return parseJson<MarketAccessDashboard>(
    await authFetch("/v1/market-access/dashboard", { cache: "no-store", signal }),
  );
}

export async function getMarketAccessInboxSummary(signal?: AbortSignal) {
  return parseJson<MarketAccessInboxSummary>(
    await authFetch("/v1/market-access/inbox-summary", { cache: "no-store", signal }),
  );
}

export async function updateMarketAccessPolicy(
  productKind: MarketAccessProductKind,
  pricingKind: MarketAccessPricingKind,
  body: { mode: "whitelist" | "blacklist"; riskAcknowledged?: boolean; expectedRevision: number },
) {
  return parseJson<MarketAccessDashboard>(
    await authFetch(`/v1/market-access/policies/${productKind}/${pricingKind}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function upsertMarketCounterparty(body: {
  email: string;
  accessRules: Array<{
    productKind: MarketAccessProductKind;
    pricingKind: MarketAccessPricingKind;
    decision: MarketAccessDecision;
  }>;
  creditLines?: Array<{
    currency: "USD";
    kind: MarketCreditKind;
    limitMinor?: number;
    riskAcknowledged?: boolean;
  }>;
}) {
  return parseJson<MarketCounterparty>(
    await authFetch("/v1/market-access/counterparties", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function updateMarketCounterparty(
  id: string,
  body: {
    accessRules: Array<{
      productKind: MarketAccessProductKind;
      pricingKind: MarketAccessPricingKind;
      decision: MarketAccessDecision;
    }>;
    status?: "active" | "revoked";
    expectedRevision: number;
  },
) {
  return parseJson<unknown>(
    await authFetch(`/v1/market-access/counterparties/${encodeURIComponent(id)}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function updateMarketCounterpartyCredit(
  id: string,
  currency: "USD",
  body: {
    kind: MarketCreditKind;
    limitMinor?: number;
    riskAcknowledged?: boolean;
    expectedRevision: number;
  },
) {
  return parseJson<unknown>(
    await authFetch(`/v1/market-access/counterparties/${encodeURIComponent(id)}/credit-lines/${currency}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function updateMarketCounterpartiesBatch(body: {
  updates: Array<{
    id: string;
    expectedRevision: number;
    accessRules: Array<{
      productKind: MarketAccessProductKind;
      pricingKind: MarketAccessPricingKind;
      decision: MarketAccessDecision;
    }>;
    status?: "active" | "revoked";
    creditLines: Array<{
      currency: "USD";
      kind: MarketCreditKind;
      limitMinor?: number;
      riskAcknowledged?: boolean;
      expectedRevision: number;
    }>;
  }>;
}) {
  return parseJson<MarketAccessDashboard>(
    await authFetch("/v1/market-access/counterparties/batch", {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function updateMarketPublicCredit(
  currency: "USD",
  body: { enabled: boolean; limitMinor?: number; riskAcknowledged?: boolean; expectedRevision: number },
) {
  return parseJson<MarketAccessDashboard>(
    await authFetch(`/v1/market-access/public-credit-lines/${currency}`, {
      method: "PUT",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function createMarketAccessRequest(body: {
  targetKind: "share_seat" | "client_host";
  targetId: string;
}) {
  return parseJson<MarketAccessRequest>(
    await authFetch("/v1/market-access/requests", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function approveMarketAccessRequest(
  id: string,
  body: {
    expectedRevision: number;
    creditLine?: {
      currency: "USD";
      kind: MarketCreditKind;
      limitMinor?: number;
      riskAcknowledged?: boolean;
      expectedRevision: number;
    };
  },
) {
  return parseJson<MarketAccessDashboard>(
    await authFetch(`/v1/market-access/requests/${encodeURIComponent(id)}/approve`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function rejectMarketAccessRequest(
  id: string,
  expectedRevision: number,
  reason: string,
) {
  return parseJson<MarketAccessDashboard>(
    await authFetch(`/v1/market-access/requests/${encodeURIComponent(id)}/reject`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedRevision, reason }),
    }),
  );
}

export async function cancelMarketAccessRequest(id: string, expectedRevision: number) {
  return parseJson<MarketAccessRequest>(
    await authFetch(`/v1/market-access/requests/${encodeURIComponent(id)}/cancel`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ expectedRevision }),
    }),
  );
}

export async function getClientMarketProviderSupply() {
  return parseJson<ClientMarketProviderSupply>(
    await authFetch("/v1/client-market/providers", { cache: "no-store" }),
  );
}

export async function updateClientMarketHostOffer(
  hostId: string,
  body: { dailyRateMinor?: number; currency?: string; freeDurationDays?: number },
) {
  return parseJson<{
    hostId: string;
    dailyRateMinor?: number;
    currency?: string;
    freeDurationDays?: number;
    offerRevision: number;
  }>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(hostId)}/offer`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function createClientMarketQuote(body: {
  providerIds: string[];
  countryCodes: string[];
  count: number;
  hostId?: string;
}) {
  return parseJson<ClientMarketAllocationQuote>(
    await authFetch("/v1/client-market/quotes", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function commitClientMarketQuote(
  quoteId: string,
  items: Array<{ quoteItemId: string; offerRevision: number; subdomain: string; password: string }>,
  idempotencyKey: string,
) {
  return parseJson<ClientMarketCommitQuoteResponse>(
    await authFetch(`/v1/client-market/quotes/${encodeURIComponent(quoteId)}/commit`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ items, idempotencyKey }),
    }),
  );
}

export async function getClientMarketBatch(batchId: string, signal?: AbortSignal) {
  return parseJson<ClientMarketBatch>(
    await authFetch(`/v1/client-market/batches/${encodeURIComponent(batchId)}`, {
      cache: "no-store",
      signal,
    }),
  );
}

export async function cancelClientMarketQuote(quoteId: string) {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/client-market/quotes/${encodeURIComponent(quoteId)}/cancel`, {
      method: "POST",
    }),
  );
}

export async function getMyClientMarketRentals(signal?: AbortSignal) {
  return parseJson<ClientMarketRental[]>(
    await authFetch("/v1/client-market/my-rentals", { cache: "no-store", signal }),
  );
}

export async function getClientMarketHostUsageHistory(
  hostId: string,
  signal?: AbortSignal,
) {
  return parseJson<ClientMarketHostUsageHistoryEntry[]>(
    await authFetch(
      `/v1/client-market/hosts/${encodeURIComponent(hostId)}/usage-history`,
      { cache: "no-store", signal },
    ),
  );
}

export async function grantClientMarketProviderTerminalAccess(
  installationId: string,
  durationMinutes: number,
) {
  return parseJson<{ active: boolean; expiresAt?: string; updatedAt: string }>(
    await authFetch(
      `/v1/client-market/clients/${encodeURIComponent(installationId)}/provider-terminal-authorization`,
      {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ durationMinutes }),
      },
    ),
  );
}

export async function revokeClientMarketProviderTerminalAccess(installationId: string) {
  return parseJson<{ active: boolean; expiresAt?: string; updatedAt: string }>(
    await authFetch(
      `/v1/client-market/clients/${encodeURIComponent(installationId)}/provider-terminal-authorization`,
      { method: "DELETE" },
    ),
  );
}

export async function exportMyClientMarketHosts() {
  return parseJson<ClientMarketHostTransferDocument>(
    await authFetch("/v1/client-market/hosts/export", { cache: "no-store" }),
  );
}

export async function importMyClientMarketHosts(document: ClientMarketHostTransferDocument) {
  return parseJson<ClientMarketHostImportResponse>(
    await authFetch("/v1/client-market/hosts/import", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(document),
    }),
  );
}

export async function getClientMarketHostImport(jobId: string, signal?: AbortSignal) {
  return parseJson<ClientMarketHostImportResponse>(
    await authFetch(`/v1/client-market/hosts/import/${encodeURIComponent(jobId)}`, {
      cache: "no-store",
      signal,
    }),
  );
}

export async function checkClientTunnelSubdomainAvailability(subdomain: string, installationId?: string) {
  const params = new URLSearchParams({ subdomain });
  if (installationId) params.set("installationId", installationId);
  const response = await authFetch(`/v1/client-tunnel/subdomain-availability?${params}`, { cache: "no-store" });
  if (response.status === 409) return { available: false, reason: "reserved" } satisfies ClientTunnelSubdomainAvailability;
  return parseJson<ClientTunnelSubdomainAvailability>(response);
}

export async function getShareMarketCatalog(signal?: AbortSignal) {
  return parseJson<ShareMarketCatalog>(
    await authFetch("/v1/share-market/listings", { cache: "no-cache", signal }),
  );
}

export async function getShareMarketOwnedListings(signal?: AbortSignal) {
  return parseJson<ShareMarketOwnedListings>(
    await authFetch("/v1/share-market/me/listings", { cache: "no-cache", signal }),
  );
}

export async function getShareMarketSubscriptions(signal?: AbortSignal) {
  return parseJson<ShareMarketSubscriptions>(
    await authFetch("/v1/share-market/me/subscriptions", { cache: "no-cache", signal }),
  );
}

export async function getShareMarketOwnedShares(signal?: AbortSignal) {
  return parseJson<ShareMarketOwnedShare[]>(
    await authFetch("/v1/share-market/owned-shares", { cache: "no-store", signal }),
  );
}

export async function createShareMarketListing(shareId: string, seats: ShareMarketSeatInput[]) {
  return parseJson<{ ok: true; listingId: string }>(
    await authFetch("/v1/share-market/listings", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ shareId, seats }),
    }),
  );
}

export async function addShareMarketSeat(listingId: string, seat: ShareMarketSeatInput) {
  return parseJson<{ ok: true; seatId: string }>(
    await authFetch(`/v1/share-market/listings/${encodeURIComponent(listingId)}/seats`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(seat),
    }),
  );
}

export async function updateShareMarketSeat(
  seatId: string,
  seat: ShareMarketSeatInput,
  offerRevision: number,
) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/seats/${encodeURIComponent(seatId)}`, {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ seat, offerRevision }),
    }),
  );
}

export async function deleteShareMarketSeat(seatId: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/seats/${encodeURIComponent(seatId)}`, { method: "DELETE" }),
  );
}

export async function closeShareMarketListing(listingId: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/listings/${encodeURIComponent(listingId)}`, { method: "DELETE" }),
  );
}

export async function deleteShareMarketListing(listingId: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/listings/${encodeURIComponent(listingId)}/delete`, {
      method: "POST",
    }),
  );
}

export async function quoteShareMarketSeat(seatId: string, requiredApp?: string) {
  return parseJson<ShareMarketRentQuote>(
    await authFetch(`/v1/share-market/seats/${encodeURIComponent(seatId)}/quote`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ requiredApp }),
    }),
  );
}

export async function rentShareMarketSeat(
  seatId: string,
  quoteId: string,
  idempotencyKey: string,
) {
  return parseJson<{ ok: true; subscriptionId: string; replayed: boolean }>(
    await authFetch(`/v1/share-market/seats/${encodeURIComponent(seatId)}/rent`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ quoteId, idempotencyKey }),
    }),
  );
}

export async function releaseShareMarketSubscription(subscriptionId: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/subscriptions/${encodeURIComponent(subscriptionId)}/release`, {
      method: "POST",
    }),
  );
}

export async function forceRevokeShareMarketSubscription(
  subscriptionId: string,
  input: { denyFutureAccess: boolean },
) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/subscriptions/${encodeURIComponent(subscriptionId)}/force-revoke`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
}

export async function proposeShareMarketPriceChange(
  subscriptionId: string,
  dailyRateMinor: number,
  offerRevision: number,
) {
  return parseJson<{ ok: true; proposalId: string }>(
    await authFetch(
      `/v1/share-market/subscriptions/${encodeURIComponent(subscriptionId)}/price-changes`,
      {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ dailyRateMinor, offerRevision }),
      },
    ),
  );
}

async function resolveShareMarketPriceChange(
  proposalId: string,
  action: "accept" | "reject" | "cancel",
) {
  return parseJson<{ ok: true }>(
    await authFetch(
      `/v1/share-market/price-changes/${encodeURIComponent(proposalId)}/${action}`,
      { method: "POST" },
    ),
  );
}

export function acceptShareMarketPriceChange(proposalId: string) {
  return resolveShareMarketPriceChange(proposalId, "accept");
}

export function rejectShareMarketPriceChange(proposalId: string) {
  return resolveShareMarketPriceChange(proposalId, "reject");
}

export function cancelShareMarketPriceChange(proposalId: string) {
  return resolveShareMarketPriceChange(proposalId, "cancel");
}
