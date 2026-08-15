export const CLIENT_WEB_VIEW_QUERY_PARAM = "view";

export function clientWebTerminalUrl(baseUrl: string): string {
  const trimmed = baseUrl.trim();
  if (!trimmed) return trimmed;
  try {
    const url = new URL(trimmed);
    url.searchParams.set(CLIENT_WEB_VIEW_QUERY_PARAM, "terminal");
    return url.toString();
  } catch {
    const [withoutHash, hash = ""] = trimmed.split("#", 2);
    const separator = withoutHash.includes("?") ? "&" : "?";
    const next = `${withoutHash}${separator}${CLIENT_WEB_VIEW_QUERY_PARAM}=terminal`;
    return hash ? `${next}#${hash}` : next;
  }
}
