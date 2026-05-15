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
  description?: string;
  forSale: string;
  marketAccessMode: string;
  subdomain: string;
  appType: string;
  tokenLimit: number;
  parallelLimit: number;
  tokensUsed: number;
  requestsCount: number;
  shareStatus: string;
  createdAt: string;
  expiresAt: string;
  isOnline: boolean;
  activeRequests: number;
  onlineRate24h: number;
  recentRequests?: ShareRequestLog[];
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
  createdAt: string;
  updatedAt: string;
  lastSeenAt: string;
  offlineSince?: string;
  shareCount: number;
  onlineShareCount: number;
  activeRequests: number;
  parallelCapacity: number;
  onlineRate24h: number;
  usageTokens: number;
  usageAmountUsd: string;
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
  }>;
  recentRequests?: MarketRequestLog[];
};

export type ShareRequestLog = {
  requestId: string;
  model: string;
  requestAgent: string;
  statusCode: number;
  latencyMs: number;
  inputTokens: number;
  outputTokens: number;
  createdAt: number;
};

export type MarketRequestLog = {
  requestId: string;
  marketId: string;
  marketEmail: string;
  marketSubdomain: string;
  requestedModel: string;
  actualModel: string;
  status: string;
  statusCode?: number;
  latencyMs?: number;
  inputTokens: number;
  outputTokens: number;
  createdAt: string;
};

export type RecentRequestEvent = {
  requestId: string;
  shareId?: string;
  shareName?: string;
  subdomain?: string;
  countryCode?: string;
  startedAt?: string;
  createdAt?: string;
  latencyMs?: number;
  inputTokens?: number;
  outputTokens?: number;
};

export type SettingsField = {
  key: string;
  label: string;
  group: string;
  fieldType: "text" | "int" | "bool" | "path" | "url" | "email" | "email_list" | "secret";
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
};

export type BoardMeta = {
  total: number;
  pinnedCount: number;
  featuredCount: number;
  canPostAsAdmin: boolean;
  maxBodyLength: number;
  guestSelfDeleteSecs: number;
};
