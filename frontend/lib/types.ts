export type AuthUser = {
  id: string;
  email: string;
};

export type SessionStatus = {
  authenticated: boolean;
  user?: AuthUser;
  expiresAt?: string;
  isAdmin: boolean;
};

export type MapViewportSettings = {
  visibleStartPx: number;
};

export type MapDisplaySettings = {
  showFlows: boolean;
  showHeat: boolean;
  viewport: MapViewportSettings;
  revision: string;
};

export type MapDisplaySettingsUpdate = {
  expectedRevision: string;
  showFlows?: boolean;
  showHeat?: boolean;
  viewport?: Partial<MapViewportSettings>;
};

export type AnnouncementSettings = {
  enabled: boolean;
  contentEn: string;
  contentZhCn: string;
  updatedAt: string;
};

export type AnnouncementSettingsUpdate = {
  expectedRevision: string;
  enabled?: boolean;
  contentEn?: string;
  contentZhCn?: string;
};

export type AnnouncementResponse = {
  enabled: boolean;
  revision: string;
  contentEn: string;
  contentZhCn: string;
};

export type DashboardResponse = {
  generatedAt: string;
  stats: {
    clients: number;
    activeShares: number;
    totalActiveRequests: number;
  };
  map: {
    server?: MapPoint;
    countries: CountryMapPoint[];
  };
  mapDisplay: MapDisplaySettings;
  clients: DashboardClient[];
  /** 全量 share 列表；ClientBoard 按 installation 分组为横向卡片。 */
  shares?: ShareView[];
  tickerShares?: DashboardTickerShare[];
  countryCounts?: Record<string, number>;
  countryBoards?: Record<string, CountryBoard>;
  userCountryCounts?: Record<string, number>;
  recentRequestEvents?: RecentRequestEvent[];
};

export type MapPoint = {
  id: string;
  label: string;
  pointType: string;
  platform?: string;
  countryCode?: string;
  country?: string;
  region?: string;
  city?: string;
  lat?: number;
  lon?: number;
  lastSeenAt?: string;
  isActive: boolean;
  activeRequests: number;
};

export type CountryMapPoint = {
  countryCode: string;
  countryCodeIso3: string;
  countryName?: string;
  lat: number;
  lon: number;
  clientCount: number;
  shareCount: number;
  onlineShareCount: number;
  inflightRequests: number;
  clientIds: string[];
};

export type CountryShareBoard = {
  shareId: string;
  shareName: string;
  subdomain: string;
  appType: string;
  isOnline: boolean;
  activeRequests: number;
  operationalState: string;
};

export type CountryClientBoard = {
  installationId: string;
  platform: string;
  label: string;
  ownerEmail?: string;
  shareCount: number;
  operationalState: string;
  shares: CountryShareBoard[];
  overflowShareCount?: number;
};

export type CountryBoard = {
  countryCode: string;
  countryCodeIso3: string;
  countryName?: string;
  lat: number;
  lon: number;
  clientCount: number;
  shareCount: number;
  onlineShareCount: number;
  inflightRequests: number;
  clientIds: string[];
  clients: CountryClientBoard[];
  overflowClientCount?: number;
};

export type RouteState = "active" | "reconnecting" | "offline";

export type OperationalState =
  | "available"
  | "online"
  | "reconnecting"
  | "degraded"
  | "offline"
  | "maintenance"
  | "disabled";

export type OperationalReasonCode =
  | "route_reconnecting"
  | "route_offline"
  | "health_check_failed"
  | "no_online_shares"
  | "partial_share_outage"
  | "parallel_capacity_full"
  | "parallel_capacity_warning"
  | "usage_limit_warning"
  | "expired"
  | "expires_soon"
  | "provider_unavailable"
  | "medium_latency"
  | "high_latency"
  | "edit_pending"
  | "edit_failed"
  | "maintenance_enabled"
  | "manually_disabled";

export type OperationalReason = {
  code: OperationalReasonCode | string;
  severity: "info" | "warning" | "critical" | string;
  startedAt?: string;
  entityType?: "client" | "share" | "provider" | string;
  entityId?: string;
  currentValue?: string;
  threshold?: string;
};

export type OperationalSummary = {
  state: OperationalState;
  primaryReason?: OperationalReason;
  additionalReasonCount: number;
  changedAt?: string;
};

export type ShareServiceReadiness = {
  ready: boolean;
  primaryBlocker?: OperationalReason;
  additionalBlockerCount: number;
};

export type DashboardClient = {
  chatAvailable?: boolean;
  logCollectionEnabled?: boolean;
  installation: {
    id: string;
    platform: string;
    appVersion: string;
    ownerEmail?: string;
    region?: string;
    countryCode?: string;
    publicIp?: string;
    createdAt: string;
    lastSeenAt: string;
    provisionSource?: string;
    upgrade?: {
      delegateUpgradeToRouterOwner: boolean;
      updateAvailable: boolean;
      upgradeCapable: boolean;
      commitId?: string;
    };
  };
  clientTunnel?: {
    ownerEmail: string;
    subdomain: string;
    tunnelUrl: string;
    enabled: boolean;
    online: boolean;
    routeState: RouteState;
    routeStateSince?: string;
  };
  /** 该 installation 名下所有独立 share 的 id 集合。 */
  shareIds?: string[];
  /** 该 installation 名下 share 总数；等价于 shareIds.length。 */
  shareCount?: number;
  onlineMinutes24h?: number;
  onlineRate24h?: number;
  observedMinutes24h?: number;
  observationCoverage24h?: number;
  healthChecks?: HealthCheckEntry[];
  healthTimeline?: HealthTimelineBucket[];
  operationalSummary?: OperationalSummary;
  removalAt?: string;
};

export type ClientLogsResponse = {
  installationId: string;
  content: string;
  lines: number;
  limit: number;
  truncated: boolean;
  fetchedAt: string;
};

export type ServerLogScope = "public" | "mine" | "all";

export type ServerLogClient = {
  installationId?: string;
  clientAlias: string;
  subdomain?: string;
  ownerEmail?: string;
  platform: string;
  appVersion: string;
  countryCode?: string;
  region?: string;
  createdAt: string;
  lastSeenAt: string;
  tunnelEnabled?: boolean;
};

export type ServerLogMeta = {
  ingestEnabled: boolean;
  publicEnabled: boolean;
  authenticated: boolean;
  isRouterOwner: boolean;
  scopes: ServerLogScope[];
  clients: ServerLogClient[];
  retentionDays: number;
  publicWindowSeconds: number;
};

export type ServerLogEvent = {
  eventId: string;
  clientAlias: string;
  clientSubdomain?: string;
  installationId?: string;
  streamId?: string;
  sequence?: number;
  occurredAtMs: number;
  receivedAtMs: number;
  level: string;
  target: string;
  message: string;
  fields?: Record<string, unknown>;
  file?: string;
  line?: number;
  serverVersion?: string;
  commitId?: string;
};

export type ServerLogEventsResponse = {
  events: ServerLogEvent[];
  nextCursor?: string;
  publicWindowSeconds?: number;
  serverTimeMs: number;
};

export type ClientSubdomainTakeoverRequest = {
  targetInstallationId: string;
  sourceInstallationId: string;
};

export type ClientSubdomainTakeoverResponse = {
  ok: boolean;
  takeoverId: string;
  status: "completed" | "activation_pending" | "recovery_pending";
  targetInstallationId: string;
  sourceInstallationId: string;
  retiredSubdomain: string;
  adoptedSubdomain: string;
  warning?: string;
};

export type ShareTokenPeriod =
  "lifetime" | "day" | "week" | "sevenDays" | "calendarMonth" | "thirtyDays";

export type ShareUserPolicy = {
  parallelLimit?: number;
  tokenLimit?: number;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchorAtMs?: number;
  expiresAt?: number;
};

export type ShareUserUsageRebase = {
  period: ShareTokenPeriod;
  anchorAtMs?: number;
  windowStartsAtMs?: number;
  windowEndsAtMs?: number;
  targetTokens: number;
  observedTokensAtRebase: number;
  observedRequestsAtRebase: number;
  usageWatermark: number;
  appliedAtMs: number;
  appliedBy?: string;
  source: "manual" | "providerReset";
};

export type ShareUserQuotaView = {
  period: ShareTokenPeriod;
  anchorAtMs?: number;
  windowStartsAtMs?: number;
  windowEndsAtMs?: number;
  effectiveTokensUsed: number;
  observedTokensUsed: number;
  manualOffsetTokens: number;
  observedRequestsCount: number;
  rebaseApplies: boolean;
};

export type ShareUserUsageEdit = {
  action: "set" | "clear";
  targetTokens?: number;
  expectedGrantRevision?: number;
  period?: ShareTokenPeriod;
  anchorAtMs?: number;
  source?: "manual" | "providerReset";
};

export type ShareUserUsageEditMap = Record<string, ShareUserUsageEdit>;

export type ShareUserGrant = {
  email: string;
  role: "owner" | "shareto";
  active: boolean;
  policy: ShareUserPolicy;
  usage?: Record<string, unknown>;
  usageRebase?: ShareUserUsageRebase;
  usageQuota?: ShareUserQuotaView;
  createdAtMs?: number;
  updatedAtMs?: number;
  revokedAtMs?: number;
  revision?: number;
  manager?: "owner" | "manual" | "routerShareMarket";
  entitlementId?: string;
};

export type ShareUserGrantMap = Record<string, ShareUserGrant>;

export type ShareView = {
  routerId?: string;
  shareId: string;
  capacityPoolId: string;
  shareName: string;
  ownerEmail?: string;
  description?: string;
  freeAccess: boolean;
  subdomain: string;
  canViewSecret?: boolean;
  canManage?: boolean;
  canEditSettings?: boolean;
  activeEdit?: ShareEditView;
  appType: string;
  providerId?: string;
  /** 全部 app/provider bindings（{app: provider_id}）。 */
  bindings?: Record<string, string>;
  tokenLimit: number;
  parallelLimit: number;
  tokensUsed: number;
  requestsCount: number;
  shareStatus: string;
  createdAt: string;
  expiresAt: string;
  isOnline: boolean;
  routeState: RouteState;
  routeStateSince?: string;
  activeRequests: number;
  activeRequestsByApp?: Record<string, number>;
  activeRequestsByUser?: Record<string, Record<string, number>>;
  tokensUsedByApp?: Record<string, number>;
  requestsCountByApp?: Record<string, number>;
  onlineMinutes24h?: number;
  onlineRate24h: number;
  observedMinutes24h?: number;
  observationCoverage24h?: number;
  recentRequests?: ShareRequestLog[];
  healthChecks?: HealthCheckEntry[];
  healthTimeline?: HealthTimelineBucket[];
  recentModelHealthChecks?: ShareModelHealthCheck[];
  support?: ShareSupport;
  appRuntimes?: ShareAppRuntimes;
  appProviders?: ShareAppProviders;
  modelHealth?: ShareModelHealthSummary;
  operationalSummary?: OperationalSummary;
  /** 后端权威的服务就绪结果；缺失时兼容旧 Router 响应。 */
  serviceReadiness?: ShareServiceReadiness;
  userGrants?: ShareUserGrantMap;
  supportedUserTokenPeriods?: ShareTokenPeriod[];
  configRevision?: number;
  autoStart?: boolean;
  allowPersonalCredits?: boolean;
  autoConsumeBankedReset?: boolean;
  bankedResetExpiryLeadMinutes?: number;
  previousResponseCacheEnabled?: boolean;
  grokMediaPolicy?: GrokMediaPolicy;
};

export type GrokMediaPolicy = {
  imageGenerationEnabled: boolean;
  imageEditEnabled: boolean;
  videoGenerationEnabled: boolean;
};

export type ShareSettingsPatch = {
  ownerEmail?: string;
  description?: string | null;
  freeAccess?: boolean;
  tokenLimit?: number;
  parallelLimit?: number;
  expiresAt?: string;
  autoStart?: boolean;
  allowPersonalCredits?: boolean;
  autoConsumeBankedReset?: boolean;
  bankedResetExpiryLeadMinutes?: number;
  previousResponseCacheEnabled?: boolean;
  grokMediaPolicy?: GrokMediaPolicy;
  support?: ShareSupport;
  userGrants?: ShareUserGrantMap;
  userUsageEdits?: ShareUserUsageEditMap;
  managedGrant?: {
    operationId: string;
    entitlementId: string;
    shareSequence: number;
    expectedConfigRevision: number;
    action: "upsert" | "revoke";
    email: string;
    policy?: ShareUserPolicy;
  };
};

export type ShareApiAuthResponse = {
  authenticated: boolean;
  user?: {
    email: string;
    scopes: string[];
  };
  canManage: boolean;
};

export type ShareApiContextResponse = {
  mode: "share";
  shareId: string;
  subdomain: string;
};

export type ShareApiShareResponse = {
  share: ShareView;
  auth: ShareApiAuthResponse;
};

export type ShareEditView = {
  id: string;
  shareId: string;
  installationId: string;
  revision: number;
  status: "pending" | "applied" | "rejected" | string;
  patch: ShareSettingsPatch;
  createdByEmail: string;
  createdAt: string;
  updatedAt: string;
  appliedAt?: string;
  errorMessage?: string;
};

export type UserApiTokenStatus = {
  prefix: string;
  createdAt: string;
  lastUsedAt?: string;
  scopes: string[];
};

export type UserApiTokenResponse = {
  apiToken?: string;
  token: UserApiTokenStatus;
};

export type UserApiTokenResetResponse = {
  apiToken: string;
  token: UserApiTokenStatus;
};

export type ModelRoutingApp = "claude" | "codex" | "gemini";

export type UserModelRouteInput = {
  appType: ModelRoutingApp;
  requestedModel: string;
  targetShareId: string;
};

export type UserModelRoute = UserModelRouteInput & {
  id: string;
  createdAt: string;
  updatedAt: string;
};

export type UserModelRoutingShareCapability = {
  app: ModelRoutingApp | string;
  providerName?: string;
  providerType?: string;
  kind?: string;
  apiUrl?: string;
  subscriptionLevel?: string;
  quota?: ShareUpstreamProvider["quota"];
};

export type UserModelRoutingShare = {
  shareId: string;
  shareName: string;
  subdomain: string;
  directApiUrl: string;
  access: "owner" | "shared" | "free" | string;
  freeAccess: boolean;
  apps: ModelRoutingApp[];
  appCapabilities?: UserModelRoutingShareCapability[];
  isOnline: boolean;
};

export type UserModelRoutingResponse = {
  enabled: boolean;
  apiBaseUrl: string;
  revision: number;
  routes: UserModelRoute[];
  eligibleShares: UserModelRoutingShare[];
  updatedAt?: string;
};

export type ReplaceUserModelRoutingRequest = {
  expectedRevision: number;
  routes: UserModelRouteInput[];
};

export type UserModelRoutingTestRequest = {
  appType: ModelRoutingApp;
  requestedModel: string;
};

export type UserModelRoutingTestHttp = {
  statusCode: number;
  statusText: string;
  headers: [string, string][];
  bodyText: string;
  bodyTruncated: boolean;
};

export type UserModelRoutingTestResponse = {
  success: boolean;
  appType: ModelRoutingApp;
  requestedModel: string;
  curl: string;
  targetShareId?: string;
  matchedWildcard: boolean;
  response?: UserModelRoutingTestHttp;
  durationMs: number;
  error?: string;
  code?: string;
};

export type NotificationChannelSettings = {
  channel: string;
  enabled: boolean;
  available: boolean;
  state: "ready" | "unbound" | "invalid" | string;
  targetLabel?: string;
  verifiedAt?: string;
};

/** GET/PATCH /v1/me/notifications */
export type NotificationSettings = {
  email: string;
  /** The single channel notifications are delivered on. */
  deliveryChannel: string;
  channels: NotificationChannelSettings[];
  telegramBotConfigured: boolean;
  telegramBotStatus: "disabled" | "reconciling" | "ready" | "error" | string;
  telegramBotTransportStatus?: string;
  telegramBotUsername?: string;
  telegramBotFailureCode?: string;
  telegramBotFailureHint?: string;
  telegramBotFailureDetails?: Record<string, unknown>;
  telegramBotLastFailureAt?: string;
};

/** POST /v1/me/notifications/telegram/bind-link — single use, short lived. */
export type TelegramBindLink = {
  url: string;
  token: string;
  botUsername: string;
  expiresAt: string;
};

export type AccountUsagePeriod = "24h" | "7d" | "30d";

export type UsageTokenTotals = {
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
};

export type UsageModelRow = UsageTokenTotals & {
  model: string;
};

export type UsageDailyBucket = UsageTokenTotals & {
  date: string;
};

export type UsageCallerRow = UsageTokenTotals & {
  email: string;
};

export type UsageShareRow = UsageTokenTotals & {
  shareId: string;
  shareName?: string;
  models?: UsageModelRow[];
};

export type AccountUsageResponse = UsageTokenTotals & {
  period: AccountUsagePeriod | string;
  bucketGranularity?: "hour" | "day" | string;
  days: number;
  models: UsageModelRow[];
  daily: UsageDailyBucket[];
  byShare?: UsageShareRow[];
};

export type ProviderShareUsage = UsageTokenTotals & {
  shareId: string;
  shareName?: string;
  models: UsageModelRow[];
  callers?: UsageCallerRow[];
};

export type ProviderInstallationUsage = UsageTokenTotals & {
  installationId: string;
  label?: string;
  shares: ProviderShareUsage[];
};

export type ProviderUsageResponse = UsageTokenTotals & {
  period: AccountUsagePeriod | string;
  bucketGranularity?: "hour" | "day" | string;
  days: number;
  installations: ProviderInstallationUsage[];
};

export type UsageCardSettingsResponse = {
  userId: string;
  email: string;
  publicStatsEnabled: boolean;
};

export type UpdateUsageCardSettingsRequest = {
  publicStatsEnabled: boolean;
};

export type ShareRequestLog = {
  exportSequence?: number;
  requestId: string;
  requestKind?: "text" | "image" | "video" | string;
  operation?: string;
  parentRequestId?: string;
  shareId?: string;
  shareName?: string;
  providerId?: string;
  providerName?: string;
  appType?: string;
  model: string;
  requestModel?: string;
  requestAgent: string;
  requestedModel?: string;
  actualModel?: string;
  actualModelSource?: string;
  requestedReasoningEffort?: string;
  effectiveReasoningEffort?: string;
  clientServiceTier?: string;
  effectiveServiceTier?: string;
  serviceTierDecision?: string;
  usageState?:
    "pending" | "observed" | "missing" | "parse_error" | "interrupted" | "not_applicable" | string;
  streamStatus?: string;
  usageRevision?: number;
  statusCode: number;
  latencyMs: number;
  firstTokenMs?: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
  cacheUsageObserved?: boolean;
  usageEstimated?: boolean;
  isStreaming?: boolean;
  isHealthCheck?: boolean;
  userEmail?: string;
  userCountry?: string;
  userCountryIso3?: string;
  errorMessage?: string;
  mediaTaskId?: string;
  mediaStatus?: string;
  videoDurationSeconds?: number;
  videoResolution?: string;
  videoAspectRatio?: string;
  createdAt: number;
};

export type ShareRequestLogsPage = {
  logs: ShareRequestLog[];
  nextCursor?: string;
  hasMore: boolean;
};

export type ImageGenerationRequestLog = {
  requestId: string;
  shareId: string;
  shareName: string;
  installationId: string;
  providerId: string;
  providerName: string;
  appType: string;
  model: string;
  status: "running" | "succeeded" | "failed" | string;
  statusCode?: number;
  latencyMs: number;
  createdAt: number;
  completedAt?: number;
  promptPreview?: string;
  errorMessage?: string;
  resultMimeType?: string;
  resultSizeBytes?: number;
  resultUrl?: string;
  createdByEmail?: string;
  userCountry?: string;
};

export type ShareUsageDailyBucket = {
  date: string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
};

export type ShareUsageEmailRow = {
  email: string;
  role: "owner" | "shareto" | "gateway" | "deprecated" | string;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens: number;
  cacheCreationTokens: number;
  totalTokens: number;
  percent: number;
  daily: ShareUsageDailyBucket[];
};

export type ShareUsageByEmailResponse = {
  shareId: string;
  period: "24h" | "1w" | "30d" | string;
  bucketGranularity?: "hour" | "day" | string;
  days: number;
  totalTokens: number;
  rows: ShareUsageEmailRow[];
};

export type ShareUserLimitStatusRow = {
  email: string;
  role: string;
  manager?: ShareUserGrant["manager"];
  parallelLimit?: number;
  tokenLimit?: number;
  tokenPeriod: ShareTokenPeriod;
  tokenPeriodAnchorAtMs?: number;
  expiresAt?: number;
  tokensUsed: number;
  percent?: number;
  windowStartsAt?: string | null;
  resetsAt?: string | null;
};

export type ShareUserLimitStatusResponse = {
  shareId: string;
  rows: ShareUserLimitStatusRow[];
};

export type ShareModelHealthCheck = {
  requestId: string;
  shareId: string;
  subdomain: string;
  appType: string;
  requestedModel: string;
  actualModel: string;
  status: string;
  statusCode?: number;
  latencyMs: number;
  firstTokenMs?: number;
  errorMessage?: string;
  checkedAt: number;
  source: string;
};

export type DashboardTickerShare = {
  shareId: string;
  shareName: string;
  subdomain: string;
  recentRequests: ShareRequestLog[];
};

export type RecentRequestEvent = {
  requestId: string;
  shareId?: string;
  shareName?: string;
  shareSubdomain?: string;
  subdomain?: string;
  countryCode?: string;
  userCountry?: string;
  userCountryIso3?: string;
  userEmail?: string;
  startedAt?: string;
  createdAt?: string;
  isInflight?: boolean;
  latencyMs?: number;
  requestAgent?: string;
  requestedModel?: string;
  actualModel?: string;
  model?: string;
  statusCode?: number;
  inputTokens?: number;
  outputTokens?: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
  totalTokens?: number;
  usageState?:
    "pending" | "observed" | "missing" | "parse_error" | "interrupted" | string;
  streamStatus?: string;
  usageRevision?: number;
  isHealthCheck?: boolean;
  healthStatus?: string;
  healthAppType?: string;
  healthModel?: string;
};

export type ShareSupport = {
  claude?: boolean;
  codex?: boolean;
  gemini?: boolean;
};

export type ModelHealthSummary = {
  appType: string;
  requestedModel: string;
  actualModel: string;
  status: "success" | "failed" | "skipped" | string;
  recentResults?: string[];
  checkedAt?: number;
  lastCheckedAt?: number;
  lastSuccessAt?: number;
  lastFailedAt?: number;
  errorMessage?: string;
  statusCode?: number;
  latencyMs?: number;
  source?: string;
  providerId?: string;
  providerName?: string;
};

export type ShareModelHealthSummary = {
  claude?: ModelHealthSummary[];
  codex?: ModelHealthSummary[];
  gemini?: ModelHealthSummary[];
};

export type ShareProviderModelPolicyScope = "global" | "per_app";
export type ShareProviderModelPolicySource =
  "bundle_global" | "app_independent" | "profile_fixed";
export type ShareProviderModelPolicy =
  { mode: "passthrough" } | { mode: "single"; upstreamModel: string };

export type ProviderModelProbe = {
  apiType: "openai" | "anthropic" | "gemini" | string;
  requestedModel: string;
  wireModel: string;
  method: "POST" | string;
  path: string;
  body: Record<string, unknown>;
  stream: boolean;
  responseMode: "json" | "anthropic_sse" | "responses_sse" | "gemini_sse" | string;
  payloadRevision: number;
  healthFingerprint?: string;
};

export type ShareUpstreamProvider = {
  providerName?: string;
  kind?: string;
  app?: string;
  providerType?: string;
  accountLabel?: string;
  accountEmail?: string;
  subscriptionLevel?: string;
  apiUrl?: string;
  quota?: {
    status?: string;
    plan?: string;
    credentialMessage?: string;
    activityCost?: string;
    queriedAt?: number;
    subscriptionPeriodEnd?: string;
    availability?: string;
    blockedUntil?: string;
    blockedReason?: string;
    blockedScope?: string;
    tiers?: Array<{
      name?: string;
      label?: string;
      utilization?: number;
      resetsAt?: string;
      used?: number;
      limit?: number;
      unit?: string;
    }>;
  };
  models?: Array<{
    slot?: string;
    actualModel?: string;
  }>;
  modelPolicyScope?: ShareProviderModelPolicyScope;
  modelPolicySource?: ShareProviderModelPolicySource;
  modelPolicy?: ShareProviderModelPolicy;
  modelProbe?: ProviderModelProbe;
};

export type ShareAppProvider = {
  id: string;
  name: string;
  app: "claude" | "codex" | "gemini" | string;
  bundleId?: string;
  supportedApps?: string[];
  kind?: string;
  providerType?: string;
  isCurrent?: boolean;
  enabled?: boolean;
  codexImageGenerationEnabled?: boolean;
  accountLabel?: string;
  accountEmail?: string;
  subscriptionLevel?: string;
  apiUrl?: string;
  quota?: ShareUpstreamProvider["quota"];
  models?: ShareUpstreamProvider["models"];
  modelPolicyScope?: ShareProviderModelPolicyScope;
  modelPolicySource?: ShareProviderModelPolicySource;
  modelPolicy?: ShareProviderModelPolicy;
  modelProbe?: ProviderModelProbe;
};

export type ShareAppProviders = {
  claude?: ShareAppProvider[];
  codex?: ShareAppProvider[];
  gemini?: ShareAppProvider[];
};

export type ShareAppRuntimes = {
  claude?: ShareUpstreamProvider;
  codex?: ShareUpstreamProvider;
  gemini?: ShareUpstreamProvider;
  kiro?: ShareUpstreamProvider;
  cursor?: ShareUpstreamProvider;
  antigravity?: ShareUpstreamProvider;
  copilot?: ShareUpstreamProvider;
};

export type HealthCheckEntry = {
  checkedAt: number;
  isHealthy: boolean;
};

export type HealthTimelineBucket = {
  startAt: string;
  endAt: string;
  status: "healthy" | "degraded" | "unhealthy" | "offline" | "unknown" | string;
  score: number;
  onlineMinutes: number;
  observedMinutes: number;
  requestCount: number;
  failureCount: number;
};

export type SettingsCategoryId =
  | "general_display"
  | "connectivity"
  | "data_lifecycle"
  | "identity_security"
  | "notifications"
  | "observability"
  | "marketplace";

export type SettingsCategory = {
  id: SettingsCategoryId;
  label: string;
  description: string;
  fieldCount: number;
};

export type SettingsField = {
  key: string;
  label: string;
  group: string;
  category: SettingsCategoryId;
  fieldType:
    | "text"
    | "select"
    | "int"
    | "decimal"
    | "bool"
    | "path"
    | "url"
    | "email"
    | "email_list"
    | "ip_list"
    | "url_list"
    | "secret";
  required: boolean;
  restartRequired: boolean;
  risk: "normal" | "caution" | "critical";
  default?: string | null;
  description: string;
  placeholder?: string | null;
  unit?: string | null;
  constraints: {
    min?: number;
    max?: number;
    step?: number;
    minItems?: number;
    maxItems?: number;
  };
  dependencies?: Array<{ key: string; equals: string }>;
  options?: string[];
};

export type SettingsSchema = {
  fields: SettingsField[];
  groups: string[];
  categories: SettingsCategory[];
};

export type SettingValueEntry = {
  key: string;
  value?: string | null;
  hasValue: boolean;
  isSecret: boolean;
  source: "env_file" | "default" | "runtime" | "unset";
  effectiveValue?: string | null;
  effectiveHasValue: boolean;
  effectiveSource: "env_file" | "default" | "runtime" | "unset";
  pendingRestart: boolean;
};

export type SettingsSnapshot = {
  revision: string;
  generatedAt: string;
  envPath: string;
  schema: SettingsSchema;
  values: SettingValueEntry[];
  pendingRestartKeys: string[];
};

export type SettingsValidationResponse = {
  valid: boolean;
  fieldErrors: Record<string, string[]>;
  formErrors: string[];
  restartRequiredKeys: string[];
};

export type ClientServerReleaseValidationStatus =
  | "valid"
  | "not_found"
  | "incomplete_assets"
  | "commit_mismatch"
  | "unavailable";

export type ClientServerReleaseValidation = {
  release: string;
  valid: boolean;
  status: ClientServerReleaseValidationStatus;
  message: string;
  tagName?: string;
  targetCommitish?: string;
  missingAssets?: string[];
  checkedAt: string;
};

export type SettingsUpdateResponse = {
  updatedKeys: string[];
  unchangedKeys: string[];
  restartRequiredKeys: string[];
  dynamicGroupsRefreshed: string[];
  envPath: string;
  revision: string;
};

export type ClientNotificationDelivery = {
  id: string;
  channel: string;
  deliveryKind: string;
  eventKind: string;
  eventCount: number;
  targetMasked: string;
  status: string;
  failureKind?: string | null;
  blockedReasonCode?: string | null;
  attempts: number;
  createdAt: string;
  nextAttemptAt?: string | null;
  sentAt?: string | null;
  errorMessage?: string | null;
};

export type ClientNotificationDeliveriesResponse = {
  deliveries: ClientNotificationDelivery[];
};

export type ClientChatDelivery = {
  id: string;
  roomId: string;
  installationId: string;
  clientLabel: string;
  recipientMasked: string;
  messageCount: number;
  status: string;
  attempts: number;
  createdAt: string;
  nextAttemptAt?: string | null;
  sentAt?: string | null;
  errorMessage?: string | null;
};

export type ClientChatDeliveriesResponse = {
  deliveries: ClientChatDelivery[];
};

export type AdminAuditEntry = {
  id: string;
  actorEmail?: string | null;
  action: string;
  payloadJson?: string | null;
  ip?: string | null;
  createdAt: string;
};

export type AdminAuditResponse = {
  entries: AdminAuditEntry[];
};

export type VersionResponse = {
  version: string;
  commit: string;
  buildTime: string;
  binaryPath: string;
  rollbackPath: string;
  rollbackAvailable: boolean;
  uptimeSecs: number;
  service: {
    manager: "systemd" | "nohup";
    active: boolean;
    unitName?: string | null;
    activeState?: string | null;
    unitFileState?: string | null;
  };
  latest: {
    binaryUrl: string;
    available: boolean;
    etag?: string | null;
    contentLength?: number | null;
    error?: string | null;
  };
};

export type MetricsHealth = "healthy" | "warning" | "critical";

export type MetricEvent = {
  id?: number | null;
  timestamp: number;
  severity: "info" | "warning" | "critical" | string;
  kind: string;
  message: string;
  details?: Record<string, unknown>;
};

export type AlertIncident = {
  id: string;
  fingerprint: string;
  scope: string;
  kind: string;
  entityKind: string;
  entityId?: string | null;
  severity: "info" | "warning" | "critical" | string;
  status: "firing" | "acknowledged" | "silenced" | "resolved" | string;
  title: string;
  message: string;
  details: Record<string, unknown>;
  occurrenceCount: number;
  startedAt: number;
  lastSeenAt: number;
  lastTransitionAt: number;
  resolvedAt?: number | null;
  acknowledgedAt?: number | null;
  acknowledgedBy?: string | null;
  acknowledgementNote?: string | null;
  silencedAt?: number | null;
  silencedUntil?: number | null;
  silencedBy?: string | null;
  silenceNote?: string | null;
};

export type AlertDelivery = {
  id: string;
  incidentId: string;
  transitionId: string;
  channel: string;
  status: string;
  attempts: number;
  providerMessageId?: string | null;
  nextAttemptAt?: number | null;
  lastError?: string | null;
  createdAt: number;
  updatedAt: number;
  sentAt?: number | null;
};

export type AlertChannelState = {
  channel: string;
  enabled: boolean;
  configured: boolean;
  status:
    "disabled" | "misconfigured" | "ready" | "healthy" | "degraded" | string;
  lastAttemptAt?: number | null;
  lastSuccessAt?: number | null;
  lastError?: string | null;
  failureCode?: string | null;
  failureHint?: string | null;
  failureDetails?: Record<string, unknown> | null;
};

export type AlertChannelTestResponse = {
  ok: boolean;
  channel: string;
  providerMessageId?: string | null;
  testedAt: number;
};

export type UserNotificationChannelState = {
  channel: string;
  enabled: boolean;
  configured: boolean;
  status:
    "disabled" | "misconfigured" | "reconciling" | "ready" | "healthy" | "degraded" | string;
  runtimeReady: boolean;
  transportStatus?: string;
  providerLabel?: string | null;
  runtimeVerifiedAt?: string | null;
  lastAttemptAt?: string | null;
  lastSuccessAt?: string | null;
  lastError?: string | null;
  failureCode?: string | null;
  failureHint?: string | null;
  failureDetails?: Record<string, unknown> | null;
  lastFailureAt?: string | null;
  testTargetAvailable: boolean;
  testTargetLabel?: string | null;
  bindingVerifiedAt?: string | null;
};

export type UserNotificationChannelTestResponse = {
  ok: boolean;
  channel: string;
  targetLabel?: string | null;
  providerMessageId?: string | null;
  testedAt: string;
};

export type AlertingOverview = {
  activeCount: number;
  criticalCount: number;
  resolvedCount: number;
  failedDeliveryCount: number;
  incidents: AlertIncident[];
  deliveries: AlertDelivery[];
  channels: AlertChannelState[];
};

export type DiskUsage = {
  label: string;
  mountPoint: string;
  usedBytes: number;
  totalBytes: number;
};

export type HostMetricsInfo = {
  hostname?: string | null;
  osName?: string | null;
  osVersion?: string | null;
  kernelVersion?: string | null;
  arch: string;
  cpuBrand?: string | null;
  cpuCores: number;
  memoryTotalBytes?: number | null;
  disks: Array<{ name: string; mountPoint: string; totalBytes: number }>;
};

export type HostMetricsStatus = {
  timestamp: number;
  uptimeSecs?: number | null;
  cpuPercent?: number | null;
  load1?: number | null;
  load5?: number | null;
  load15?: number | null;
  memoryUsedBytes?: number | null;
  memoryTotalBytes?: number | null;
  memoryAvailableBytes?: number | null;
  swapUsedBytes?: number | null;
  swapTotalBytes?: number | null;
  disks: DiskUsage[];
  network: {
    rxBytesPerSec?: number | null;
    txBytesPerSec?: number | null;
    tcpEstablished?: number | null;
    tcpTimeWait?: number | null;
  };
  process: {
    openFds?: number | null;
    maxFds?: number | null;
    fdUsagePercent?: number | null;
    threads?: number | null;
    rssBytes?: number | null;
    cpuPercent?: number | null;
    uptimeSecs?: number | null;
  };
};

export type ClockSourceResult = {
  url: string;
  ok: boolean;
  offsetMs?: number | null;
  rttMs?: number | null;
  error?: string | null;
};

export type ClockHealthStatus = {
  enabled: boolean;
  status:
    | "healthy"
    | "warning"
    | "critical"
    | "degraded"
    | "unknown"
    | "disabled"
    | string;
  direction: "ahead" | "behind" | "aligned" | "unknown" | string;
  confidence: "quorum" | "single_source" | "unavailable" | string;
  offsetMs?: number | null;
  uncertaintyMs?: number | null;
  validSources: number;
  totalSources: number;
  ntpSynchronized?: boolean | null;
  sampledAt?: number | null;
  lastSuccessAt?: number | null;
  probeAgeSecs?: number | null;
  ingressExpiredTotal: number;
  ingressFutureTotal: number;
  ingressContractErrorTotal: number;
  sources: ClockSourceResult[];
};

export type RouterMetricsStatus = {
  activeRoutes: number;
  pendingRoutes: number;
  healthProbeFailureCache: number;
  sshActiveSessions: number;
  sshForwardListeners: number;
  sshForwardListenerCreatedTotal: number;
  sshForwardListenerShutdownTotal: number;
  sshForwardBindErrorsTotal: number;
  sshForwardAcceptErrorsTotal: number;
  sshForwardEmfileErrorsTotal: number;
  sshPendingChannelOpens: number;
  sshChannelOpenStartedTotal: number;
  sshChannelOpenSucceededTotal: number;
  sshChannelOpenExplicitFailuresTotal: number;
  sshChannelOpenTimeoutTotal: number;
  sshChannelOpenSessionErrorsTotal: number;
  sshChannelOpenCancelledTotal: number;
  sshActiveBridges: number;
  sshBridgeCreatedTotal: number;
  sshBridgeCompletedTotal: number;
  sshBridgeCancelledTotal: number;
  sshBridgeWriteStallTotal: number;
  sshBridgeHalfCloseIdleTotal: number;
  sshBridgeIoErrorsTotal: number;
  sshForwardCapacityRejectedTotal: number;
  proxyInflight: number;
  proxyRequestsTotal: number;
  proxyUpstreamErrorsTotal: number;
  proxy5xxTotal: number;
  shareActiveRequests: number;
  shareOldestInflightAgeSecs: number;
  shareOldestProgressAgeSecs: number;
  shareRequestWatchdogForcedReleaseTotal: number;
  shareRequestManualReleaseTotal: number;
  proxyRequestBodyTimeoutTotal: number;
  proxyResponseHeaderTimeoutTotal: number;
  proxyDownstreamStallTimeoutTotal: number;
  proxyRequestHardTimeoutTotal: number;
  proxyStreamSemanticTerminalTotal: number;
  proxyStreamFirstEventTimeoutTotal: number;
  proxyStreamIdleTimeoutTotal: number;
  proxyStreamParserOverflowTotal: number;
  proxyStreamUpstreamErrorsTotal: number;
  healthProbeFailuresTotal: number;
  healthProbeCachedFailuresTotal: number;
  dbErrorsTotal: number;
};

export type ClientMetricsItem = {
  installationId: string;
  clientLabel: string;
  status: "online" | "recovering" | "offline" | "unknown" | string;
  monitoringEnabled: boolean;
  platform: string;
  appVersion: string;
  countryCode?: string | null;
  lastHeartbeatAt?: number | null;
  offlineSince?: number | null;
  lastRecoveredAt?: number | null;
  offlineEpisode: number;
};

export type ClientMetricsSnapshot = {
  timestamp: number;
  total: number;
  monitored: number;
  online: number;
  recovering: number;
  offline: number;
  unknown: number;
  items: ClientMetricsItem[];
};

export type LlmMetricsSnapshot = {
  rpm: number;
  tpm: number;
  inputTpm: number;
  outputTpm: number;
  inflight: number;
  errorRate: number;
  rateLimitPerMinute: number;
  p95LatencyMs?: number | null;
  p95TtftMs?: number | null;
  averageTtftMs?: number | null;
  averageTps?: number | null;
  ttftSampleCount: number;
  tpsSampleCount: number;
  activeModels: number;
  activeShares: number;
  failoverSuccessRate?: number | null;
  cacheHitRate?: number | null;
};

export type MetricsSnapshot = {
  status: MetricsHealth;
  sampledAt: number;
  enabled: boolean;
  sampleIntervalSecs: number;
  lastPersistedAt?: number | null;
  clock: ClockHealthStatus;
  host: HostMetricsStatus;
  router: RouterMetricsStatus;
  clients: ClientMetricsSnapshot;
  llm: LlmMetricsSnapshot;
  alerts: MetricEvent[];
  incidents: AlertIncident[];
};

export type ClockMetricsPoint = {
  timestamp: number;
  offsetMs?: number | null;
  uncertaintyMs?: number | null;
  validSources?: number | null;
};

export type HostMetricsPoint = {
  timestamp: number;
  cpuPercent?: number | null;
  memoryUsagePercent?: number | null;
  diskUsagePercent?: number | null;
  fdUsagePercent?: number | null;
  rxBytesPerSec?: number | null;
  txBytesPerSec?: number | null;
  processRssBytes?: number | null;
};

export type RouterMetricsPoint = {
  timestamp: number;
  activeRoutes: number;
  forwardListeners: number;
  proxyInflight: number;
  proxyUpstreamErrorsTotal: number;
  healthProbeFailuresTotal: number;
  dbErrorsTotal: number;
};

export type ClientMetricsPoint = {
  timestamp: number;
  total: number;
  online: number;
  recovering: number;
  offline: number;
  unknown: number;
};

export type LlmMetricsPoint = {
  timestamp: number;
  rpm: number;
  tpm: number;
  inputTpm: number;
  outputTpm: number;
  errorRate: number;
  rateLimited: number;
  concurrencyLimited: number;
  p95LatencyMs?: number | null;
  p95TtftMs?: number | null;
  averageTtftMs?: number | null;
  averageTps?: number | null;
  ttftSampleCount: number;
  tpsSampleCount: number;
};

export type MetricsSeriesResponse = {
  range: string;
  step: string;
  clock: ClockMetricsPoint[];
  host: HostMetricsPoint[];
  router: RouterMetricsPoint[];
  clients: ClientMetricsPoint[];
  llm: LlmMetricsPoint[];
};

export type LlmTopResponse = {
  range: string;
  by: string;
  items: Array<{
    key: string;
    requests: number;
    totalTokens: number;
    errors: number;
    errorRate: number;
    p95LatencyMs?: number | null;
    averageTtftMs?: number | null;
    averageTps?: number | null;
    ttftSampleCount: number;
    tpsSampleCount: number;
    lastRequestAt?: number | null;
  }>;
};

export type LlmReliabilityResponse = {
  range: string;
  totalRequests: number;
  substitutedRequests: number;
  substitutionRate: number;
  substitutionSuccessRate?: number | null;
  items: Array<{
    requestedModel: string;
    actualModel: string;
    requests: number;
    errors: number;
    errorRate: number;
  }>;
};

export type ClearMetricsResponse = {
  ok: boolean;
  deletedRows: Record<string, number>;
};

export type ClientChatMessage = {
  id: string;
  seq: number;
  body: string;
  authorLabel: string;
  authorKind: "user" | "system" | string;
  messageKind: "text" | "market_event" | string;
  eventType?: string;
  eventPayload?: Record<string, unknown>;
  isMine: boolean;
  status: "visible" | "deleted" | string;
  createdAt: string;
};

export type ClientChatMessagePreview = {
  seq: number;
  body: string;
  authorLabel: string;
  authorKind: "user" | "system" | string;
  messageKind: "text" | "market_event" | string;
  eventType?: string;
  eventPayload?: Record<string, unknown>;
  createdAt: string;
};

export type ClientChatRoom = {
  id: string;
  installationId: string;
  clientLabel: string;
  status: "active" | "archived" | string;
  canPost: boolean;
  readOnly: boolean;
  latestSeq: number;
  unreadCount: number;
  lastMessageAt?: string | null;
  lastMessage?: ClientChatMessagePreview | null;
  archivedAt?: string | null;
};

export type ClientChatRoomListResponse = {
  rooms: ClientChatRoom[];
  totalUnread: number;
};

export type ClientChatMessageListResponse = {
  messages: ClientChatMessage[];
  latestSeq: number;
  hasMore: boolean;
};

export type ClientChatVisit = {
  installationId: string;
  lastReadSeq: number;
};

// P18: test-connection types
export type ShareConnectionTestRequest = {
  app: "claude" | "codex" | "gemini";
  operation?: "text" | "image_generation" | "image_edit" | "video_generation";
  timeoutMs?: number;
};

export type ShareConnectionTestResponse = {
  success: boolean;
  request: {
    method: string;
    url: string;
    headers: [string, string][];
    body: string | null;
  };
  response: {
    statusCode: number;
    statusText: string;
    headers: [string, string][];
    bodyText: string;
    bodyTruncated: boolean;
  } | null;
  durationMs: number;
  error: string | null;
  terminalEvent?: string | null;
  schedulingRecovery?: {
    shareModelHealthDeleted: number;
    gatewayModelFailuresDeleted: number;
    gatewayRuntimeStatesDeleted: number;
  };
};

export type ShareModelHealthCalendarDay = {
  date: string;
  active: boolean;
  expectedChecks: number;
  completedChecks: number;
  successfulChecks: number;
  observedChecks: number;
  upstreamFailureChecks: number;
  monitoringGapChecks: number;
  successRate?: number;
  coverageRate?: number;
  mixedEpoch: boolean;
  evidenceVersion: number;
};

export type ShareModelHealthProbeEpoch = {
  epochId: string;
  startsAt: number;
  endsAt?: number;
  appType: "claude" | "codex" | "gemini" | string;
  apiType: "anthropic" | "openai" | "gemini" | string;
  providerId: string;
  providerName?: string;
  requestedModel: string;
  wireModel: string;
  policyMode?: "passthrough" | "single" | string;
  evidenceVersion: number;
};

export type ClientOnlineCalendarDay = {
  date: string;
  onlineMinutes: number;
  observedMinutes: number;
  onlineRate?: number;
};

export type ClientOnlineCalendar = {
  installationId: string;
  timezone: "UTC" | string;
  startDate: string;
  endDate: string;
  days: ClientOnlineCalendarDay[];
};

export type ShareModelHealthCalendar = {
  shareId: string;
  timezone: "UTC" | string;
  expectedChecksPerFullDay: number;
  startDate: string;
  endDate: string;
  days: ShareModelHealthCalendarDay[];
  epochs: ShareModelHealthProbeEpoch[];
  currentProbe?: ShareModelHealthProbeEpoch;
  sharedProbe: boolean;
  evidenceVersion: number;
};

export type ShareUsageRefreshRequest = {
  app?: "claude" | "codex" | "gemini";
};

export type ShareUsageRefreshResponse = {
  ok: boolean;
  refreshed: Array<{
    app: string;
    providerId?: string | null;
    providerName?: string | null;
    authProvider?: string | null;
    refreshed: boolean;
    error?: string | null;
  }>;
};

export type ClientMarketHostStatus =
  | "idle"
  | "reserved"
  | "allocated"
  | "locked"
  | "draining"
  | "disabled"
  | "unreachable"
  | "abnormal";

export type HostIpIntel = {
  query: string;
  ip?: string;
  location?: string;
  score?: number;
  level?: string;
  riskScore?: number;
  riskLevel?: string;
  confidence?: number;
  countryCode: string;
  country?: string;
  region?: string;
  city?: string;
  latitude?: number;
  longitude?: number;
  timezone?: string;
  asn?: string;
  asName?: string;
  isp?: string;
  owner?: string;
  networkType?: string;
  classificationType?: string;
  proxy?: boolean;
  vpn?: boolean;
  hosting?: boolean;
  tor?: boolean;
  source: string;
};

export type ClientMarketConnection = {
  state: "online" | "reconnecting" | "offline" | "disabled" | string;
  since?: string;
  lastHeartbeatAt?: string;
};

export type ClientMarketHost = {
  id: string;
  providerId?: string;
  ip?: string;
  port?: number;
  hostOwnerEmail: string;
  dailyRateMinor?: number;
  currency?: "USD";
  freeDurationDays?: number;
  offerRevision: number;
  paymentMethodKinds: string[];
  contacts?: PaymentContact[];
  sellerApprovalRequired?: boolean;
  eligibility: MarketEligibility;
  countryCode?: string;
  hostname?: string;
  sshHostKeyFingerprint?: string;
  status: ClientMarketHostStatus | string;
  clientSubdomain?: string;
  clientOwnerEmail?: string;
  installationId?: string;
  canWebTerminal?: boolean;
  isHostOwner?: boolean;
  isClientOwner?: boolean;
  canRetireUnreachable?: boolean;
  clientConnection?: ClientMarketConnection;
  lastVerifiedAt?: string;
  lastError?: string;
  note?: string;
  ipIntel?: HostIpIntel;
  createdAt?: string;
  updatedAt?: string;
};

export type ClientMarketSshHostKeyInspection = {
  hostId: string;
  endpoint: string;
  storedFingerprint?: string;
  observedFingerprint: string;
  observedKeyType: string;
  changed: boolean;
  confirmationRequired: boolean;
};

export type ClientMarketSshHostKeyRotationResponse = {
  host: ClientMarketHost;
  inspection: ClientMarketSshHostKeyInspection;
};

export type SupplySummaryEntry = {
  hostOwnerEmail: string;
  countryCode?: string;
  idleCount: number;
  totalCount: number;
};

export type ProvisionSshKey = {
  publicKey: string;
  authorizedKeysLine: string;
};

export type ProvisioningJobStatus =
  "pending" | "running" | "succeeded" | "failed";

export type ProvisioningJob = {
  id: string;
  jobType: string;
  hostId?: string;
  hostOwnerEmail?: string;
  clientOwnerEmail?: string;
  subdomain?: string;
  installationId?: string;
  status: ProvisioningJobStatus;
  phase: string;
  failureCode?: string;
  countryCode?: string;
  clientUrl?: string;
  log: string;
  createdAt: string;
  updatedAt: string;
};

export type CreateClientMarketClientResponse = {
  jobId: string;
};

export type ClientMarketPaymentMethod = {
  kind: "alipay" | "wechat" | "binance" | "crypto" | "custom" | string;
  account?: string;
  qrImageUrl?: string;
  assetUrl?: string;
  token?: "USDT" | "USDC" | string;
  chain?: "bsc" | "base" | "eth" | "tron" | string;
  address?: string;
  instructions?: string;
};

export type PaymentContactChannel = "wechat" | "telegram" | "custom";

export type PaymentContact = {
  channel: PaymentContactChannel | string;
  handle: string;
};

export type AccountPaymentProfile = {
  providerId: string;
  ownerEmail: string;
  methods: ClientMarketPaymentMethod[];
  contacts?: PaymentContact[];
  updatedAt: string;
};

export type MarketBillingSupplierProfile = {
  currency: "USD";
  settlementGraceHours: number;
  revision: number;
  updatedAt: string;
};

export type MarketBillingService = {
  id: string;
  productKind: "share" | "client_host" | string;
  productRef: string;
  serviceRef: string;
  serviceLabel: string;
  status: string;
  healthState: string;
  dailyRateMinor: number;
  offerRevision: number;
  trialSecondsRemaining: number;
  activatedAt: string;
  suspendedAt?: string;
  terminatedAt?: string;
};

export type MarketBillingInvoiceLine = {
  id: string;
  contractId: string;
  productKind: string;
  productRef: string;
  serviceRef: string;
  serviceLabel: string;
  dailyRateMinor: number;
  billableSeconds: number;
  amountMinor: number;
  amountUsdMinor: number;
  amountCnyMinor: number;
  serviceStartedAt: string;
  serviceEndedAt: string;
  evidence: Record<string, unknown>;
};

export type MarketBillingPaymentDeclaration = {
  id: string;
  status: string;
  paymentMethodKind?: string;
  paymentReference?: string;
  note?: string;
  evidenceUrl?: string;
  declaredAt: string;
  rejectedAt?: string;
  rejectionReason?: string;
};

export type MarketBillingDispute = {
  id: string;
  reason: string;
  status: string;
  resolution?: string;
  createdAt: string;
  respondBy?: string;
  escalatedAt?: string;
  autoResolveAt?: string;
  resolvedAt?: string;
};

export type MarketBillingCreditNote = {
  id: string;
  kind: "service_credit" | "external_refund" | string;
  amountMinor: number;
  amountUsdMinor: number;
  amountCnyMinor: number;
  currency: "USD";
  reason: string;
  externalReference?: string;
  status: string;
  createdByEmail: string;
  createdAt: string;
};

export type MarketBillingInvoice = {
  id: string;
  sequence: number;
  status: string;
  amountMinor: number;
  amountUsdMinor: number;
  amountCnyMinor: number;
  usdCnyRateMicros: number;
  currency: "USD";
  dueAt: string;
  deadlineAt: string;
  openedAt: string;
  declaredAt?: string;
  paidAt?: string;
  paymentMethods: ClientMarketPaymentMethod[];
  contacts: PaymentContact[];
  paymentProfileUpdatedAt: string;
  lines: MarketBillingInvoiceLine[];
  declaration?: MarketBillingPaymentDeclaration;
  dispute?: MarketBillingDispute;
  creditNotes: MarketBillingCreditNote[];
};

export type MarketBillingInvoiceHistory = {
  invoices: MarketBillingInvoice[];
  nextBeforeSequence?: number | null;
};

export type MarketCreditAccount = {
  id: string;
  buyerUserId: string;
  buyerEmail: string;
  supplierUserId: string;
  supplierEmail: string;
  currency: "USD";
  status: string;
  balanceMinor: number;
  creditKind: "none" | "limited" | "unlimited" | string;
  creditLimitMinor?: number;
  utilizationBps?: number;
  dailyRateMinor: number;
  estimatedSettlementAt?: string;
  isBuyer: boolean;
  isSupplier: boolean;
  canSettle: boolean;
  canClose: boolean;
  closeRequested: boolean;
  services: MarketBillingService[];
  openInvoice?: MarketBillingInvoice;
  createdAt: string;
  updatedAt: string;
};

export type MarketCreditRestriction = {
  id: string;
  invoiceId: string;
  reason: string;
  createdAt: string;
};

export type MarketBillingDashboard = {
  accounts: MarketCreditAccount[];
  supplierProfiles: MarketBillingSupplierProfile[];
  restrictions: MarketCreditRestriction[];
  refundObligations: ShareMarketRefundObligation[];
  trialHours: number;
  usdCnyRateMicros: number;
};

export type MarketBillingConfig = {
  currency: "USD";
  usdCnyRateMicros: number;
};

export type AdminMarketBillingDispute = {
  dispute: MarketBillingDispute;
  accountId: string;
  buyerEmail: string;
  supplierEmail: string;
  invoice: MarketBillingInvoice;
};

export type MarketAccessProductKind = "share" | "client_host";
export type MarketAccessPricingKind = "free" | "paid";
export type MarketAccessMode = "whitelist" | "blacklist";
export type MarketAccessDecision = "inherit" | "allow" | "deny";
export type MarketCreditKind = "none" | "limited" | "unlimited";

export type MarketEligibilityStatus =
  | "allowed"
  | "login_required"
  | "access_required"
  | "credit_required"
  | "buyer_restricted"
  | "settlement_required"
  | "credit_limit_reached"
  | "relationship_closed"
  | string;

export type MarketAccessRequestSummary = {
  id: string;
  status: "requested" | string;
  revision: number;
  requestedAt: string;
};

export type MarketEligibility = {
  allowed: boolean;
  status: MarketEligibilityStatus;
  request?: MarketAccessRequestSummary;
};

export type MarketAccessPolicy = {
  productKind: MarketAccessProductKind;
  pricingKind: MarketAccessPricingKind;
  mode: MarketAccessMode;
  revision: number;
  riskAcknowledgedAt?: string;
  updatedAt: string;
};

export type MarketCounterpartyAccessRule = {
  productKind: MarketAccessProductKind;
  pricingKind: MarketAccessPricingKind;
  decision: MarketAccessDecision;
};

export type MarketCreditLine = {
  currency: "USD";
  kind: MarketCreditKind;
  limitMinor?: number;
  revision: number;
  updatedAt: string;
};

export type MarketCounterpartyExposure = {
  currency: "USD";
  balanceMinor: number;
  status: string;
  activeServiceCount: number;
};

export type MarketCounterparty = {
  id: string;
  buyerEmail: string;
  buyerUserId?: string;
  status: "active" | "revoked" | string;
  revision: number;
  accessRules: MarketCounterpartyAccessRule[];
  creditLines: MarketCreditLine[];
  exposures: MarketCounterpartyExposure[];
  createdAt: string;
  updatedAt: string;
};

export type MarketAccessRequest = {
  id: string;
  supplierUserId: string;
  supplierEmail: string;
  buyerUserId: string;
  buyerEmail: string;
  productKind: MarketAccessProductKind;
  pricingKind: MarketAccessPricingKind;
  targetKind: "share_seat" | "client_host" | string;
  targetId: string;
  targetLabel: string;
  dailyRateMinor?: number;
  currency?: "USD";
  status: "requested" | "approved" | "rejected" | "cancelled" | string;
  revision: number;
  requestedAt: string;
  resolvedAt?: string;
  resolvedByUserId?: string;
  resolutionReason?: string;
  resolutionNote?: string;
};

export type MarketPublicCreditLine = {
  currency: "USD";
  limitMinor?: number;
  enabled: boolean;
  revision: number;
  updatedAt: string;
};

export type MarketAccessDashboard = {
  policies: MarketAccessPolicy[];
  counterparties: MarketCounterparty[];
  accessRequests: MarketAccessRequest[];
  publicCreditLines: MarketPublicCreditLine[];
};

export type MarketAccessInboxSummary = {
  pendingRequests: number;
};

export type ClientMarketProviderCountry = {
  code: string;
  idle: number;
  total: number;
  freeIdle: number;
  freeTotal: number;
};

export type ClientMarketProvider = {
  providerId: string;
  ownerEmail: string;
  official: boolean;
  joinedAt: string;
  offerStableSince: string;
  hostTotal: number;
  idleTotal: number;
  allocatedTotal: number;
  allocationRate: number;
  freeHostTotal: number;
  freeAllocatedTotal: number;
  paidHostTotal: number;
  paidAllocatedTotal: number;
  externalClientOwnerTotal: number;
  externalClientsOver3Days: number;
  externalClientsOver30Days: number;
  onlineRate30d?: number;
  anomalousHostRate: number;
  minDailyRateMinor?: number;
  maxDailyRateMinor?: number;
  successfulAllocations: number;
  paymentMethodKinds: string[];
  countries: ClientMarketProviderCountry[];
};

export type ClientMarketProviderSupply = {
  routerOwnerEmail?: string;
  officialProviderId?: string;
  providers: ClientMarketProvider[];
};

export type ClientMarketQuoteItem = {
  id: string;
  hostId: string;
  providerId: string;
  hostOwnerEmail: string;
  countryCode?: string;
  hostname?: string;
  ip?: string;
  dailyRateMinor?: number;
  currency?: string;
  freeDurationDays?: number;
  offerRevision: number;
};

export type ClientMarketAllocationQuote = {
  id: string;
  status: string;
  expiresAt: string;
  items: ClientMarketQuoteItem[];
};

export type ClientMarketCommitQuoteResponse = {
  batchId: string;
  jobIds: string[];
};

export type ClientMarketBatch = {
  id: string;
  quoteId: string;
  status: "running" | "succeeded" | "partial_failed" | "failed" | string;
  createdAt: string;
  updatedAt: string;
  jobs: ProvisioningJob[];
};

export type ClientMarketRental = {
  installationId: string;
  hostId: string;
  providerId: string;
  hostOwnerEmail: string;
  clientOwnerEmail: string;
  status:
    | "active"
    | "billing_suspended"
    | "releasing"
    | "release_failed"
    | "released"
    | string;
  dailyRateMinor?: number;
  currency?: "USD";
  freeDurationDays?: number;
  offerRevision: number;
  activatedAt?: string;
  expiresAt?: string;
  paymentMethodKinds: string[];
  contacts?: PaymentContact[];
  isClientOwner: boolean;
  canRelease: boolean;
  canFinalizeRelease: boolean;
  /** Pending/running cleanup job — used to resume progress UI after refresh. */
  activeCleanupJobId?: string;
  providerTerminalAuthorizedUntil?: string;
  providerTerminalAccessActive: boolean;
  canManageProviderTerminal: boolean;
  updatedAt: string;
};

export type ClientMarketHostUsageHistoryEntry = {
  installationId: string;
  clientOwnerEmail: string;
  clientSubdomain?: string;
  status: string;
  startedAt: string;
  endedAt?: string;
  dailyRateMinor?: number;
  currency?: string;
  chargesMinor: number;
  unbilledMinor: number;
  invoicedMinor: number;
};

export type ClientMarketHostTransferDocument = {
  version: number;
  exportedAt?: string;
  hosts: Array<{
    ip: string;
    port: number;
    note?: string;
    dailyRateMinor?: number;
    currency?: "USD";
    freeDurationDays?: number;
    expectedFingerprint?: string;
    informationalStatus?: string;
  }>;
};

export type ClientMarketHostImportResponse = {
  jobId: string;
  status: "pending" | "running" | "completed" | string;
  imported: number;
  skipped: number;
  failed: number;
  items: Array<{
    ip: string;
    port: number;
    status: string;
    hostId?: string;
    error?: string;
  }>;
};

export type ClientTunnelSubdomainAvailability = {
  available: boolean;
  reason?: string;
};

export type CreateClientSelectionPersist = {
  mode: "official_default" | "custom";
  providerIds: string[];
};

export type CreateClientRegionsPersist = {
  mode: "all" | "subset";
  codes: string[];
};

export type ShareMarketSeatInput = {
  parallelLimit?: number;
  tokenLimit?: number;
  tokenPeriod: ShareTokenPeriod;
  dailyRateMinor?: number;
  currency?: "USD";
  serviceDurationDays?: number;
  trialHours?: number;
  trialTokenLimit?: number;
};

export type ShareMarketSubscription = {
  id: string;
  seatId: string;
  listingId: string;
  shareId: string;
  installationId: string;
  shareName: string;
  appType: string;
  apps: string[];
  subdomain?: string;
  shareOnline?: boolean;
  ownerEmail: string;
  renterEmail?: string;
  status: string;
  integrityState: "compatible" | "violated" | "remediating" | "terminated" | string;
  integrityReason?: string;
  integrityViolatedAt?: string;
  terminationAdjustment?: ShareMarketTerminationAdjustmentSummary;
  seatPosition: number;
  parallelLimit?: number;
  tokenLimit?: number;
  tokenPeriod: ShareTokenPeriod;
  dailyRateMinor?: number;
  currency?: "USD";
  serviceDurationDays?: number;
  trialHours?: number;
  trialTokenLimit?: number;
  offerRevision: number;
  activatedAt?: string;
  serviceStartedAt?: string;
  expiresAt?: string;
  paymentMethodKinds: string[];
  contacts?: PaymentContact[];
  canRelease: boolean;
  canForceRevoke: boolean;
  canRetryGrant: boolean;
  canProposePriceChange: boolean;
  priceChange?: ShareMarketPriceChange;
  releaseReason?: string;
  failureCode?: string;
  grantAttempts?: number;
  releasedAt?: string;
  createdAt: string;
  updatedAt: string;
};

export type ShareMarketTerminationCalculation = {
  contractId: string;
  accountId: string;
  productKind: string;
  productRef: string;
  currency: "USD" | string;
  serviceStartedAt: string;
  serviceEndsAt: string;
  evaluatedAt: string;
  elapsedBps: number;
  refundBps: number;
  refundableBaseUnits: number;
  amountUnits: number;
  amountMinor: number;
};

export type ShareMarketRefundObligation = {
  id: string;
  adjustmentId: string;
  invoiceId: string;
  amountMinor: number;
  currency: "USD" | string;
  status: string;
  dueAt: string;
  externalReference?: string;
  recordedAt?: string;
  canRecord: boolean;
};

export type ShareMarketTerminationAdjustmentSummary = {
  id: string;
  status: "applied" | "refund_due" | "settled" | string;
  currency: "USD" | string;
  elapsedBps: number;
  refundBps: number;
  amountMinor: number;
  unbilledCreditMinor: number;
  invoiceCreditMinor: number;
  externalRefundMinor: number;
  refundObligationStatus?: "pending" | "recorded" | string;
};

export type ShareMarketTerminationQuote = {
  id: string;
  subscriptionId: string;
  status: "active" | "consumed" | "expired";
  expiresAt: string;
  calculation: ShareMarketTerminationCalculation;
};

export type ShareMarketTerminationAdjustment = ShareMarketTerminationAdjustmentSummary & {
  calculation: ShareMarketTerminationCalculation;
  obligations: ShareMarketRefundObligation[];
};

export type ShareMarketPriceChange = {
  id: string;
  previousDailyRateMinor: number;
  proposedDailyRateMinor: number;
  currency: "USD";
  baseOfferRevision: number;
  status: "pending" | "accepted";
  canAccept: boolean;
  canReject: boolean;
  canCancel: boolean;
  createdAt: string;
  updatedAt: string;
  respondedAt?: string;
};

export type ShareMarketSeat = ShareMarketSeatInput & {
  id: string;
  position: number;
  status: string;
  offerRevision: number;
  isFree: boolean;
  canRent: boolean;
  rentPrerequisitesMet: boolean;
  sellerApprovalRequired: boolean;
  rentBlockReason?: string;
  eligibility: MarketEligibility;
  readOnly: boolean;
  canDelete: boolean;
  deleteBlockedReason?: string;
  canRepublish: boolean;
  retiredAt?: string;
  subscription?: ShareMarketSubscription;
};

export type ShareMarketProviderFamily =
  | "anthropic"
  | "openai"
  | "google"
  | "xai"
  | "cursor"
  | "kiro"
  | "copilot"
  | "api"
  | "multi"
  | "other";

export type ShareMarketProviderHealthState =
  | "healthy"
  | "degraded"
  | "unavailable"
  | "unknown";

export type ShareMarketProviderQuota = {
  status?: string;
  plan?: string;
  subscriptionPeriodEnd?: string;
  tiers: Array<{
    label: string;
    utilization: number;
    resetsAt?: string;
    used?: number;
    limit?: number;
    unit?: string;
  }>;
};

export type ShareMarketAppCapability = {
  app: string;
  providerFamily: ShareMarketProviderFamily;
  providerName?: string;
  providerType?: string;
  subscriptionLevel?: string;
  modelMode: "fixed" | "passthrough" | "unknown";
  upstreamModel?: string;
  models?: string[];
  available?: boolean;
  healthState: ShareMarketProviderHealthState;
  accountHint?: string;
  quota?: ShareMarketProviderQuota;
};

export type ShareMarketPerformance = {
  averageTtftMs?: number;
  averageTps?: number;
  recentRequestCount: number;
  ttftSampleCount: number;
  tpsSampleCount: number;
  latestSampleAt?: string;
  windowHours: number;
};

export type ShareMarketReliability = {
  onlineRate24h?: number;
  observedMinutes24h: number;
  observationCoverage24h: number;
  sufficientCoverage: boolean;
  latestObservedAt?: string;
};

export type ShareMarketListing = {
  id: string;
  shareId: string;
  installationId: string;
  shareName: string;
  appType: string;
  supportedApps: string[];
  providerFamily: ShareMarketProviderFamily;
  providerFamilies: ShareMarketProviderFamily[];
  appCapabilities: ShareMarketAppCapability[];
  ownerEmail: string;
  status: string;
  shareStatus: string;
  subdomain?: string;
  shareOnline: boolean;
  isOwner: boolean;
  canDelete: boolean;
  deleteBlockedReason?: string;
  canReopen: boolean;
  reopenBlockedReason?: string;
  reopenableSeatCount: number;
  contacts?: PaymentContact[];
  paymentMethodKinds: string[];
  performance: ShareMarketPerformance;
  reliability: ShareMarketReliability;
  tokenLimit?: number;
  parallelLimit?: number;
  tokensUsed?: number;
  supportedUserTokenPeriods: ShareTokenPeriod[];
  seats: ShareMarketSeat[];
  createdAt: string;
  updatedAt: string;
};

export type ShareMarketCatalog = {
  listings: ShareMarketListing[];
  trialHours: number;
};

export type ShareMarketRentAppService = {
  app: string;
  providerFamily: ShareMarketProviderFamily;
  providerType?: string;
  modelMode: "fixed" | "passthrough" | "unknown";
  upstreamModel?: string;
  models?: string[];
};

export type ShareMarketRentService = {
  schemaVersion: number;
  supportedApps: string[];
  apps: ShareMarketRentAppService[];
  shareParallelLimit?: number;
  shareTokenLimit?: number;
  shareTokensUsed: number;
};

export type ShareMarketRentQuote = {
  id: string;
  status: "active" | "consumed" | "expired";
  expiresAt: string;
  trialSecondsRemaining: number;
  offer: {
    seatId: string;
    listingId: string;
    shareId: string;
    shareName: string;
    ownerEmail: string;
    seatPosition: number;
    parallelLimit?: number;
    tokenLimit?: number;
    tokenPeriod: ShareTokenPeriod;
    dailyRateMinor?: number;
    currency?: "USD";
    serviceDurationDays?: number;
    trialHours?: number;
    trialTokenLimit?: number;
    offerRevision: number;
    service: ShareMarketRentService;
  };
};

export type ShareMarketOwnedListings = {
  listings: ShareMarketListing[];
};

export type ShareMarketSubscriptions = {
  subscriptions: ShareMarketSubscription[];
  nextCursor?: string;
};

export type ShareControlOperationSummary = {
  pending: number;
  dispatched: number;
  deadLettered: number;
};

export type ShareControlDeadLetter = {
  id: string;
  shareId: string;
  subscriptionId: string;
  action: "upsert" | "revoke" | string;
  email: string;
  attempts: number;
  lastError?: string;
  errorCode?: string;
  createdAt: string;
  updatedAt: string;
  deadLetteredAt: string;
  canRequeue: boolean;
};

export type ShareControlDeadLetterPage = {
  operations: ShareControlDeadLetter[];
  summary: ShareControlOperationSummary;
  nextCursor?: string;
};

export type ShareMarketOwnedShare = {
  shareId: string;
  shareName: string;
  appType: string;
  subdomain: string;
  ownerEmail: string;
  supportedApps: string[];
  appCapabilities?: ShareMarketAppCapability[];
  shareStatus: string;
  expiresAt?: string;
  parallelLimit?: number;
  tokenLimit?: number;
  alreadyListed: boolean;
  activeListingId?: string;
  reopenListingId?: string;
  hasActiveRentals: boolean;
  marketState: "listed" | "stopped" | "rented" | "public_access" | "inactive" | "available";
  canCreateListing: boolean;
  createBlockedReason?: string;
  freeAccess: boolean;
  supportedUserTokenPeriods: ShareTokenPeriod[];
};

export type ShareMarketReopenExistingSeatInput = {
  seatId: string;
  offerRevision: number;
  seat: ShareMarketSeatInput;
};

export type ShareMarketReopenListingInput = {
  existingSeats: ShareMarketReopenExistingSeatInput[];
  newSeats: ShareMarketSeatInput[];
};

export type ShareMarketReopenListingResponse = {
  ok: true;
  listingId: string;
  reopenedSeatIds: string[];
  newSeatIds: string[];
};
