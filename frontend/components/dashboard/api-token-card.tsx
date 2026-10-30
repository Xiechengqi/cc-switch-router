"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Copy, Eye, EyeOff, KeyRound, Loader2, RotateCcw } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { AccountSectionError } from "@/components/dashboard/account-section";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getUserApiToken, resetUserApiToken } from "@/lib/api";
import type { UserApiTokenStatus } from "@/lib/types";
import { formatDateTime } from "@/lib/utils";

function buildShareCurlExample(host: string, apiToken: string) {
  const bearer = apiToken ? `Bearer ${apiToken}` : "Bearer <your-api-token>";
  const base = `https://<share-subdomain>.${host}`;
  return [
    `curl -N -sS -X POST \\`,
    `  '${base}/v1/messages' \\`,
    `  -H 'Authorization: ${bearer}' \\`,
    `  -H 'Accept: text/event-stream' \\`,
    `  -H 'Content-Type: application/json' \\`,
    `  -d '{"model":"<model>","max_tokens":16,"stream":true,"messages":[{"role":"user","content":"who are you"}]}'`,
  ].join("\n");
}

/** The account API token: reveal, copy and reset, with a ready-to-edit Share call. */
export function ApiTokenCard() {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;

  const [token, setToken] = React.useState<UserApiTokenStatus | null>(null);
  const [rawToken, setRawToken] = React.useState("");
  const [showToken, setShowToken] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  // Retry re-reads the token only when reading it failed, never after a failed reset.
  const [loadFailed, setLoadFailed] = React.useState(false);
  const [copiedToken, setCopiedToken] = React.useState(false);
  const [copiedCurl, setCopiedCurl] = React.useState(false);
  const [host, setHost] = React.useState("router.example.com");
  const [resetConfirmOpen, setResetConfirmOpen] = React.useState(false);

  const maskedToken = React.useMemo(() => {
    if (rawToken) {
      if (rawToken.length <= 12) return "•".repeat(rawToken.length);
      return `${rawToken.slice(0, 8)}${"•".repeat(16)}${rawToken.slice(-4)}`;
    }
    if (token?.prefix) return `${token.prefix}${"•".repeat(16)}`;
    return t("account.apiKeys.resetToGenerate");
  }, [rawToken, t, token?.prefix]);

  const curlExample = React.useMemo(
    () => buildShareCurlExample(host, showToken && rawToken ? rawToken : ""),
    [host, rawToken, showToken],
  );

  const load = React.useCallback(async () => {
    setBusy(true);
    setError("");
    setLoadFailed(false);
    try {
      const response = await getUserApiToken();
      setToken(response.token);
      setRawToken(response.apiToken || "");
      setShowToken(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setLoadFailed(true);
    } finally {
      setBusy(false);
    }
  }, []);

  React.useEffect(() => {
    if (typeof window !== "undefined" && window.location.host) {
      setHost(window.location.host);
    }
  }, []);

  React.useEffect(() => {
    if (!authed) return;
    load().catch(console.error);
  }, [authed, load]);

  const reset = async () => {
    setBusy(true);
    setError("");
    setLoadFailed(false);
    setCopiedToken(false);
    try {
      const response = await resetUserApiToken();
      setToken(response.token);
      setRawToken(response.apiToken);
      setShowToken(true);
      setResetConfirmOpen(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const copyToken = async () => {
    if (!rawToken) return;
    await navigator.clipboard.writeText(rawToken);
    setCopiedToken(true);
    window.setTimeout(() => setCopiedToken(false), 1500);
  };

  const copyCurl = async () => {
    const forClipboard = buildShareCurlExample(host, rawToken || "");
    await navigator.clipboard.writeText(forClipboard);
    setCopiedCurl(true);
    window.setTimeout(() => setCopiedCurl(false), 1500);
  };

  return (
    <section aria-labelledby="api-token-title" className="grid min-w-0 grid-cols-1 gap-4 overflow-hidden rounded-xl border border-border bg-card p-4 sm:p-5 [&>*]:min-w-0">
      <div>
        <h3 id="api-token-title" className="flex items-center gap-2 text-sm font-semibold text-foreground">
          <KeyRound className="h-4 w-4 text-muted-foreground" aria-hidden />
          {t("account.apiKeys.title")}
        </h3>
        <p className="mt-0.5 text-sm text-muted-foreground">{t("account.apiKeys.hint")}</p>
      </div>

      {error ? <AccountSectionError message={error} onRetry={loadFailed ? () => void load() : undefined} /> : null}

      <div className="grid gap-3">
        <div className="grid min-w-0 gap-2 text-sm">
          <div className="flex min-w-0 justify-between gap-3">
            <span className="shrink-0 text-muted-foreground">{t("account.apiKeys.created")}</span>
            <span className="min-w-0 text-right tabular-nums">{token?.createdAt ? formatDateTime(token.createdAt, locale) : "-"}</span>
          </div>
          <div className="flex min-w-0 justify-between gap-3">
            <span className="shrink-0 text-muted-foreground">{t("account.apiKeys.lastUsed")}</span>
            <span className="min-w-0 text-right tabular-nums">{token?.lastUsedAt ? formatDateTime(token.lastUsedAt, locale) : "-"}</span>
          </div>
        </div>

        <div className="flex min-w-0 items-start gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2">
          <div className="min-w-0 flex-1 break-all font-mono text-xs text-foreground">
            {busy && !token ? t("account.loading") : showToken && rawToken ? rawToken : maskedToken}
          </div>
          <button
            type="button"
            aria-label={showToken ? t("account.apiKeys.hideToken") : t("account.apiKeys.showToken")}
            title={showToken ? t("account.apiKeys.hideToken") : t("account.apiKeys.showToken")}
            className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
            disabled={!rawToken || busy}
            onClick={() => setShowToken((value) => !value)}
          >
            {showToken ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
          </button>
        </div>

        <div className="flex flex-wrap gap-2">
          <Button variant="outline" size="sm" onClick={() => void copyToken()} isDisabled={!rawToken || busy}>
            <Copy className="h-4 w-4" />
            {copiedToken ? t("account.apiKeys.copied") : t("account.apiKeys.copy")}
          </Button>
          <Button variant="outline" size="sm" className="text-rose-700" onClick={() => setResetConfirmOpen(true)} isDisabled={busy}>
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
            {t("account.apiKeys.reset")}
          </Button>
        </div>
      </div>

      <div className="grid gap-3 border-t border-border pt-4">
        <div>
          <h4 className="text-sm font-semibold text-foreground">{t("account.apiKeys.exampleTitle")}</h4>
          <p className="mt-0.5 text-sm text-muted-foreground">{t("account.apiKeys.exampleHint")}</p>
        </div>
        <div className="relative min-w-0 overflow-hidden rounded-xl border border-border bg-muted/30 p-4">
          <Button
            variant="ghost"
            size="sm"
            className="absolute right-2 top-2 h-8"
            onClick={() => void copyCurl()}
          >
            <Copy className="h-3.5 w-3.5" />
            {copiedCurl ? t("account.apiKeys.copied") : t("account.apiKeys.copy")}
          </Button>
          <pre className="overflow-x-auto whitespace-pre pr-24 font-mono text-[11px] leading-relaxed text-foreground sm:text-xs">
            {curlExample}
          </pre>
        </div>
      </div>

      <ConfirmAlertDialog
        open={resetConfirmOpen}
        title={t("account.apiKeys.resetConfirmTitle")}
        description={t("account.apiKeys.resetConfirmDescription")}
        confirmLabel={t("account.apiKeys.resetConfirmAction")}
        cancelLabel={t("common.cancel")}
        tone="danger"
        busy={busy}
        onConfirm={() => void reset()}
        onOpenChange={(next) => !busy && setResetConfirmOpen(next)}
      />
    </section>
  );
}
