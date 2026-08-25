"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Check, Copy, ExternalLink, LogIn, Mail } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getUserApiToken } from "@/lib/api";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { ShareConnectionTestRow } from "@/components/dashboard/share-connection-test";
import {
  shareEnabledApps,
  SHARE_APP_LABELS,
} from "@/lib/share-app";
import type { ShareView, UserApiTokenStatus } from "@/lib/types";

const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";

/**
 * Dashboard Share「连接」弹窗。
 *
 * API Key 是当前登录用户的固定密钥（/v1/me/api-token），与是否为该 Share 的
 * owner / shareto 无关。canViewSecret 只控制能否对这个 Share 发连接测试。
 */
export const ShareConnectDialog = React.memo(function ShareConnectDialog({
  share,
  open,
  onOpenChange,
}: {
  share: ShareView | null;
  open: boolean;
  onOpenChange: (next: boolean) => void;
}) {
  const { t } = useLocaleText();
  const { session, loading } = useAuth();
  const authenticated = !!session?.authenticated;
  const canViewSecret = !!share?.canViewSecret;

  const baseUrl = React.useMemo(() => {
    if (!share?.subdomain) return "";
    if (typeof window === "undefined") return "";
    const host = window.location.host;
    if (!host) return "";
    return `https://${share.subdomain}.${host}`;
  }, [share?.subdomain]);

  const [token, setToken] = React.useState<UserApiTokenStatus | null>(null);
  const [apiTokenPlain, setApiTokenPlain] = React.useState<string>("");
  const [tokenError, setTokenError] = React.useState<string>("");
  const [tokenBusy, setTokenBusy] = React.useState(false);

  React.useEffect(() => {
    if (!open || !authenticated) {
      setToken(null);
      setApiTokenPlain("");
      setTokenError("");
      return;
    }
    let cancelled = false;
    setTokenBusy(true);
    setTokenError("");
    getUserApiToken()
      .then((response) => {
        if (cancelled) return;
        setToken(response.token);
        setApiTokenPlain(response.apiToken || "");
      })
      .catch((err) => {
        if (cancelled) return;
        setTokenError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setTokenBusy(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, authenticated]);

  const apiKeyDisplay = React.useMemo(() => {
    // 优先明文；缺明文（老库没补 plaintext 列）才回落到 prefix + 遮罩 + 提示。
    if (apiTokenPlain) return apiTokenPlain;
    if (token?.prefix) return `${token.prefix}${"•".repeat(16)}`;
    return "";
  }, [apiTokenPlain, token?.prefix]);
  const apiKeyIsPlaintext = !!apiTokenPlain;

  const requestLogin = React.useCallback(() => {
    if (typeof window === "undefined") return;
    window.dispatchEvent(new CustomEvent(ROUTER_OPEN_LOGIN_EVENT));
    onOpenChange(false);
  }, [onOpenChange]);

  const ownerEmail = share?.ownerEmail?.trim() || "";
  const requestAccessHref = React.useMemo(() => {
    if (!ownerEmail) return null;
    const subject = encodeURIComponent(`Request access: ${share?.subdomain || share?.shareId || ""}`);
    return `mailto:${ownerEmail}?subject=${subject}`;
  }, [ownerEmail, share?.subdomain, share?.shareId]);

  const shareApps = shareEnabledApps(share);
  const shareAppLabel = shareApps.map((app) => SHARE_APP_LABELS[app]).join(" / ");

  if (!share) return null;

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light flex max-h-[min(90vh,calc(100vh-1.5rem))] w-[min(640px,calc(100vw-2rem))] max-w-none flex-col overflow-hidden !bg-white !text-slate-900 [--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)] [--overlay:#fff] [--overlay-foreground:rgb(15,23,42)] [--surface:#fff] [--surface-foreground:rgb(15,23,42)]">
            <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
            <Modal.Header className="border-0 shadow-none">
              <div className="pr-8">
                <Modal.Heading>
                  {t("dashboard.connectDialog.title")}
                </Modal.Heading>
                <div className="mt-2 flex flex-wrap items-center gap-2 text-sm text-slate-600">
                  <span className="break-all font-medium text-slate-900">
                    {share.subdomain || share.shareName || share.shareId}
                  </span>
                  {shareApps.map((app) => (
                    <ShareAppLogo key={app} app={app} size={14} />
                  ))}
                </div>
                <p className="mt-1 text-xs text-slate-500">
                  {shareAppLabel
                    ? t("dashboard.connectDialog.appSharedSingle", { app: shareAppLabel })
                    : t("dashboard.connectDialog.appShared")}
                </p>
              </div>
            </Modal.Header>
            <Modal.Body className="grid min-h-0 flex-1 gap-5 overflow-y-auto !px-6 !py-5 !text-slate-900">
              <ConnectSection title={t("dashboard.connectDialog.credentials")}>
                <BaseUrlRow t={t} baseUrl={baseUrl} />
                <ApiKeyRow
                  t={t}
                  state={
                    loading || (authenticated && tokenBusy)
                      ? "loading"
                      : !authenticated
                        ? "unauth"
                        : tokenError
                          ? "error"
                          : "revealable"
                  }
                  apiKeyDisplay={apiKeyDisplay}
                  apiKeyIsPlaintext={apiKeyIsPlaintext}
                  tokenError={tokenError}
                  requestLogin={requestLogin}
                />
              </ConnectSection>
              <ConnectSection title={t("dashboard.connectDialog.test.section")}>
                {authenticated && !canViewSecret ? (
                  <p className="text-xs leading-5 text-slate-500">
                    {t("dashboard.connectDialog.test.needPermission")}
                    {requestAccessHref ? (
                      <>
                        {" · "}
                        <a
                          href={requestAccessHref}
                          className="inline-flex items-center gap-1 font-medium text-slate-700 underline-offset-4 hover:underline"
                        >
                          <Mail className="h-3 w-3" />
                          {t("dashboard.connectDialog.requestAccess")}
                        </a>
                      </>
                    ) : null}
                  </p>
                ) : null}
                <div className="divide-y divide-slate-100">
                  {shareApps.map((app) => (
                    <ShareConnectionTestRow
                      key={`${share.shareId}:${app}:${share.appRuntimes?.[app]?.modelProbe?.healthFingerprint || "no-probe"}`}
                      share={share}
                      app={app}
                      apiToken={apiTokenPlain}
                      baseUrl={baseUrl}
                      authenticated={authenticated}
                      canExecute={authenticated && canViewSecret}
                    />
                  ))}
                </div>
              </ConnectSection>
            </Modal.Body>
            <Modal.Footer className="sticky bottom-0 shrink-0 border-0 bg-white/95 backdrop-blur supports-[backdrop-filter]:bg-white/80">
              <Button
                variant="outline"
                onClick={() => onOpenChange(false)}
              >
                {t("dashboard.connectDialog.close")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
    </Modal.Backdrop>
  );
});

function ConnectSection({
  title,
  children,
}: {
  title: string;
  children: React.ReactNode;
}) {
  return (
    <section className="grid gap-3">
      <h3 className="text-[11px] font-semibold uppercase tracking-[0.08em] text-slate-500">{title}</h3>
      {children}
    </section>
  );
}

function BaseUrlRow({
  t,
  baseUrl,
}: {
  t: ReturnType<typeof useLocaleText>["t"];
  baseUrl: string;
}) {
  return (
    <div className="grid gap-1">
      <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
        {t("dashboard.connectDialog.baseUrl")}
      </span>
      <div className="flex items-start gap-2 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-slate-900">
        <div className="min-w-0 flex-1 break-all font-mono text-xs leading-5">
          {baseUrl || "-"}
        </div>
        <CopyButton value={baseUrl} t={t} />
      </div>
    </div>
  );
}

type ApiKeyState =
  | "loading"
  | "unauth"
  | "revealable"
  | "error";

function ApiKeyRow({
  t,
  state,
  apiKeyDisplay,
  apiKeyIsPlaintext,
  tokenError,
  requestLogin,
}: {
  t: ReturnType<typeof useLocaleText>["t"];
  state: ApiKeyState;
  apiKeyDisplay: string;
  apiKeyIsPlaintext: boolean;
  tokenError: string;
  requestLogin: () => void;
}) {
  return (
    <div className="grid gap-1">
      <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
        {t("dashboard.connectDialog.apiKey")}
      </span>
      {state === "loading" ? (
        <div className="rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-500">
          ···
        </div>
      ) : state === "unauth" ? (
        <div className="grid gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-sm text-amber-900">
          <span>{t("dashboard.connectDialog.loginRequired")}</span>
          <div>
            <Button variant="primary" onClick={requestLogin}>
              <LogIn className="h-4 w-4" />
              {t("dashboard.connectDialog.loginAction")}
            </Button>
          </div>
        </div>
      ) : state === "error" ? (
        <div className="rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          {tokenError || "error"}
        </div>
      ) : (
        // revealable
        <div className="grid gap-2">
          <div className="flex items-start gap-2 rounded-lg border border-slate-200 bg-white px-3 py-2 text-slate-900">
            <div className="min-w-0 flex-1 break-all font-mono text-xs">
              {apiKeyDisplay || "-"}
            </div>
            <CopyButton value={apiKeyDisplay} t={t} />
          </div>
          {apiKeyIsPlaintext ? null : (
            <span className="inline-flex items-center gap-1 text-xs text-slate-500">
              <ExternalLink className="h-3 w-3" />
              {t("dashboard.connectDialog.maskedHint")}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

function CopyButton({
  value,
  t,
}: {
  value: string;
  t: ReturnType<typeof useLocaleText>["t"];
}) {
  const [copied, setCopied] = React.useState(false);
  const copy = React.useCallback(
    async (event: React.MouseEvent<HTMLButtonElement>) => {
      // 全部局部副作用：clipboard 写入 + 本地 setState。不再走 heroui 全局
      // Toast.Provider —— 它依赖 react-aria 的 overlay/inert 机制，新 toast
      // 进出会触发整页 aria-hidden 重算，UI 看起来像"整屏刷新"。
      event.preventDefault();
      event.stopPropagation();
      if (!value) return;
      try {
        await navigator.clipboard.writeText(value);
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      } catch {
        // 静默失败：用户可以手动复制——别用 alert 打断。
      }
    },
    [value],
  );
  return (
    <span className="relative inline-flex shrink-0">
      <button
        type="button"
        onClick={copy}
        disabled={!value}
        title={copied ? t("dashboard.connectDialog.copyOk") : t("dashboard.connectDialog.copy")}
        aria-label={copied ? t("dashboard.connectDialog.copyOk") : t("dashboard.connectDialog.copy")}
        className="inline-flex h-7 w-7 items-center justify-center rounded-md text-slate-500 transition-colors hover:bg-slate-100 hover:text-slate-900 disabled:cursor-not-allowed disabled:opacity-40"
      >
        <Copy className="h-4 w-4" />
      </button>
      {copied ? (
        <span
          role="status"
          aria-live="polite"
          // 局部"已复制"小条：绝对定位、淡入、定时移除。完全不依赖全局
          // Toast.Provider，所以不触发 react-aria 的整页 inert 重算。
          className="pointer-events-none absolute -top-7 right-0 inline-flex animate-fade-in-up items-center gap-1 rounded-md bg-emerald-600 px-2 py-0.5 text-[11px] font-medium text-white shadow-sm"
        >
          <Check className="h-3 w-3" />
          {t("dashboard.connectDialog.copyOk")}
        </span>
      ) : null}
    </span>
  );
}

export { ROUTER_OPEN_LOGIN_EVENT };
