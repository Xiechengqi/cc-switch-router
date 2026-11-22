import { ApiError } from "@/lib/api";
import { messages, type MessageKey } from "@/lib/i18n";

type Translate = (key: MessageKey, values?: Record<string, string | number>) => string;

/** Error code carried by an API or confirmation failure, if any. */
export function walletErrorCode(cause: unknown): string | undefined {
  if (cause instanceof ApiError) return cause.code;
  if (cause && typeof cause === "object" && "code" in cause && typeof cause.code === "string") return cause.code;
  return undefined;
}

/**
 * Localized text for a wallet failure. Known codes get plain-language copy;
 * anything else falls back to the server message.
 */
export function walletErrorText(cause: unknown, t: Translate, locale?: string): string {
  const code = walletErrorCode(cause);
  const key = code ? `walletError.${code}` : "";
  if (key && key in messages.en) {
    const usableAt = cause instanceof ApiError && typeof cause.details?.usableAt === "string" ? cause.details.usableAt : null;
    const time = usableAt && Number.isFinite(Date.parse(usableAt))
      ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(usableAt))
      : "";
    return t(key as MessageKey, { time });
  }
  return cause instanceof Error ? cause.message : String(cause);
}
