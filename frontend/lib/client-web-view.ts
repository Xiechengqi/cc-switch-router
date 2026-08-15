export const CLIENT_WEB_VIEW_QUERY_PARAM = "view";
/** Tells the Client web app that our console window already frames it. */
export const CLIENT_WEB_EMBED_QUERY_PARAM = "embed";

export function clientWebTerminalUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) return trimmed;
  try {
    const url = new URL(trimmed);
    url.searchParams.set(CLIENT_WEB_VIEW_QUERY_PARAM, "terminal");
    url.searchParams.set(CLIENT_WEB_EMBED_QUERY_PARAM, "1");
    return url.toString();
  } catch {
    const [withoutHash, hash = ""] = trimmed.split("#", 2);
    const separator = withoutHash.includes("?") ? "&" : "?";
    const next = `${withoutHash}${separator}${CLIENT_WEB_VIEW_QUERY_PARAM}=terminal&${CLIENT_WEB_EMBED_QUERY_PARAM}=1`;
    return hash ? `${next}#${hash}` : next;
  }
}
