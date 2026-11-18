export const SHARE_OPERATIONS = [
  "image.generate",
  "image.edit",
  "video.generate",
  "audio.generate",
  "tool.web_search",
  "tool.web_fetch",
  "tool.file_search",
  "tool.code_execution",
  "tool.computer",
  "tool.remote_mcp",
] as const;
export type ShareOperation = (typeof SHARE_OPERATIONS)[number];
export type CapabilityConsent = {
  credential: string;
  acceptUncovered: string[];
};
export type CoverageAcknowledgement = {
  condition: string;
  scope: string;
  operation: ShareOperation;
  quotaUnit: string;
  bindingFingerprint: string;
  actor: string;
  acceptedAtMs: number;
};
export type ShareCapabilityPolicy = {
  version: 1;
  allowedOperations: ShareOperation[];
  operationBindings: Partial<
    Record<ShareOperation, { app: string; fingerprint: string }[]>
  >;
  coverageAcknowledgements?: CoverageAcknowledgement[];
};
export type CapabilityPreview = {
  shareId: string;
  configRevision: number;
  ready: boolean;
  legacy: boolean;
  policy: ShareCapabilityPolicy;
  previewCredential?: string;
  expiresAt: number;
  reason?: string;
  incidents?: {
    scope: string;
    bindingFingerprint: string;
    requestId: string;
    resource: string;
    detectedAtMs: number;
  }[];
  recentUsage?: CapabilityUsage[];
  blockers?: { scope: string; blocker: string }[];
  requiredAcknowledgements: CoverageAcknowledgement[];
  rows: {
    app: string;
    operation: ShareOperation;
    bindingFingerprint: string;
    execution: string;
    controllable: boolean;
    selected?: boolean;
    model?: string;
    measurement: string;
    nativeUnit: string;
    pricing: string;
    admission: string;
    scopes: {
      scope: string;
      quotaUnit: string;
      coverage: string;
      reason: string;
      condition?: string;
      blocker?: string;
    }[];
  }[];
};
export function emptyCapabilityPolicy(): ShareCapabilityPolicy {
  return { version: 1, allowedOperations: [], operationBindings: {} };
}
export function capabilityIntent(policy: ShareCapabilityPolicy | undefined) {
  return JSON.stringify(
    policy
      ? [
          [...policy.allowedOperations].sort(),
          Object.entries(policy.operationBindings).sort(([a], [b]) =>
            a.localeCompare(b),
          ),
        ]
      : null,
  );
}
export const OPERATION_LABELS: Record<ShareOperation, [string, string]> = {
  "image.generate": ["图片生成", "Image generation"],
  "image.edit": ["图片编辑", "Image editing"],
  "video.generate": ["视频生成", "Video generation"],
  "audio.generate": ["音频输出", "Audio output"],
  "tool.web_search": ["网页搜索", "Web search"],
  "tool.web_fetch": ["网页读取", "Web fetch"],
  "tool.file_search": ["文件搜索", "File search"],
  "tool.code_execution": ["供应商代码执行", "Hosted code execution"],
  "tool.computer": ["供应商计算机操作", "Hosted computer"],
  "tool.remote_mcp": ["远程 MCP 服务", "Remote MCP"],
};
export const COVERAGE_LABELS: Record<string, [string, string]> = {
  resource_not_in_token_ledger: [
    "资源消耗不计入 Token 限额；仅真实对话 Token 继续扣减",
    "Resources do not debit Token quotas; real conversation Tokens still count",
  ],
  no_usage_limit: ["未设置用量上限", "No usage limit"],
  estimated_resource_quantity: [
    "资源数量可能使用估算，预占不是上游执行上限",
    "Quantities may be estimated; reservations are not upstream execution limits",
  ],
  priced_requests_only: [
    "仅允许实际模型与参数有价的请求；拒绝缺价不能通过风险确认绕过",
    "Only requests with prices for their actual model and parameters can run; acceptance cannot override rejection",
  ],
  model_or_resource_price_unknown: [
    "模型范围或资源价格未完全覆盖；缺价维度不扣美元额度",
    "Model range or resource prices are incomplete; unpriced dimensions do not debit USD quotas",
  ],
};

export function capabilityQuotaContext(
  policy: ShareCapabilityPolicy | undefined,
  grants: Record<string, { active: boolean; policy: unknown }>,
  tokenLimit: number,
) {
  return JSON.stringify([
    capabilityIntent(policy),
    Object.entries(grants)
      .filter(([, g]) => g.active)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([email, g]) => [email, g.policy]),
    tokenLimit,
  ]);
}

export type CapabilityUsage = {
  requestId: string;
  attempt: number;
  scope: string;
  admittedAtMs: number;
  settledPicos?: string;
  pendingRiskPicos?: string;
  meteringStatus: string;
  pricingRevision?: string;
  unpricedReason?: string;
  estimated?: boolean;
  dimensions: {
    dimension: string;
    quantity: number;
    ratePicos?: string;
    amountPicos?: string;
    pricing: string;
  }[];
};
export function capabilityAmount(picos: string | undefined): string {
  if (picos == null || !/^\d+$/.test(picos)) return "—";
  const value = BigInt(picos);
  return `$${value / 1_000_000_000_000n}.${((value % 1_000_000_000_000n) / 1_000_000n).toString().padStart(6, "0")}`;
}

/** Reject stale or malformed control replies before rendering their authority. */
export function parseCapabilityPreview(
  value: unknown,
  shareId: string,
  input: unknown,
): CapabilityPreview {
  const object = (item: unknown): item is Record<string, unknown> =>
    item !== null && typeof item === "object" && !Array.isArray(item);
  const knownOperation = (op: unknown) =>
    typeof op === "string" &&
    (SHARE_OPERATIONS as readonly string[]).includes(op);
  const expected = object(input) ? input.expectedConfigRevision : undefined;
  if (
    !object(value) ||
    value.shareId !== shareId ||
    value.configRevision !== expected ||
    typeof value.ready !== "boolean" ||
    typeof value.expiresAt !== "number" ||
    !Number.isFinite(value.expiresAt) ||
    !object(value.policy) ||
    value.policy.version !== 1 ||
    !Array.isArray(value.policy.allowedOperations) ||
    !value.policy.allowedOperations.every(knownOperation) ||
    !object(value.policy.operationBindings) ||
    !Array.isArray(value.rows) ||
    value.rows.length > 30 ||
    !value.rows.every(
      (row) =>
        object(row) &&
        knownOperation(row.operation) &&
        typeof row.app === "string" &&
        typeof row.bindingFingerprint === "string" &&
        typeof row.controllable === "boolean" &&
        Array.isArray(row.scopes) &&
        row.scopes.every(
          (scope) =>
            object(scope) &&
            typeof scope.scope === "string" &&
            typeof scope.reason === "string",
        ),
    ) ||
    !Array.isArray(value.requiredAcknowledgements) ||
    value.requiredAcknowledgements.length > 16_384 ||
    !value.requiredAcknowledgements.every(
      (ack) =>
        object(ack) &&
        knownOperation(ack.operation) &&
        typeof ack.condition === "string" &&
        typeof ack.scope === "string",
    ) ||
    (value.ready &&
      (typeof value.previewCredential !== "string" ||
        !value.previewCredential ||
        value.previewCredential.length > 8192)) ||
    (value.blockers !== undefined &&
      (!Array.isArray(value.blockers) ||
        !value.blockers.every(
          (item) =>
            object(item) &&
            typeof item.scope === "string" &&
            typeof item.blocker === "string",
        ))) ||
    (value.recentUsage !== undefined &&
      (!Array.isArray(value.recentUsage) ||
        !value.recentUsage.every(
          (item) => object(item) && Array.isArray(item.dimensions),
        ))) ||
    (value.incidents !== undefined &&
      (!Array.isArray(value.incidents) ||
        !value.incidents.every(
          (item) =>
            object(item) &&
            typeof item.scope === "string" &&
            typeof item.resource === "string" &&
            typeof item.bindingFingerprint === "string",
        )))
  ) {
    throw new Error(
      "Capability preview is invalid or stale; refresh the configuration",
    );
  }
  return value as CapabilityPreview;
}
