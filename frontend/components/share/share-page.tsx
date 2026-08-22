"use client";

import * as React from "react";
import { Alert, Button, Checkbox, Chip, Input, TextArea } from "@heroui/react";
import { KeyRound, Loader2, LogOut, Save } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ShareUserGrantsEditor } from "@/components/dashboard/share-edit/share-user-grants-editor";
import { getShareApiAuth, getShareContext, getSharePageShare, updateSharePageSettings, readShareApiCredentials, writeShareApiCredentials, clearShareApiCredentials, ShareApiError } from "@/lib/share-api";
import type { ShareApiAuthResponse, ShareApiContextResponse, ShareView } from "@/lib/types";
import {
  buildShareSettingsPatch,
  draftFromShare,
  fromDateTimeLocal,
  PERMANENT_EXPIRES_AT_ISO,
  shareSettingsFieldErrors,
  shareSettingsHasFieldErrors,
  toDateTimeLocal,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  type ShareSettingsDraft,
} from "@/lib/share-settings";
import {
  formatShareCeilingParallel,
  formatShareCeilingToken,
  ShareCeilingBar,
} from "@/components/dashboard/share-edit/share-ceiling-bar";
import { FieldGroup } from "@/components/dashboard/share-edit/share-edit-shared";
import { compactTokens, formatDateTime } from "@/lib/utils";
import { millionsInputToTokens, tokensToMillionsInput } from "@/lib/token-units";

function statusTone(online: boolean) {
  return online ? "success" : "default";
}

function tokenLabel(value: number) {
  return value < 0 ? "∞" : compactTokens(value);
}

function AuthPanel({
  auth,
  ownerEmail,
  onAuthenticated,
}: {
  auth: ShareApiAuthResponse | null;
  ownerEmail?: string;
  onAuthenticated: () => Promise<void>;
}) {
  const initial = readShareApiCredentials();
  const [email, setEmail] = React.useState(initial.email || ownerEmail || "");
  const [token, setToken] = React.useState(initial.token || "");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");

  const submit = async () => {
    if (!email.trim() || !token.trim() || busy) return;
    setBusy(true);
    setError("");
    try {
      writeShareApiCredentials(email, token);
      const nextAuth = await getShareApiAuth(email, token);
      if (!nextAuth.authenticated) throw new Error("API token is invalid.");
      if (!nextAuth.canManage) throw new Error(`Only owner ${ownerEmail || "of this share"} can edit this share.`);
      await onAuthenticated();
    } catch (err) {
      clearShareApiCredentials();
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const logout = async () => {
    clearShareApiCredentials();
    setToken("");
    await onAuthenticated();
  };

  if (auth?.authenticated) {
    return (
      <div className="flex flex-col gap-2 rounded-lg border border-border bg-card px-4 py-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="text-sm">
          <span className={auth.canManage ? "text-emerald-700" : "text-amber-700"}>
            {auth.canManage
              ? `API token owner verified: ${auth.user?.email || "-"}`
              : `Signed in as ${auth.user?.email || "-"}; only owner ${ownerEmail || "-"} can edit.`}
          </span>
        </div>
        <Button size="sm" variant="outline" onClick={() => void logout()}>
          <LogOut className="h-4 w-4" />
          Sign out
        </Button>
      </div>
    );
  }

  return (
    <div className="rounded-lg border border-border bg-card px-4 py-3">
      <div className="grid gap-2 md:grid-cols-[minmax(180px,260px)_minmax(220px,1fr)_auto] md:items-center">
        <Input
          type="email"
          value={email}
          placeholder={ownerEmail || "owner@example.com"}
          onChange={(event) => setEmail(event.target.value)}
        />
        <Input
          type="password"
          value={token}
          placeholder="Router API token"
          onChange={(event) => setToken(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void submit();
          }}
        />
        <Button variant="primary" isDisabled={busy || !email.trim() || !token.trim()} onClick={() => void submit()}>
          {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <KeyRound className="h-4 w-4" />}
          Unlock edit
        </Button>
      </div>
      {error ? <div className="mt-2 text-xs text-red-600">{error}</div> : null}
    </div>
  );
}

function ShareSettingsForm({
  share,
  editable,
  onSaved,
}: {
  share: ShareView;
  editable: boolean;
  onSaved: () => Promise<void>;
}) {
  const { t } = useLocaleText();
  const [draft, setDraft] = React.useState<ShareSettingsDraft>(() => draftFromShare(share));
  const [expiryPermanent, setExpiryPermanent] = React.useState(() => draft.expiresAt === PERMANENT_EXPIRES_AT_ISO || new Date(draft.expiresAt).getUTCFullYear() >= 2099);
  const [expiryLocal, setExpiryLocal] = React.useState(() => toDateTimeLocal(draft.expiresAt));
  const [tokenUnlimited, setTokenUnlimited] = React.useState(draft.tokenLimit === UNLIMITED_TOKEN_LIMIT);
  const [tokenLimitInput, setTokenLimitInput] = React.useState(() =>
    draft.tokenLimit === UNLIMITED_TOKEN_LIMIT ? "" : tokensToMillionsInput(draft.tokenLimit),
  );
  const [parallelUnlimited, setParallelUnlimited] = React.useState(draft.parallelLimit === UNLIMITED_PARALLEL_LIMIT);
  const [busy, setBusy] = React.useState(false);
  const [notice, setNotice] = React.useState("");
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    const next = draftFromShare(share);
    setDraft(next);
    setExpiryPermanent(next.expiresAt === PERMANENT_EXPIRES_AT_ISO || new Date(next.expiresAt).getUTCFullYear() >= 2099);
    setExpiryLocal(toDateTimeLocal(next.expiresAt));
    setTokenUnlimited(next.tokenLimit === UNLIMITED_TOKEN_LIMIT);
    setTokenLimitInput(
      next.tokenLimit === UNLIMITED_TOKEN_LIMIT ? "" : tokensToMillionsInput(next.tokenLimit),
    );
    setParallelUnlimited(next.parallelLimit === UNLIMITED_PARALLEL_LIMIT);
  }, [share]);

  const effectiveDraft = React.useMemo<ShareSettingsDraft>(() => {
    const expiresAt = expiryPermanent ? PERMANENT_EXPIRES_AT_ISO : fromDateTimeLocal(expiryLocal);
    return {
      ...draft,
      tokenLimit: tokenUnlimited
        ? UNLIMITED_TOKEN_LIMIT
        : millionsInputToTokens(tokenLimitInput) ?? 0,
      parallelLimit: parallelUnlimited ? UNLIMITED_PARALLEL_LIMIT : draft.parallelLimit,
      expiresAt,
    };
  }, [draft, expiryLocal, expiryPermanent, parallelUnlimited, tokenLimitInput, tokenUnlimited]);
  const fieldErrors = shareSettingsFieldErrors(effectiveDraft);
  const formInvalid = shareSettingsHasFieldErrors(fieldErrors);
  const ceilingInvalid =
    fieldErrors.tokenLimit || fieldErrors.parallelLimit || fieldErrors.expiresAt;
  const firstValidationMessage = fieldErrors.description
    ? t("dashboard.shareSettings.invalidDescription")
    : fieldErrors.tokenLimit
      ? t("dashboard.shareSettings.invalidToken")
      : fieldErrors.parallelLimit
        ? t("dashboard.shareSettings.invalidParallel")
        : fieldErrors.expiresAt
          ? t("dashboard.shareSettings.invalidExpiry")
          : "";
  const defaultUserPolicy = {
    parallelLimit:
      effectiveDraft.parallelLimit === UNLIMITED_PARALLEL_LIMIT
        ? undefined
        : effectiveDraft.parallelLimit,
    tokenLimit:
      effectiveDraft.tokenLimit === UNLIMITED_TOKEN_LIMIT
        ? undefined
        : effectiveDraft.tokenLimit,
    tokenPeriod: "lifetime" as const,
    expiresAt: expiryPermanent
      ? undefined
      : new Date(effectiveDraft.expiresAt).getTime(),
  };

  const save = async () => {
    if (!editable || busy || formInvalid) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const result = await updateSharePageSettings(buildShareSettingsPatch(effectiveDraft, share));
      setNotice(
        result.appliedSynchronously
          ? t("dashboard.shareEditApplied")
          : t("dashboard.shareEditQueued"),
      );
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4">
      {notice ? <Alert status="success" className="!text-slate-900">{notice}</Alert> : null}
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {firstValidationMessage ? (
        <Alert status="warning" className="!text-slate-900">{firstValidationMessage}</Alert>
      ) : null}

      <div className="grid gap-4 lg:grid-cols-2">
        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">Description</span>
          <TextArea
            value={draft.description}
            maxLength={200}
            disabled={!editable}
            onChange={(event) => setDraft((current) => ({ ...current, description: event.target.value }))}
          />
        </label>

        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">Subdomain</span>
          <Input value={share.subdomain} disabled />
        </label>

        <div className="grid gap-2 rounded-lg border border-border px-3 py-2 text-sm lg:col-span-2">
          <Checkbox
            isSelected={draft.freeAccess}
            isDisabled={!editable}
            onChange={(freeAccess: boolean) =>
              setDraft((current) => ({ ...current, freeAccess }))
            }
          >
            <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
            <Checkbox.Content>Public free access</Checkbox.Content>
          </Checkbox>
          <p className="text-xs text-muted-foreground">
            Disabled is private. When enabled, any signed-in Router user may invoke this Share; individual grants below override quotas.
          </p>
        </div>
      </div>

      {editable ? (
        <ShareUserGrantsEditor
          value={draft.userGrants}
          ownerEmail={share.ownerEmail || ""}
          defaultPolicy={defaultUserPolicy}
          supportedPeriods={share.supportedUserTokenPeriods}
          t={t}
          shareId={share.shareId}
          onChange={(userGrants) =>
            setDraft((current) => ({ ...current, userGrants }))
          }
        />
      ) : (
        <div className="grid gap-2">
          <span className="text-sm font-medium text-foreground">{t("dashboard.userLimit.title")}</span>
          <div className="flex flex-wrap gap-2">
            {Object.values(draft.userGrants)
              .filter((grant) => grant.active !== false)
              .map((grant) => (
                <Chip key={grant.email} size="sm" variant="soft">
                  {grant.email}
                </Chip>
              ))}
          </div>
        </div>
      )}

      <ShareCeilingBar
        t={t}
        tokenDisplay={formatShareCeilingToken(effectiveDraft.tokenLimit, tokenUnlimited, t)}
        parallelDisplay={formatShareCeilingParallel(effectiveDraft.parallelLimit, parallelUnlimited, t)}
        expiryDisplay={
          expiryPermanent
            ? t("dashboard.userLimit.permanent")
            : formatDateTime(effectiveDraft.expiresAt) || "—"
        }
        editable={editable}
        invalid={ceilingInvalid}
      >
        <div className="grid gap-3 md:grid-cols-3">
          <FieldGroup label={t("dashboard.field.tokenLimit")} invalid={fieldErrors.tokenLimit}>
            <div className="grid gap-2">
              <Input
                type="text"
                inputMode="decimal"
                value={tokenUnlimited ? "" : tokenLimitInput}
                placeholder={t("common.unlimited")}
                disabled={!editable || tokenUnlimited}
                onChange={(event) => setTokenLimitInput(event.target.value)}
              />
              <Checkbox isSelected={tokenUnlimited} isDisabled={!editable} onChange={(value: boolean) => setTokenUnlimited(value)}>
                <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                <Checkbox.Content><span className="text-xs text-muted-foreground">{t("common.unlimited")}</span></Checkbox.Content>
              </Checkbox>
            </div>
          </FieldGroup>
          <FieldGroup label={t("dashboard.field.parallelLimit")} invalid={fieldErrors.parallelLimit}>
            <div className="grid gap-2">
              <Input
                type="number"
                min={1}
                value={parallelUnlimited ? "" : String(draft.parallelLimit)}
                placeholder={t("common.unlimited")}
                disabled={!editable || parallelUnlimited}
                onChange={(event) => setDraft((current) => ({ ...current, parallelLimit: Number.parseInt(event.target.value, 10) || 0 }))}
              />
              <Checkbox isSelected={parallelUnlimited} isDisabled={!editable} onChange={(value: boolean) => setParallelUnlimited(value)}>
                <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                <Checkbox.Content><span className="text-xs text-muted-foreground">{t("common.unlimited")}</span></Checkbox.Content>
              </Checkbox>
            </div>
          </FieldGroup>
          <FieldGroup label={t("dashboard.field.expiresAt")} invalid={fieldErrors.expiresAt}>
            <div className="grid gap-2">
              <Input
                type="datetime-local"
                value={expiryLocal}
                disabled={!editable || expiryPermanent}
                onChange={(event) => setExpiryLocal(event.target.value)}
              />
              <Checkbox isSelected={expiryPermanent} isDisabled={!editable} onChange={(value: boolean) => setExpiryPermanent(value)}>
                <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
                <Checkbox.Content><span className="text-xs text-muted-foreground">{t("dashboard.permanent")}</span></Checkbox.Content>
              </Checkbox>
            </div>
          </FieldGroup>
        </div>
      </ShareCeilingBar>

      {editable ? (
        <div className="flex justify-end">
          <Button variant="primary" isDisabled={busy || formInvalid} onClick={() => void save()}>
            {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
            Save settings
          </Button>
        </div>
      ) : null}
    </div>
  );
}

export function SharePage() {
  const [context, setContext] = React.useState<ShareApiContextResponse | null>(null);
  const [share, setShare] = React.useState<ShareView | null>(null);
  const [auth, setAuth] = React.useState<ShareApiAuthResponse | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [currentHost, setCurrentHost] = React.useState("");
  const editable = Boolean(auth?.canManage && share?.canEditSettings);

  const load = React.useCallback(async () => {
    setLoading(true);
    try {
      const shareContext = await getShareContext();
      setContext(shareContext);
      const shareResponse = await getSharePageShare();
      setShare(shareResponse.share);
      setAuth(shareResponse.auth);
      setError("");
    } catch (err) {
      if (err instanceof ShareApiError && (err.status === 401 || err.status === 403)) {
        setShare(null);
        setAuth({ authenticated: false, canManage: false });
        setError("");
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    load().catch(console.error);
  }, [load]);

  React.useEffect(() => {
    setCurrentHost(window.location.host || window.location.hostname || "");
  }, []);

  return (
    <main className="min-h-screen bg-background px-4 py-5 text-foreground">
      <div className="mx-auto grid max-w-5xl gap-5">
        <header className="flex flex-col gap-3 border-b border-border pb-4 sm:flex-row sm:items-end sm:justify-between">
          <div>
            <div className="flex flex-wrap items-center gap-2">
              <h1 className="text-2xl font-semibold tracking-normal">{share?.shareName || context?.subdomain || "Share"}</h1>
              {share ? <Chip color={statusTone(share.isOnline)} size="sm" variant="soft">{share.isOnline ? "online" : share.shareStatus}</Chip> : null}
            </div>
            <p className="mt-1 break-all text-sm text-muted-foreground">{currentHost || context?.subdomain || "Loading share..."}</p>
          </div>
          {share ? (
            <div className="grid gap-1 text-right text-xs text-muted-foreground">
              <span>Owner: {share.ownerEmail || "-"}</span>
              <span>Usage: {tokenLabel(share.tokensUsed)} / {tokenLabel(share.tokenLimit)}</span>
            </div>
          ) : null}
        </header>

        {context ? (
          <AuthPanel auth={auth} ownerEmail={share?.ownerEmail} onAuthenticated={load} />
        ) : null}

        {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
        {loading && !share ? <div className="py-16 text-center text-sm text-muted-foreground">Loading...</div> : null}

        {share ? (
          <section className="grid gap-4 rounded-lg border border-border bg-card p-4">
            <div className="grid gap-3 sm:grid-cols-3">
              <div>
                <div className="text-xs uppercase text-muted-foreground">App</div>
                <div className="mt-1 font-medium">{share.appType}</div>
              </div>
              <div>
                <div className="text-xs uppercase text-muted-foreground">Parallel</div>
                <div className="mt-1 font-medium">{share.activeRequests} / {share.parallelLimit < 0 ? "∞" : share.parallelLimit}</div>
              </div>
              <div>
                <div className="text-xs uppercase text-muted-foreground">Expires</div>
                <div className="mt-1 font-medium">{share.expiresAt ? formatDateTime(share.expiresAt) : "-"}</div>
              </div>
            </div>
            <ShareSettingsForm share={share} editable={editable} onSaved={load} />
          </section>
        ) : null}
      </div>
    </main>
  );
}
