/** Append embed=compact for Router iframe console loads (idempotent). */
export function withEmbedCompact(url: string): string {
  try {
    const parsed = new URL(url);
    if (
      !parsed.searchParams.has("embed") &&
      !parsed.searchParams.has("density")
    ) {
      parsed.searchParams.set("embed", "compact");
    }
    return parsed.toString();
  } catch {
    return url;
  }
}
