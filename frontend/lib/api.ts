import { authFetch } from "@/lib/auth";
import type {
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
  ShareUserLimitStatusResponse,
  UserApiTokenResponse,
  UserApiTokenResetResponse,
  AccountUsagePeriod,
  AccountUsageResponse,
  ProviderUsageResponse,
  UserProfileResponse,
  UpdateUserProfileRequest,
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
  ClientMarketHost,
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
  ClientMarketBilling,
  ClientMarketHostTransferDocument,
  ClientMarketHostImportResponse,
  ClientMarketProviderBlock,
  ShareMarketCatalog,
  ShareMarketOwnedShare,
  ShareMarketSeatInput,
} from "@/lib/types";


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

export async function getShareUserLimitStatus(
  shareId: string,
  app: "claude" | "codex" | "gemini" | string,
) {
  const params = new URLSearchParams({ app });
  return parseJson<ShareUserLimitStatusResponse>(
    await fetch(`/v1/shares/${encodeURIComponent(shareId)}/user-limit-status?${params}`, {
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

export async function getMyProfile() {
  return parseJson<UserProfileResponse>(await authFetch("/v1/me/profile", { cache: "no-store" }));
}

export async function updateMyProfile(patch: UpdateUserProfileRequest) {
  return parseJson<UserProfileResponse>(
    await authFetch("/v1/me/profile", {
      method: "PATCH",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(patch),
    }),
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

export async function getMetricsSeries(range: string, step?: string) {
  const params = new URLSearchParams({ range });
  if (step) params.set("step", step);
  return parseJson<MetricsSeriesResponse>(await authFetch(`/v1/admin/metrics/series?${params}`, { cache: "no-store" }));
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
  priceCents?: number;
  rentalPeriodDays?: number;
  currency?: string;
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

export async function reverifyClientMarketHost(id: string) {
  return parseJson<ClientMarketHost>(
    await authFetch(`/v1/client-market/hosts/${encodeURIComponent(id)}/reverify`, {
      method: "POST",
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

export async function cleanupClientMarketClientWithReason(
  installationId: string,
  body: {
    reason:
      | "client_release"
      | "provider_release"
      | "payment_not_received"
      | "host_maintenance"
      | "service_terminated"
      | "other";
    blockClientForProvider?: boolean;
  },
) {
  return parseJson<CreateClientMarketClientResponse>(
    await authFetch(`/v1/client-market/clients/${encodeURIComponent(installationId)}/cleanup`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
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

export async function getClientMarketProviderBlocks() {
  return parseJson<ClientMarketProviderBlock[]>(
    await authFetch("/v1/client-market/provider-blocks", { cache: "no-store" }),
  );
}

export async function createClientMarketProviderBlock(body: { email: string; reason?: string }) {
  return parseJson<ClientMarketProviderBlock>(
    await authFetch("/v1/client-market/provider-blocks", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

export async function liftClientMarketProviderBlock(clientUserId: string) {
  return parseJson<{ ok: boolean }>(
    await authFetch(`/v1/client-market/provider-blocks/${encodeURIComponent(clientUserId)}`, {
      method: "DELETE",
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
  body: { priceCents?: number; rentalPeriodDays?: number; currency?: string },
) {
  return parseJson<{
    hostId: string;
    priceCents?: number;
    rentalPeriodDays?: number;
    currency?: string;
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
) {
  return parseJson<ClientMarketCommitQuoteResponse>(
    await authFetch(`/v1/client-market/quotes/${encodeURIComponent(quoteId)}/commit`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ items }),
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

export async function getMyClientMarketBilling(signal?: AbortSignal) {
  return parseJson<ClientMarketBilling[]>(
    await authFetch("/v1/client-market/my-billing", { cache: "no-store", signal }),
  );
}

export async function declareClientMarketPayment(
  installationId: string,
  invoiceId: string,
  offerRevision: number,
  paymentProfileUpdatedAt?: string,
  /** The amount actually shown to the user. The Router rejects the declaration if
   *  this disagrees with the invoice, so a silent price change cannot be paid
   *  through by a UI that refreshed without the user re-reading the number. */
  amountCentsConfirmed?: number,
) {
  return parseJson<{ billing: ClientMarketBilling }>(
    await authFetch(`/v1/client-market/clients/${encodeURIComponent(installationId)}/declare-paid`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        invoiceId,
        offerRevision,
        paymentProfileUpdatedAt,
        amountCentsConfirmed,
        confirmed: true,
      }),
    }),
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

export async function checkClientTunnelSubdomainAvailability(subdomain: string, installationId?: string) {
  const params = new URLSearchParams({ subdomain });
  if (installationId) params.set("installationId", installationId);
  const response = await authFetch(`/v1/client-tunnel/subdomain-availability?${params}`, { cache: "no-store" });
  if (response.status === 409) return { available: false, reason: "reserved" } satisfies ClientTunnelSubdomainAvailability;
  return parseJson<ClientTunnelSubdomainAvailability>(response);
}

export async function getShareMarketCatalog(signal?: AbortSignal) {
  return parseJson<ShareMarketCatalog>(
    await authFetch("/v1/share-market/listings", { cache: "no-store", signal }),
  );
}

export async function getShareMarketOwnedShares() {
  return parseJson<ShareMarketOwnedShare[]>(
    await authFetch("/v1/share-market/owned-shares", { cache: "no-store" }),
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

export async function rentShareMarketSeat(seatId: string, offerRevision: number) {
  return parseJson<{ ok: true; subscriptionId: string }>(
    await authFetch(`/v1/share-market/seats/${encodeURIComponent(seatId)}/rent`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ offerRevision }),
    }),
  );
}

export async function declareShareMarketPaid(
  subscriptionId: string,
  input: {
    invoiceId: string;
    offerRevision: number;
    amountMinorConfirmed: number;
    paymentProfileUpdatedAt: string;
  },
) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/subscriptions/${encodeURIComponent(subscriptionId)}/declare-paid`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ ...input, confirmed: true }),
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
  input: { blockUser: boolean; reason?: string },
) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/subscriptions/${encodeURIComponent(subscriptionId)}/force-revoke`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(input),
    }),
  );
}

export async function liftShareMarketBlock(userId: string) {
  return parseJson<{ ok: true }>(
    await authFetch(`/v1/share-market/blocks/${encodeURIComponent(userId)}`, { method: "DELETE" }),
  );
}
