import { shellQuote } from "@/lib/share-model-probe";

const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;
const CODEX_EFFORTS = [...EFFORTS, "minimal"] as const;

export type ZenCliApp = "claude" | "codex";

export type ZenCliModel = {
  id: string;
  displayName: string;
  maxOutputTokens?: number;
  effort: (typeof CODEX_EFFORTS)[number];
};

export type ZenCliCatalog = { fixed: boolean; models: ZenCliModel[] };

export class ZenCliCatalogError extends Error {
  constructor(readonly reason: "stale" | "bindingChanged") {
    super(reason);
  }
}

export async function loadZenCliCatalog(
  app: ZenCliApp,
  baseUrl: string,
  routerHost: string,
  apiToken: string,
  shareId: string,
  providerId: string,
  signal: AbortSignal,
): Promise<ZenCliCatalog> {
  const base = new URL(baseUrl);
  const router = new URL(`https://${routerHost}`);
  if (base.protocol !== "https:" || base.username || base.password || base.port !== router.port
    || !base.hostname.endsWith(`.${router.hostname}`) || base.pathname !== "/"
    || base.search || base.hash || !apiToken || !shareId || !providerId) {
    throw new Error("invalid CLI Share scope");
  }
  const url = new URL("/v1/models", base);
  url.search = new URLSearchParams({ app, expectedShareId: shareId, expectedProviderId: providerId }).toString();
  const headers = new Headers({ Authorization: `Bearer ${apiToken}` });
  if (app === "claude") headers.set("anthropic-version", "2023-06-01");
  const response = await fetch(url, {
    headers,
    credentials: "omit",
    redirect: "error",
    cache: "no-store",
    signal,
  });
  if (response.status === 409) throw new ZenCliCatalogError("bindingChanged");
  if (!response.ok) throw new Error("CLI catalog request failed");
  const data = await response.json();
  const catalog = app === "claude" ? data?.claudeCode : data?.codexCli;
  if (data?.source !== "opencode_zen_live_model_catalog" || typeof catalog?.fixed !== "boolean"
    || !Array.isArray(catalog.models) || catalog.models.length > 512
    || (catalog.fixed && catalog.models.length > 1)) {
    throw new Error("CLI catalog is unavailable");
  }
  if (data.stale === true) throw new ZenCliCatalogError("stale");
  const ids = new Set<string>();
  const efforts: readonly string[] = app === "claude" ? EFFORTS : CODEX_EFFORTS;
  const models = catalog.models.map((model: ZenCliModel) => {
    if (typeof model?.id !== "string" || !/^[a-zA-Z0-9_.:/-]{1,256}$/.test(model.id)
      || ids.has(model.id) || typeof model.displayName !== "string"
      || (app === "claude" && (!Number.isSafeInteger(model.maxOutputTokens) || (model.maxOutputTokens ?? 0) <= 0))
      || !efforts.includes(model.effort)) {
      throw new Error("invalid CLI model profile");
    }
    ids.add(model.id);
    return { id: model.id, displayName: model.displayName, maxOutputTokens: model.maxOutputTokens, effort: model.effort };
  });
  return { fixed: catalog.fixed, models };
}

export function zenClaudeCodeCommand(baseUrl: string, model: ZenCliModel, verify: boolean) {
  const id = shellQuote(model.id);
  return [
    `ANTHROPIC_BASE_URL=${shellQuote(baseUrl)}`,
    "ANTHROPIC_API_KEY=",
    'ANTHROPIC_AUTH_TOKEN="${TOKENSWITCH_API_KEY:?Set your Share API key in TOKENSWITCH_API_KEY}"',
    `ANTHROPIC_DEFAULT_SONNET_MODEL=${id}`,
    `ANTHROPIC_DEFAULT_OPUS_MODEL=${id}`,
    `ANTHROPIC_DEFAULT_HAIKU_MODEL=${id}`,
    "CLAUDE_CODE_DISABLE_THINKING=1",
    `CLAUDE_CODE_MAX_OUTPUT_TOKENS=${model.maxOutputTokens}`,
    // The connection snippet works even when the selected model has no live
    // deferred-tool receipt. The CLI's ordinary local tools remain available.
    "ENABLE_TOOL_SEARCH=false",
    `claude --model=${id} --effort=${shellQuote(model.effort)}${verify ? ` -p ${shellQuote("Reply with OK. Do not use tools.")}` : ""}`,
  ].join(" \\\n  ");
}

export function zenCodexCliCommand(baseUrl: string, model: ZenCliModel, verify: boolean) {
  const base = JSON.stringify(new URL("/v1", baseUrl).href);
  const settings = [
    'model_provider="tokenswitch_zen"',
    'model_providers.tokenswitch_zen.name="TokenSwitch OpenCode Zen"',
    "model_providers.tokenswitch_zen.base_url=" + base,
    'model_providers.tokenswitch_zen.wire_api="responses"',
    'model_providers.tokenswitch_zen.env_key="TOKENSWITCH_API_KEY"',
    "model_providers.tokenswitch_zen.requires_openai_auth=false",
    "model_providers.tokenswitch_zen.supports_websockets=false",
    "model_reasoning_effort=" + JSON.stringify(model.effort),
    'model_reasoning_summary="auto"',
    'model_verbosity="medium"',
    'web_search="disabled"',
  ];
  const invocation = verify ? "codex exec --ephemeral --skip-git-repo-check --sandbox read-only" : "codex";
  return [
    invocation + " --model " + shellQuote(model.id),
    ...settings.map(setting => "-c " + shellQuote(setting)),
    ...(verify ? [shellQuote("Reply with OK. Do not use tools.")] : []),
  ].join(" \\\n  ");
}
