"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { Check, Copy, ExternalLink, LogIn, Mail } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getUserApiToken } from "@/lib/api";
import { ShareConnectionTestRow } from "@/components/dashboard/share-connection-test";
import type { ShareView, UserApiTokenStatus } from "@/lib/types";

const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";

/**
 * P18: 让 dashboard ShareTable 行点击「连接」时打开这个弹窗。
 *
 * 状态机（按 useAuth().session?.authenticated 与 share.canViewSecret 联动）：
 *   - 未登录   → 提示登录 + 触发 LoginDialog
 *   - 已登录 + canViewSecret 为假 → 「没有权限」 + mailto 申请加入
 *   - 已登录 + canViewSecret 为真 → 拉 /v1/me/api-token，展示前缀脱敏 +
 *     "去顶部 API Token 面板重置" 提示。后端只在 reset 时返回明文，所以这里
 *     不重复 reset 入口，集中在 <ApiTokenDialog>。
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
  // P18.1: 后端 `/v1/me/api-token` 在 user_api_tokens.token_plaintext 列里持久化
  // 明文（store.rs:10879），任意时刻调用都能拿到 raw token。这里直接展示明文，
  // 不再加遮罩——share owner 已经过 canViewSecret 校验，复制即用。
  const [apiTokenPlain, setApiTokenPlain] = React.useState<string>("");
  const [tokenError, setTokenError] = React.useState<string>("");
  const [tokenBusy, setTokenBusy] = React.useState(false);

  // 拉一次 token 明文；若后端因为是老库没有 plaintext，apiToken 为空，回落到
  // prefix 提示去重置。
  React.useEffect(() => {
    if (!open || !authenticated || !canViewSecret) {
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
  }, [open, authenticated, canViewSecret]);

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

  if (!share) return null;

  return (
    <Modal isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Backdrop>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900 [--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)] [--overlay:#fff] [--overlay-foreground:rgb(15,23,42)] [--surface:#fff] [--surface-foreground:rgb(15,23,42)]">
            <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
            <Modal.Header>
              <div>
                <Modal.Heading>
                  {t("dashboard.connectDialog.title")}
                </Modal.Heading>
                <p className="mt-1 text-sm text-slate-600">
                  {t("dashboard.connectDialog.appShared")}
                </p>
              </div>
            </Modal.Header>
            <Modal.Body className="grid gap-4">
              <BaseUrlRow t={t} baseUrl={baseUrl} />
              <ApiKeyRow
                t={t}
                state={
                  loading
                    ? "loading"
                    : !authenticated
                      ? "unauth"
                      : !canViewSecret
                        ? "forbidden"
                        : tokenBusy
                          ? "loading"
                          : tokenError
                            ? "error"
                            : "revealable"
                }
                apiKeyDisplay={apiKeyDisplay}
                apiKeyIsPlaintext={apiKeyIsPlaintext}
                tokenError={tokenError}
                requestLogin={requestLogin}
                requestAccessHref={requestAccessHref}
              />
              {/* P18: test rows — always render (disabled states show explanatory text) */}
              <div className="grid gap-2">
                <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
                  {t("dashboard.connectDialog.test.section")}
                </span>
                {(["claude", "codex", "gemini"] as const).map((app) => (
                  <ShareConnectionTestRow
                    key={app}
                    share={share}
                    app={app}
                    apiToken={apiTokenPlain}
                    baseUrl={baseUrl}
                    canExecute={authenticated && canViewSecret}
                  />
                ))}
              </div>
            </Modal.Body>
            <Modal.Footer>
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
    </Modal>
  );
});

function BaseUrlRow({
  t,
  baseUrl,
}: {
  t: ReturnType<typeof useLocaleText>["t"];
  baseUrl: string;
}) {
  return (
    <div className="grid gap-2">
      <span className="text-xs font-medium uppercase tracking-wide text-slate-500">
        {t("dashboard.connectDialog.baseUrl")}
      </span>
      <div className="flex items-start gap-2 rounded-lg border border-slate-200 bg-slate-50 px-3 py-2 text-slate-900">
        <div className="min-w-0 flex-1 break-all font-mono text-xs">
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
  | "forbidden"
  | "revealable"
  | "error";

function ApiKeyRow({
  t,
  state,
  apiKeyDisplay,
  apiKeyIsPlaintext,
  tokenError,
  requestLogin,
  requestAccessHref,
}: {
  t: ReturnType<typeof useLocaleText>["t"];
  state: ApiKeyState;
  apiKeyDisplay: string;
  apiKeyIsPlaintext: boolean;
  tokenError: string;
  requestLogin: () => void;
  requestAccessHref: string | null;
}) {
  return (
    <div className="grid gap-2">
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
      ) : state === "forbidden" ? (
        <div className="grid gap-2 rounded-lg border border-red-200 bg-red-50 px-3 py-2 text-sm text-red-700">
          <span>{t("dashboard.connectDialog.forbidden")}</span>
          {requestAccessHref ? (
            <a
              href={requestAccessHref}
              className="inline-flex w-fit items-center gap-1 text-xs font-semibold text-red-800 underline-offset-4 hover:underline"
            >
              <Mail className="h-3 w-3" />
              {t("dashboard.connectDialog.requestAccess")}
            </a>
          ) : null}
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
