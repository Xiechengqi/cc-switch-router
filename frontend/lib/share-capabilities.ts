import type { ShareUserGrantMap } from "@/lib/types";

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
  displayVersion?: 2;
  unavailableBindings?: { app: string; reason: string }[];
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
    implemented?: boolean;
    providerEnabled?: boolean;
    accountSupport?: "supported" | "unsupported" | "unknown";
    unavailableReason?:
      | "driver_not_implemented"
      | "provider_disabled"
      | "account_unsupported"
      | "runtime_unavailable"
      | null;
    requiredProviderOption?: string | null;
    providerLabel?: string | null;
    modelScope?: "fixed" | "request";
    pricingCoverage?:
      | "base_rate"
      | "parameter_rates"
      | "request_dependent"
      | "missing"
      | "tariff_unavailable";
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
  measurement_unknown: [
    "资源计量状态尚未明确，可能无法完整扣减额度",
    "Resource metering is unknown and may not fully debit quotas",
  ],
  runtime_binding_changed: [
    "账号或执行绑定已改变。确认后，将为上述能力重新授权当前绑定。",
    "The account or execution binding changed. Confirmation authorizes the current binding for these extensions.",
  ],
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
    (value.displayVersion !== undefined && value.displayVersion !== 2) ||
    (value.unavailableBindings !== undefined &&
      (!Array.isArray(value.unavailableBindings) ||
        value.unavailableBindings.length > 3 ||
        !value.unavailableBindings.every(
          (item) =>
            object(item) &&
            typeof item.app === "string" &&
            item.reason === "runtime_unavailable",
        ))) ||
    !Array.isArray(value.rows) ||
    value.rows.length > 30 ||
    !value.rows.every(
      (row) =>
        object(row) &&
        knownOperation(row.operation) &&
        typeof row.app === "string" &&
        typeof row.bindingFingerprint === "string" &&
        typeof row.controllable === "boolean" &&
        (value.displayVersion !== 2 ||
          (typeof row.implemented === "boolean" &&
            typeof row.providerEnabled === "boolean" &&
            ["supported", "unsupported", "unknown"].includes(
              String(row.accountSupport),
            ) &&
            ["fixed", "request"].includes(String(row.modelScope)) &&
            [
              "base_rate",
              "parameter_rates",
              "request_dependent",
              "missing",
              "tariff_unavailable",
            ].includes(String(row.pricingCoverage)) &&
            (row.unavailableReason == null ||
              [
                "driver_not_implemented",
                "provider_disabled",
                "account_unsupported",
                "runtime_unavailable",
              ].includes(String(row.unavailableReason))) &&
            (row.providerLabel == null ||
              typeof row.providerLabel === "string"))) &&
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

export type CapabilityRow = CapabilityPreview["rows"][number];
export type CapabilityApproval = {
  policy: ShareCapabilityPolicy;
  consent: CapabilityConsent;
};
export type CapabilityReviewHandle = {
  review: () => Promise<CapabilityApproval | null>;
};

/** Missing new display facts never turn a legacy runtime into verified support. */
export function capabilitySelectable(row: CapabilityRow): boolean {
  return (
    row.controllable &&
    !row.unavailableReason &&
    row.accountSupport !== "unsupported"
  );
}

export function capabilityOptions(
  preview: CapabilityPreview | undefined,
  policy: ShareCapabilityPolicy | undefined,
) {
  return SHARE_OPERATIONS.map((operation) => {
    const rows =
      preview?.rows.filter((row) => row.operation === operation) ?? [];
    const selected = policy?.allowedOperations.includes(operation) ?? false;
    const available = rows.some(capabilitySelectable);
    const configurable = rows.some(
      (row) => row.implemented && row.unavailableReason === "provider_disabled",
    );
    return { operation, rows, selected, available, configurable };
  }).filter(
    (option) => option.available || option.selected || option.configurable,
  );
}

/** Only a complete, current runtime result can establish that there is nothing to configure. */
export function capabilityPresentation({
  preview,
  policy,
  savedPolicy,
  current,
  loading,
  error,
  reviewing,
}: {
  preview?: CapabilityPreview;
  policy?: ShareCapabilityPolicy;
  savedPolicy?: ShareCapabilityPolicy;
  current: boolean;
  loading: boolean;
  error?: string;
  reviewing?: boolean;
}) {
  const options = capabilityOptions(preview, policy);
  const main = options.filter((option) => option.available || option.selected);
  const setup = options.filter(
    (option) => !option.available && !option.selected && option.configurable,
  );
  const pending = capabilityIntent(policy) !== capabilityIntent(savedPolicy);
  const retained =
    pending ||
    reviewing ||
    main.length > 0 ||
    !!savedPolicy?.allowedOperations.length ||
    !!preview?.policy.allowedOperations.length ||
    !!preview?.requiredAcknowledgements.length ||
    !!preview?.blockers?.length ||
    !!preview?.incidents?.length ||
    !!preview?.recentUsage?.length;
  const blocked =
    current &&
    !loading &&
    !!preview &&
    (!preview.ready || !!preview.blockers?.length);
  const status = error
    ? "unavailable"
    : loading || !current
      ? "loading"
      : !preview?.ready ||
          preview.displayVersion !== 2 ||
          preview.expiresAt <= Date.now() ||
          !!preview.blockers?.length ||
          !!preview.unavailableBindings?.length ||
          preview.rows.some(
            (row) => row.unavailableReason === "runtime_unavailable",
          )
        ? "unavailable"
        : undefined;
  return {
    main,
    setup,
    pending,
    blocked,
    status,
    mode: retained ? "full" : status || setup.length ? "compact" : "hidden",
  } as const;
}

/** Keep financial gaps visible even when their detailed evidence is collapsed. */
export function capabilityWarnings(rows: CapabilityRow[], lang: 0 | 1) {
  const reasons = [
    ...new Set(
      rows
        .filter((row) => row.selected)
        .flatMap((row) => row.scopes.map((scope) => scope.reason)),
    ),
  ];
  const labels: Record<string, [string, string]> = {
    resource_not_in_token_ledger: [
      "资源不计入 Token 限额",
      "Resources do not debit Token quotas",
    ],
    model_or_resource_price_unknown: [
      "缺价资源不扣美元额度",
      "Unpriced resources do not debit USD quotas",
    ],
    estimated_resource_quantity: ["含估算用量", "Includes estimated usage"],
    no_usage_limit: ["资源无用量上限", "Resources have no usage limit"],
    measurement_unknown: ["资源计量待确认", "Resource metering unknown"],
  };
  return reasons
    .filter((reason) => labels[reason])
    .map((reason) => ({
      reason,
      label: labels[reason][lang],
      detail: COVERAGE_LABELS[reason][lang],
    }));
}

export function capabilityNeedsReview(
  policy: ShareCapabilityPolicy | undefined,
  grants: ShareUserGrantMap,
  tokenLimit: number,
  savedPolicy: ShareCapabilityPolicy | undefined,
  savedGrants: ShareUserGrantMap,
  savedTokenLimit: number,
) {
  return (
    capabilityIntent(policy) !== capabilityIntent(savedPolicy) ||
    (!!policy &&
      capabilityQuotaContext(policy, grants, tokenLimit) !==
        capabilityQuotaContext(savedPolicy, savedGrants, savedTokenLimit))
  );
}

/** Restrict ordinary users atomically with Share changes; managed contracts stay immutable. */
export function reconcileCapabilityGrants(
  policy: ShareCapabilityPolicy,
  grants: ShareUserGrantMap,
): ShareUserGrantMap {
  return Object.fromEntries(
    Object.entries(grants).map(([email, grant]) => {
      if (!grant.active || grant.manager === "routerShareMarket")
        return [email, grant];
      const allowedOperations = grant.policy.allowedOperations?.filter((op) =>
        policy.allowedOperations.includes(op),
      );
      const hasExtensions = policy.allowedOperations.some(
        (op) => allowedOperations?.includes(op) ?? true,
      );
      const pending = grant.pendingValueQuota?.policy;
      const permissionChanged =
        JSON.stringify(allowedOperations ?? null) !==
        JSON.stringify(grant.policy.allowedOperations ?? null);
      const tariffChanged =
        hasExtensions &&
        (pending?.valueQuota ?? grant.policy.valueQuota)?.tariffPolicyId ===
          "api-equivalent-v1";
      const source =
        pending && (permissionChanged || tariffChanged)
          ? pending
          : grant.policy;
      const nextPolicy = {
        ...source,
        allowedOperations,
        ...(hasExtensions && source.valueQuota
          ? {
              valueQuota: {
                ...source.valueQuota,
                tariffPolicyId: "api-equivalent-v2" as const,
              },
            }
          : {}),
      };
      // A signed capability-only patch updates both units while the Server keeps
      // the original activation date. Explicit quota edits drop this metadata.
      return [email, { ...grant, policy: nextPolicy }];
    }),
  );
}

export type CapabilityPermissionMode = "inherit" | "custom" | "disabled";
export function capabilityPermissionMode(
  operations: ShareOperation[] | undefined,
): CapabilityPermissionMode {
  return operations === undefined
    ? "inherit"
    : operations.length
      ? "custom"
      : "disabled";
}
export function capabilityPermissionChange(
  mode: CapabilityPermissionMode,
  current: ShareOperation[] | undefined,
  share: readonly ShareOperation[],
) {
  return mode === "inherit"
    ? undefined
    : mode === "disabled"
      ? []
      : current?.length
        ? current.filter((op) => share.includes(op))
        : [...share];
}
export function capabilityPermissionSummary(
  operations: ShareOperation[] | undefined,
  lang: 0 | 1,
) {
  if (operations === undefined)
    return lang === 0 ? "跟随 Share" : "Follow Share";
  if (!operations.length)
    return lang === 0 ? "不允许扩展能力" : "Extensions disabled";
  return operations
    .map((op) => OPERATION_LABELS[op][lang])
    .join(lang === 0 ? "、" : ", ");
}

/** A single visible risk group still accepts each exact Server-owned condition. */
export function capabilityRiskGroups(
  preview: CapabilityPreview,
  savedPolicy?: ShareCapabilityPolicy,
) {
  const groups = new Map<
    string,
    {
      key: string;
      reason: string;
      quotaUnit: string;
      operations: ShareOperation[];
      scopes: string[];
      conditions: string[];
    }
  >();
  for (const ack of preview.requiredAcknowledgements) {
    if (
      savedPolicy?.coverageAcknowledgements?.some(
        (old) => old.condition === ack.condition,
      )
    )
      continue;
    const row = preview.rows.find(
      (row) =>
        row.operation === ack.operation &&
        row.bindingFingerprint === ack.bindingFingerprint,
    );
    const scope = row?.scopes.find(
      (scope) => scope.condition === ack.condition,
    );
    const reason = scope?.reason ?? "measurement_unknown";
    const key = JSON.stringify([reason, ack.quotaUnit]);
    const group = groups.get(key) ?? {
      key,
      reason,
      quotaUnit: ack.quotaUnit,
      operations: [],
      scopes: [],
      conditions: [],
    };
    if (!group.operations.includes(ack.operation))
      group.operations.push(ack.operation);
    if (!group.scopes.includes(ack.scope)) group.scopes.push(ack.scope);
    group.conditions.push(ack.condition);
    groups.set(key, group);
  }
  return [...groups.values()];
}

export function capabilityStatus(row: CapabilityRow, lang: 0 | 1) {
  const reason = row.unavailableReason;
  if (reason === "provider_disabled")
    return lang === 0
      ? "需先在供应商设置中开启"
      : "Enable in Provider settings first";
  if (reason === "runtime_unavailable")
    return lang === 0
      ? "供应商配置需要处理"
      : "Provider configuration needs attention";
  if (reason === "account_unsupported" || row.accountSupport === "unsupported")
    return lang === 0 ? "账号明确不支持" : "Account does not support this";
  if (reason === "driver_not_implemented" || !row.controllable)
    return lang === 0
      ? "当前执行路径未接入"
      : "Not implemented on this execution path";
  return row.accountSupport === "supported" || row.execution === "supported"
    ? lang === 0
      ? "账号已验证"
      : "Account verified"
    : lang === 0
      ? "账号待验证"
      : "Account verification pending";
}
export function capabilityPricingText(row: CapabilityRow, lang: 0 | 1) {
  const state = row.pricingCoverage;
  if (state === "parameter_rates")
    return lang === 0
      ? "部分参数有价，按请求检查"
      : "Some parameters priced; checked per request";
  if (state === "request_dependent")
    return lang === 0
      ? "按实际模型与参数检查价格"
      : "Prices checked for the actual model and parameters";
  if (state === "tariff_unavailable")
    return lang === 0
      ? "尚未取得有效资源价目"
      : "Active resource tariff unavailable";
  if (row.pricing === "explicit_free")
    return lang === 0 ? "资源价格明确为零" : "Resource rate explicitly zero";
  if (state === "base_rate" || row.pricing === "priced")
    return lang === 0
      ? "已配置资源价，数量可能估算"
      : "Resource rate configured; quantities may be estimated";
  return lang === 0
    ? "当前模型未配置资源价"
    : "Resource rate missing for the current model";
}
export function capabilityScopeLabel(scope: string, lang: 0 | 1) {
  return scope === "$default"
    ? lang === 0
      ? "未单独配置的调用"
      : "Calls without an individual policy"
    : scope;
}

/** Keep a legacy waiting quota when only its independent permissions change. */
export function preservesCapabilitySchedule(
  grant: ShareUserGrantMap[string],
  requested: ShareUserGrantMap[string]["policy"],
) {
  if (!grant.pendingValueQuota) return false;
  const comparable = (policy: typeof requested) => {
    const value = { ...policy, allowedOperations: undefined };
    if (value.valueQuota) {
      value.valueQuota = {
        ...value.valueQuota,
        unpricedUsage: value.valueQuota.unpricedUsage ?? "zero",
        ...(requested.valueQuota?.tariffPolicyId === "api-equivalent-v2"
          ? { tariffPolicyId: "api-equivalent-v2" as const }
          : {}),
      };
    }
    const ordered = (item: unknown): unknown =>
      Array.isArray(item)
        ? item.map(ordered)
        : item && typeof item === "object"
          ? Object.fromEntries(
              Object.entries(item)
                .filter(([, value]) => value !== undefined)
                .sort(([a], [b]) => a.localeCompare(b))
                .map(([key, value]) => [key, ordered(value)]),
            )
          : item;
    return JSON.stringify(ordered(value));
  };
  return [grant.policy, grant.pendingValueQuota.policy].some(
    (original) =>
      comparable(requested) === comparable(original) &&
      (JSON.stringify(requested.allowedOperations ?? null) !==
        JSON.stringify(original.allowedOperations ?? null) ||
        (requested.valueQuota?.tariffPolicyId === "api-equivalent-v2" &&
          original.valueQuota?.tariffPolicyId !== "api-equivalent-v2")),
  );
}
