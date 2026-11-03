"use client";

import { providerInitials, providerLogo } from "@/lib/provider-presentation";
import { shareEnabledApps, type CoreShareApp } from "@/lib/share-app";
import type { PublicProviderIdentity, ShareView } from "@/lib/types";

type ShareProviderIdentity = {
  providerIdentity?: PublicProviderIdentity;
  providerName?: string;
  providerType?: string;
  kind?: string;
};

export type ShareProviderLogoEntry = {
  app: CoreShareApp;
  provider: ShareProviderIdentity;
  key: string;
};

/** One Share identity, independent of how many protocol surfaces are enabled. */
export function shareProviderLogoEntries(share: ShareView): ShareProviderLogoEntry[] {
  return [{
    app: shareEnabledApps(share)[0] || "codex",
    provider: { providerIdentity: share.providerIdentity },
    key: share.providerIdentity?.familyId || "unknown",
  }];
}

export function resolveShareProviderLogo(provider: ShareProviderIdentity) {
  return providerLogo(provider.providerIdentity);
}

export function ShareProviderLogo({
  provider,
  size = 16,
}: {
  provider: ShareProviderIdentity;
  // Kept for source compatibility; an App is never a provider-logo fallback.
  fallbackApp?: CoreShareApp;
  size?: number;
}) {
  const logo = resolveShareProviderLogo(provider);
  const label = provider.providerIdentity?.displayName || provider.providerName || "Unknown provider";
  const style = { width: size, height: size };
  if (!logo) {
    return (
      <span
        className="inline-flex shrink-0 items-center justify-center rounded bg-slate-100 font-semibold text-slate-600"
        style={{ ...style, fontSize: Math.max(8, size * 0.5) }}
        title={label} aria-label={label} role="img"
      >
        {providerInitials(label)}
      </span>
    );
  }
  if (logo.mode === "inline") {
    return (
      <span
        className="inline-flex shrink-0 items-center justify-center [&>svg]:h-full [&>svg]:w-full"
        style={{ ...style, color: logo.color }}
        title={label} aria-label={label} role="img"
        // Only frozen, audited local catalog assets, never Server-provided markup/URLs.
        dangerouslySetInnerHTML={{ __html: logo.content }}
      />
    );
  }
  return <img src={logo.content} width={size} height={size} style={style} className="shrink-0 object-contain" alt={label} title={label} loading="lazy" />;
}
