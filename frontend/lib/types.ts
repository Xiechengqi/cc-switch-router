export type AuthUser = {
  id: string;
  email: string;
};

export type SessionStatus = {
  authenticated: boolean;
  user?: AuthUser;
  expiresAt?: string;
  installationOwnerEmail?: string;
  isAdmin: boolean;
};

export type MapViewportSettings = {
  visibleStartPx: number;
};

export type MapDisplaySettings = {
  showFlows: boolean;
  showHeat: boolean;
  viewport: MapViewportSettings;
};

export type MapDisplaySettingsUpdate = {
  showFlows?: boolean;
  showHeat?: boolean;
  viewport?: Partial<MapViewportSettings>;
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
    clients: MapPoint[];
  };
  mapDisplay: MapDisplaySettings;
  clients: DashboardClient[];
  /** 全量 share 列表；ClientBoard 按 installation 分组为横向卡片。 */
  shares?: ShareView[];
  markets?: DashboardMarket[];
  tickerShares?: DashboardTickerShare[];
  countryCounts?: Record<string, number>;
  userCountryCounts?: Record<string, number>;
  recentRequestEvents?: RecentRequestEvent[];
  marketRequestLogs?: MarketRequestLog[];
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

export type OperationalState = "available" | "online" | "degraded" | "offline" | "maintenance" | "disabled";

export type OperationalReasonCode =
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
  | "high_latency"
  | "edit_pending"
  | "edit_failed"
  | "maintenance_enabled"
  | "manually_disabled";

export type OperationalReason = {
  code: OperationalReasonCode | string;
  severity: "info" | "warning" | "critical" | string;
  startedAt?: string;
  entityType?: "client" | "share" | "market" | "provider" | string;
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

export type DashboardClient = {
  installation: {
    id: string;
    platform: string;
    appVersion: string;
    ownerEmail?: string;
    region?: string;
    countryCode?: string;
    createdAt: string;
    lastSeenAt: string;
  };
  clientTunnel?: {
    ownerEmail: string;
    subdomain: string;
    tunnelUrl: string;
    enabled: boolean;
    online: boolean;
  };
  payoutProfile?: {
    addressType: "evm";
    address: string;
    token: "USDC" | "USDT";
    networks: Array<"eip155:56" | "eip155:8453" | "eip155:42161">;
    verificationStatus: "self_declared";
    updatedAt: string;
  };
  /** 该 installation 名下所有独立 share 的 id 集合。 */
  shareIds?: string[];
  /** 该 installation 名下 share 总数；等价于 shareIds.length。 */
  shareCount?: number;
  onlineMinutes24h?: number;
  onlineRate24h?: number;
  healthChecks?: HealthCheckEntry[];
  healthTimeline?: HealthTimelineBucket[];
  operationalSummary?: OperationalSummary;
};

export type ShareSaleMarketKind = "token" | "share";

export type ShareView = {
  routerId?: string;
  shareId: string;
  shareName: string;
  ownerEmail?: string;
  sharedWithEmails?: string[];
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  marketLinks?: ShareMarketLink[];
  unknownMarketEmails?: string[];
  description?: string;
  forSale: string;
  saleMarketKind?: ShareSaleMarketKind;
  marketAccessMode: string;
  forSaleOfficialPricePercentByApp?: Record<string, number>;
  subdomain: string;
  canViewSecret?: boolean;
  canManage?: boolean;
  canEditSettings?: boolean;
  activeEdit?: ShareEditView;
  appType: string;
  providerId?: string;
  /** 唯一 app/provider binding（{app: provider_id}）。 */
  bindings?: Record<string, string>;
  tokenLimit: number;
  parallelLimit: number;
  tokensUsed: number;
  requestsCount: number;
  shareStatus: string;
  createdAt: string;
  expiresAt: string;
  isOnline: boolean;
  activeRequests: number;
  activeRequestsByApp?: Record<string, number>;
  tokensUsedByApp?: Record<string, number>;
  requestsCountByApp?: Record<string, number>;
  onlineMinutes24h?: number;
  onlineRate24h: number;
  recentRequests?: ShareRequestLog[];
  healthChecks?: HealthCheckEntry[];
  healthTimeline?: HealthTimelineBucket[];
  recentModelHealthChecks?: ShareModelHealthCheck[];
  support?: ShareSupport;
  appRuntimes?: ShareAppRuntimes;
  appProviders?: ShareAppProviders;
  modelHealth?: ShareModelHealthSummary;
  operationalSummary?: OperationalSummary;
};

export type ShareAppAccess = {
  sharedWithEmails: string[];
  marketAccessMode: "selected" | "all";
};

export type ShareAccessByApp = Partial<Record<"claude" | "codex" | "gemini", ShareAppAccess>>;

export type ShareAppSettings = {
  forSale?: "Yes" | "No" | "Free";
  saleMarketKind?: ShareSaleMarketKind;
  marketAccessMode?: "selected" | "all";
  sharedWithEmails?: string[];
  tokenLimit?: number;
  parallelLimit?: number;
  expiresAt?: string;
};

export type ShareAppSettingsByApp = Partial<Record<"claude" | "codex" | "gemini", ShareAppSettings>>;

export type ShareSettingsPatch = {
  ownerEmail?: string;
  description?: string | null;
  forSale?: "Yes" | "No" | "Free";
  saleMarketKind?: ShareSaleMarketKind;
  marketAccessMode?: "selected" | "all";
  sharedWithEmails?: string[];
  accessByApp?: ShareAccessByApp;
  appSettings?: ShareAppSettingsByApp;
  forSaleOfficialPricePercentByApp?: Record<string, number>;
  tokenLimit?: number;
  parallelLimit?: number;
  expiresAt?: string;
  autoStart?: boolean;
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

export type ShareMarketLink = {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  marketKind?: string;
  status: string;
  online: boolean;
  listingStatusByApp?: Record<string, ShareMarketListingStatus>;
};

export type ShareMarketListingStatus = {
  listingUrl?: string;
  status?: "idle" | "carpooling" | "full" | "unavailable" | "unknown" | string;
  saleMode?: "single" | "carpool" | string | null;
  filledSeats?: number | null;
  requiredSeats?: number | null;
  listingStatus?: string | null;
  updatedAt?: string | null;
  expiresAt?: string | null;
  isStale?: boolean;
};

export type DashboardMarket = {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  marketKind?: string;
  status: string;
  online: boolean;
  canManage?: boolean;
  maintenanceEnabled?: boolean;
  maintenanceMessage?: string;
  createdAt: string;
  updatedAt: string;
  lastSeenAt: string;
  offlineSince?: string;
  shareCount: number;
  onlineShareCount: number;
  activeRequests: number;
  parallelCapacity: number;
  onlineMinutes24h?: number;
  onlineRate24h: number;
  usageTokens: number;
  usageAmountUsd: string;
  pricingSummary?: Record<string, string | number | null>;
  healthChecks?: HealthCheckEntry[];
  healthTimeline?: HealthTimelineBucket[];
  linkedShares?: Array<{
    shareId: string;
    shareName: string;
    subdomain: string;
    ownerEmail?: string;
    appType: string;
    online: boolean;
    activeRequests: number;
    parallelLimit: number;
    onlineRate24h: number;
    disabledByMarket?: boolean;
    marketDisabledAt?: string;
    support?: ShareSupport;
    appRuntimes?: ShareAppRuntimes;
    appAvailability?: MarketAppAvailability;
    marketStates?: MarketShareRuntimeState[];
  }>;
  recentRequests?: MarketRequestLog[];
  operationalSummary?: OperationalSummary;
};

export type MarketShare = {
  routerId: string;
  shareId: string;
  subdomain: string;
  installationId: string;
  shareName: string;
  ownerEmail?: string;
  installationOwnerEmail?: string;
  appType: string;
  forSale: string;
  saleMarketKind?: ShareSaleMarketKind;
  marketAccessMode: string;
  shareStatus: string;
  online: boolean;
  activeRequests: number;
  parallelLimit: number;
  onlineRate24h: number;
  lastSeenAt: string;
  shareCreatedAt?: string;
  disabledByMarket?: boolean;
  marketDisabledAt?: string;
  support?: ShareSupport;
  appAvailability?: MarketAppAvailability;
  appRuntimes?: ShareAppRuntimes;
  modelHealth?: ShareModelHealthSummary;
  marketStates?: MarketShareRuntimeState[];
  signals?: ShareSignals;
  sessionLoad?: number;
};

export type ShareSessionLoad = {
  routerId: string;
  shareId: string;
  sessionLoad: number;
};

export type ShareSignals = {
  quotaHealth?: number;
  stability?: number;
  headroom?: number;
  samples10m?: number;
  ownerPenalty?: number;
};

export type MarketShareRuntimeState = {
  shareId: string;
  routerId?: string;
  scope: string;
  kind: string;
  appType?: string;
  modelId?: string;
  modelName?: string;
  reasonKind?: string;
  reason?: string;
  failureCount?: number;
  expiresAt?: string;
  updatedAt: string;
};

export type PublicMarket = {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  marketKind?: string;
  status: string;
  maintenanceEnabled?: boolean;
  maintenanceMessage?: string;
  pricingSummary?: unknown;
};

export type MarketsResponse = {
  markets: PublicMarket[];
};

export type ShareRequestLog = {
  requestId: string;
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
  statusCode: number;
  latencyMs: number;
  firstTokenMs?: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
  isStreaming?: boolean;
  isHealthCheck?: boolean;
  userEmail?: string;
  createdAt: number;
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
  role: "owner" | "shareto" | "market" | string;
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
  app: "claude" | "codex" | "gemini" | string;
  period: "24h" | "1w" | "30d" | string;
  bucketGranularity?: "hour" | "day" | string;
  days: number;
  totalTokens: number;
  rows: ShareUsageEmailRow[];
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

export type MarketRequestLog = {
  requestId: string;
  marketId: string;
  marketEmail: string;
  marketSubdomain: string;
  userEmail?: string;
  apiKeyPrefix?: string;
  routerId?: string;
  shareId?: string;
  shareSubdomain?: string;
  model?: string;
  requestAgent: string;
  requestedModel: string;
  actualModel: string;
  actualModelSource?: string;
  status: string;
  statusCode?: number;
  errorMessage?: string;
  latencyMs?: number;
  inputTokens: number;
  outputTokens: number;
  cacheReadTokens?: number;
  cacheCreationTokens?: number;
  usageAmountUsd?: string;
  createdAt: string;
  settledAt?: string;
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
  inputTokens?: number;
  outputTokens?: number;
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

export type MarketAppAvailability = {
  claude?: MarketAppAvailabilityEntry;
  codex?: MarketAppAvailabilityEntry;
  gemini?: MarketAppAvailabilityEntry;
};

export type MarketAppAvailabilityEntry = {
  status: "available" | "degraded" | "unavailable" | "unknown" | string;
  reason?: string;
  requestedModel?: string;
  actualModel?: string;
  lastCheckedAt?: number;
  recentResults?: string[];
};

export type ModelHealthSummary = {
  appType: string;
  requestedModel: string;
  actualModel: string;
  status: "success" | "failed" | "skipped" | string;
  recentResults?: string[];
  lastCheckedAt?: number;
  lastSuccessAt?: number;
  lastFailedAt?: number;
  errorMessage?: string;
  statusCode?: number;
};

export type ShareModelHealthSummary = {
  claude?: ModelHealthSummary[];
  codex?: ModelHealthSummary[];
  gemini?: ModelHealthSummary[];
};

export type ShareUpstreamProvider = {
  providerName?: string;
  kind?: string;
  app?: string;
  providerType?: string;
  accountEmail?: string;
  forSaleOfficialPricePercent?: number;
  apiUrl?: string;
  quota?: {
    status?: string;
    plan?: string;
    credentialMessage?: string;
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
};

export type ShareAppProvider = {
  id: string;
  name: string;
  app: "claude" | "codex" | "gemini" | string;
  kind?: string;
  providerType?: string;
  isCurrent?: boolean;
  enabled?: boolean;
  codexImageGenerationEnabled?: boolean;
  forSaleOfficialPricePercent?: number;
  accountEmail?: string;
  apiUrl?: string;
  quota?: ShareUpstreamProvider["quota"];
  models?: ShareUpstreamProvider["models"];
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

export type SettingsField = {
  key: string;
  label: string;
  group: string;
  fieldType: "text" | "int" | "bool" | "path" | "url" | "email" | "email_list" | "ip_list" | "secret";
  required: boolean;
  restartRequired: boolean;
  default?: string | null;
  description: string;
  placeholder?: string | null;
};

export type SettingsSchema = {
  fields: SettingsField[];
  groups: string[];
};

export type SettingValueEntry = {
  key: string;
  value?: string | null;
  hasValue: boolean;
  isSecret: boolean;
  source: "env_file" | "default" | "unset";
};

export type SettingsValuesResponse = {
  values: SettingValueEntry[];
};

export type SettingsUpdateResponse = {
  updatedKeys: string[];
  unchangedKeys: string[];
  restartRequiredKeys: string[];
  dynamicGroupsRefreshed: string[];
  envPath: string;
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
  proxyInflight: number;
  proxyRequestsTotal: number;
  proxyUpstreamErrorsTotal: number;
  proxy5xxTotal: number;
  healthProbeFailuresTotal: number;
  healthProbeCachedFailuresTotal: number;
  dbErrorsTotal: number;
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
  host: HostMetricsStatus;
  router: RouterMetricsStatus;
  llm: LlmMetricsSnapshot;
  alerts: MetricEvent[];
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

export type LlmMetricsPoint = {
  timestamp: number;
  rpm: number;
  tpm: number;
  inputTpm: number;
  outputTpm: number;
  errorRate: number;
  rateLimited: number;
  p95LatencyMs?: number | null;
  p95TtftMs?: number | null;
};

export type MetricsSeriesResponse = {
  range: string;
  step: string;
  host: HostMetricsPoint[];
  router: RouterMetricsPoint[];
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

export type BoardMessage = {
  id: string;
  body: string;
  authorKind: string;
  authorLabel: string;
  isMine: boolean;
  pinned: boolean;
  featured: boolean;
  createdAt: string;
  pinnedAt?: string;
  featuredAt?: string;
};

export type BoardListResponse = {
  messages: BoardMessage[];
  tab: string;
  totalVisible: number;
  asOf: string;
  removedIds?: string[];
  incremental?: boolean;
};

export type BoardMeta = {
  total: number;
  pinnedCount: number;
  featuredCount: number;
  canPostAsAdmin: boolean;
  maxBodyLength: number;
  guestSelfDeleteSecs: number;
};

// P18: test-connection types
export type ShareConnectionTestRequest = {
  app: "claude" | "codex" | "gemini";
  kind?: "text" | "chat" | "image" | "tools";
  timeoutMs?: number;
};

export type ShareConnectionTestResponse = {
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
  schedulingRecovery?: {
    shareModelHealthDeleted: number;
    marketModelFailuresDeleted: number;
    marketRuntimeStatesDeleted: number;
  };
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
