"use client";

import type { CoreShareApp } from "@/lib/share-app";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";

type ShareProviderIdentity = {
  providerName?: string;
  providerType?: string;
  kind?: string;
};

type ProviderLogo = {
  key: string;
  label: string;
  asset?: string;
  monochromeColor?: string;
};

const PROVIDER_LOGOS: Record<string, Omit<ProviderLogo, "key">> = {
  anthropic: {
    label: "Anthropic",
    asset: "/provider-icons/anthropic.svg",
    monochromeColor: "#D4915D",
  },
  openai: {
    label: "OpenAI",
    asset: "/provider-icons/openai.svg",
    monochromeColor: "#111827",
  },
  gemini: { label: "Gemini", asset: "/provider-icons/gemini.svg" },
  grok: {
    label: "Grok",
    asset: "/provider-icons/grok.svg",
    monochromeColor: "#111827",
  },
  kiro: { label: "Kiro", asset: "/provider-icons/kiro.png" },
  kimi: { label: "Kimi" },
  cursor: { label: "Cursor", asset: "/provider-icons/cursor.png" },
  ollama: {
    label: "Ollama",
    asset: "/provider-icons/ollama.svg",
    monochromeColor: "#111827",
  },
  openrouter: {
    label: "OpenRouter",
    asset: "/provider-icons/openrouter.svg",
    monochromeColor: "#111827",
  },
  github: {
    label: "GitHub",
    asset: "/provider-icons/github.svg",
    monochromeColor: "#111827",
  },
  deepseek: { label: "DeepSeek", asset: "/provider-icons/deepseek.svg" },
  aws: {
    label: "AWS",
    asset: "/provider-icons/aws.svg",
    monochromeColor: "#FF9900",
  },
  nvidia: { label: "NVIDIA", asset: "/provider-icons/nvidia.svg" },
  qoder: { label: "Qoder" },
};

const PROVIDER_TYPE_LOGOS: Record<string, string> = {
  claude_auth: "anthropic",
  claude_oauth: "anthropic",
  codex_oauth: "openai",
  gemini_cli: "gemini",
  google_gemini_oauth: "gemini",
  antigravity_oauth: "gemini",
  agy_oauth: "gemini",
  grok_oauth: "grok",
  kiro_oauth: "kiro",
  kimi_code: "kimi",
  qoder_cosy: "qoder",
  cursor_oauth: "cursor",
  cursor_apikey: "cursor",
  ollama_cloud: "ollama",
  openrouter: "openrouter",
  github_copilot: "github",
  deepseek_account: "deepseek",
  deepseek_api: "deepseek",
  aws_bedrock: "aws",
  nvidia: "nvidia",
};

const PROVIDER_NAME_LOGOS: Array<[marker: string, logo: string]> = [
  ["anthropic", "anthropic"],
  ["claude", "anthropic"],
  ["openai", "openai"],
  ["codex", "openai"],
  ["gemini", "gemini"],
  ["google", "gemini"],
  ["antigravity", "gemini"],
  ["grok", "grok"],
  ["x.ai", "grok"],
  ["xai", "grok"],
  ["kiro", "kiro"],
  ["kimi", "kimi"],
  ["moonshot", "kimi"],
  ["qoder", "qoder"],
  ["cursor", "cursor"],
  ["ollama", "ollama"],
  ["openrouter", "openrouter"],
  ["copilot", "github"],
  ["github", "github"],
  ["deepseek", "deepseek"],
  ["bedrock", "aws"],
  ["aws", "aws"],
  ["nvidia", "nvidia"],
];

function normalizeProviderValue(value: string | undefined) {
  return (value || "").trim().toLowerCase();
}

function KimiProviderLogo({ size }: { size: number }) {
  return (
    <svg
      aria-hidden
      viewBox="0 0 24 24"
      width={size}
      height={size}
      xmlns="http://www.w3.org/2000/svg"
    >
      <path
        d="M19.738 5.776c.163-.209.306-.4.457-.585.07-.087.064-.153-.004-.244-.655-.861-.717-1.817-.34-2.787.283-.73.909-1.072 1.674-1.145.477-.045.945.004 1.379.236.57.305.902.77 1.01 1.412.086.512.07 1.012-.075 1.508-.257.878-.888 1.333-1.753 1.448-.718.096-1.446.108-2.17.157-.056.004-.113 0-.178 0z"
        fill="#027AFF"
      />
      <path
        d="M17.962 1.844h-4.326l-3.425 7.81H5.369V1.878H1.5V22h3.87v-8.477h6.824a3.025 3.025 0 002.743-1.75V22h3.87v-8.477a3.87 3.87 0 00-3.588-3.86v-.01h-2.125a3.94 3.94 0 002.323-2.12l2.545-5.689z"
        fill="#6366F1"
      />
    </svg>
  );
}

export function resolveShareProviderLogo(
  provider: ShareProviderIdentity,
): ProviderLogo | null {
  const providerType = normalizeProviderValue(provider.providerType);
  const kind = normalizeProviderValue(provider.kind);
  const typedLogo = PROVIDER_TYPE_LOGOS[providerType] || PROVIDER_TYPE_LOGOS[kind];
  const searchable = [provider.providerName, provider.providerType, provider.kind]
    .map(normalizeProviderValue)
    .filter(Boolean)
    .join(" ");
  const namedLogo = PROVIDER_NAME_LOGOS.find(([marker]) =>
    searchable.includes(marker),
  )?.[1];
  const logoKey = typedLogo || namedLogo;
  if (!logoKey) return null;
  return { key: logoKey, ...PROVIDER_LOGOS[logoKey] };
}

export function ShareProviderLogo({
  provider,
  fallbackApp,
  size = 16,
}: {
  provider: ShareProviderIdentity;
  fallbackApp: CoreShareApp;
  size?: number;
}) {
  const logo = resolveShareProviderLogo(provider);
  const label = provider.providerName?.trim() || logo?.label;

  if (!logo) {
    if (label) {
      const initials = label
        .split(/\s+/)
        .map((word) => word[0])
        .join("")
        .toUpperCase()
        .slice(0, 2);
      return (
        <span
          className="inline-flex shrink-0 items-center justify-center rounded bg-slate-100 font-semibold text-slate-600"
          style={{
            width: size,
            height: size,
            fontSize: Math.max(8, size * 0.5),
          }}
          title={label}
          aria-label={label}
          role="img"
        >
          {initials}
        </span>
      );
    }
    return <ShareAppLogo app={fallbackApp} size={size} />;
  }

  if (logo.key === "kimi") {
    return (
      <span
        className="inline-flex shrink-0"
        title={label || logo.label}
        aria-label={label || logo.label}
        role="img"
      >
        <KimiProviderLogo size={size} />
      </span>
    );
  }

  if (!logo.asset) {
    return (
      <span
        className="inline-flex shrink-0 items-center justify-center font-semibold text-slate-700"
        style={{ width: size, height: size, fontSize: Math.max(8, size * 0.5) }}
        title={label}
        aria-label={label}
        role="img"
      >
        {logo.label.slice(0, 2).toUpperCase()}
      </span>
    );
  }

  if (logo.monochromeColor) {
    return (
      <span
        className="inline-flex shrink-0"
        style={{
          width: size,
          height: size,
          backgroundColor: logo.monochromeColor,
          maskImage: `url(${logo.asset})`,
          maskPosition: "center",
          maskRepeat: "no-repeat",
          maskSize: "contain",
          WebkitMaskImage: `url(${logo.asset})`,
          WebkitMaskPosition: "center",
          WebkitMaskRepeat: "no-repeat",
          WebkitMaskSize: "contain",
        }}
        title={label || logo.label}
        aria-label={label || logo.label}
        role="img"
      />
    );
  }

  return (
    <img
      src={logo.asset}
      width={size}
      height={size}
      className="shrink-0 object-contain"
      alt={label || logo.label}
      title={label || logo.label}
      loading="lazy"
    />
  );
}
