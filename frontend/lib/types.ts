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
  clients: DashboardClient[];
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

export type DashboardClient = {
  installation: {
    id: string;
    platform: string;
    appVersion: string;
    region?: string;
    countryCode?: string;
    createdAt: string;
    lastSeenAt: string;
  };
  share?: ShareView;
};

export type ShareView = {
  shareId: string;
  shareName: string;
  ownerEmail?: string;
  sharedWithEmails?: string[];
  marketLinks?: ShareMarketLink[];
  unknownMarketEmails?: string[];
  description?: string;
  forSale: string;
  marketAccessMode: string;
  subdomain: string;
  shareToken?: string;
  canViewSecret?: boolean;
  canManage?: boolean;
  canEditSettings?: boolean;
  activeEdit?: ShareEditView;
  appType: string;
  providerId?: string;
  tokenLimit: number;
  parallelLimit: number;
  tokensUsed: number;
  requestsCount: number;
  shareStatus: string;
  createdAt: string;
  expiresAt: string;
  isOnline: boolean;
  activeRequests: number;
  onlineMinutes24h?: number;
  onlineRate24h: number;
  recentRequests?: ShareRequestLog[];
  healthChecks?: HealthCheckEntry[];
  support?: ShareSupport;
  appRuntimes?: ShareAppRuntimes;
};

export type ShareSettingsPatch = {
  description?: string | null;
  forSale?: "Yes" | "No" | "Free";
  marketAccessMode?: "selected" | "all";
  sharedWithEmails?: string[];
  forSaleOfficialPricePercentByApp?: Record<string, number>;
  tokenLimit?: number;
  parallelLimit?: number;
  expiresAt?: string;
  autoStart?: boolean;
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

export type ShareMarketLink = {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  status: string;
  online: boolean;
};

export type DashboardMarket = {
  id: string;
  displayName: string;
  email: string;
  subdomain: string;
  publicBaseUrl: string;
  status: string;
  online: boolean;
  canManage?: boolean;
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
  }>;
  recentRequests?: MarketRequestLog[];
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
  marketAccessMode: string;
  shareStatus: string;
  online: boolean;
  activeRequests: number;
  parallelLimit: number;
  onlineRate24h: number;
  lastSeenAt: string;
  disabledByMarket?: boolean;
  marketDisabledAt?: string;
  support?: ShareSupport;
  appRuntimes?: ShareAppRuntimes;
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
  createdAt: number;
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
  startedAt?: string;
  createdAt?: string;
  isInflight?: boolean;
  latencyMs?: number;
  inputTokens?: number;
  outputTokens?: number;
};

export type ShareSupport = {
  claude?: boolean;
  codex?: boolean;
  gemini?: boolean;
};

export type ShareUpstreamProvider = {
  kind?: string;
  app?: string;
  accountEmail?: string;
  forSaleOfficialPricePercent?: number;
  apiUrl?: string;
  quota?: {
    status?: string;
    tiers?: Array<{
      label?: string;
      utilization?: number;
      resetsAt?: string;
    }>;
  };
  models?: Array<{
    slot?: string;
    actualModel?: string;
  }>;
};

export type ShareAppRuntimes = {
  claude?: ShareUpstreamProvider;
  codex?: ShareUpstreamProvider;
  gemini?: ShareUpstreamProvider;
};

export type HealthCheckEntry = {
  checkedAt: number;
  isHealthy: boolean;
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
