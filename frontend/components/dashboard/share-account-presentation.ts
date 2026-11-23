import type { AppLocale } from "@/lib/i18n";
import type { ShareUpstreamProvider } from "@/lib/types";
import { isApiProviderRuntime, providerApiEndpoint, providerQuotaStatusLine, providerQuotaDetails, providerStatusIdentity } from "./share-dashboard-utils";

/** Both cards use this account view; adapters supply only viewer-authorized data. */
export function shareAccountPresentation(runtime: ShareUpstreamProvider | undefined, locale: AppLocale, options: {
  allowIdentityFallback?: boolean;
  api?: boolean;
  publicApiLabel?: string;
} = {}) {
  const api = options.api ?? (runtime ? isApiProviderRuntime(runtime) : false);
  const primaryLine = api
    ? options.publicApiLabel ?? providerApiEndpoint(runtime)
    : providerQuotaStatusLine(runtime, locale);
  const identityLine = api ? "-" : providerStatusIdentity(runtime, options.allowIdentityFallback);
  return {
    primaryLine,
    identityLine,
    primaryTitle: api ? primaryLine : providerQuotaDetails(runtime, locale),
    identityTitle: identityLine,
    primaryMonospace: api && !options.publicApiLabel,
  };
}
