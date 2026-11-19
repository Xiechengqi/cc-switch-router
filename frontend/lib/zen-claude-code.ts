import { shellQuote } from "@/lib/share-model-probe";

const EFFORTS = ["low", "medium", "high", "xhigh", "max"] as const;

export type ZenClaudeCodeModel = {
  id: string;
  displayName: string;
  maxOutputTokens: number;
  effort: (typeof EFFORTS)[number];
};

export type ZenClaudeCodeCatalog = { fixed: boolean; models: ZenClaudeCodeModel[] };

export async function loadZenClaudeCodeCatalog(
  baseUrl: string,
  routerHost: string,
  apiToken: string,
  shareId: string,
  providerId: string,
  signal: AbortSignal,
): Promise<ZenClaudeCodeCatalog> {
  const base = new URL(baseUrl);
  const router = new URL(`https://${routerHost}`);
  if (base.protocol !== "https:" || base.username || base.password || base.port !== router.port
    || !base.hostname.endsWith(`.${router.hostname}`) || base.pathname !== "/"
    || base.search || base.hash || !apiToken || !shareId || !providerId) {
    throw new Error("invalid Claude Code Share scope");
  }
  const url = new URL("/v1/models", base);
  url.search = new URLSearchParams({ app: "claude", expectedShareId: shareId, expectedProviderId: providerId }).toString();
  const response = await fetch(url, {
    headers: { Authorization: `Bearer ${apiToken}`, "anthropic-version": "2023-06-01" },
    credentials: "omit",
    redirect: "error",
    cache: "no-store",
    signal,
  });
  if (!response.ok) throw new Error("Claude Code catalog request failed");
  const data = await response.json();
  const catalog = data?.claudeCode;
  if (data?.source !== "opencode_zen_live_model_catalog" || typeof catalog?.fixed !== "boolean"
    || !Array.isArray(catalog.models) || catalog.models.length > 512
    || (catalog.fixed && catalog.models.length > 1)) {
    throw new Error("Claude Code catalog is unavailable");
  }
  const ids = new Set<string>();
  const models = catalog.models.map((model: ZenClaudeCodeModel) => {
    if (typeof model?.id !== "string" || !/^[a-zA-Z0-9_.:/-]{1,256}$/.test(model.id)
      || ids.has(model.id) || typeof model.displayName !== "string"
      || !Number.isSafeInteger(model.maxOutputTokens) || model.maxOutputTokens <= 0
      || !EFFORTS.includes(model.effort)) {
      throw new Error("invalid Claude Code model profile");
    }
    ids.add(model.id);
    return { id: model.id, displayName: model.displayName, maxOutputTokens: model.maxOutputTokens, effort: model.effort };
  });
  return { fixed: catalog.fixed, models };
}

export function zenClaudeCodeCommand(baseUrl: string, model: ZenClaudeCodeModel, verify: boolean) {
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
