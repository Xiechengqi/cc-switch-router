"use client";

import * as React from "react";
import { Alert, Button, Checkbox, Chip, Input, TextArea } from "@heroui/react";
import { KeyRound, Loader2, LogOut, Save } from "lucide-react";
import { getPublicMarkets, getShareApiAuth, getShareContext, getSharePageShare, updateSharePageSettings, readShareApiCredentials, writeShareApiCredentials, clearShareApiCredentials, ShareApiError } from "@/lib/share-api";
import type { PublicMarket, ShareAccessByApp, ShareApiAuthResponse, ShareApiContextResponse, ShareView } from "@/lib/types";
import {
  buildShareSettingsPatch,
  draftFromShare,
  fromDateTimeLocal,
  MIN_PARALLEL_LIMIT,
  normalizeEmailList,
  PERMANENT_EXPIRES_AT_ISO,
  toDateTimeLocal,
  UNLIMITED_PARALLEL_LIMIT,
  UNLIMITED_TOKEN_LIMIT,
  validateShareSettingsDraft,
  type ShareSettingsDraft,
} from "@/lib/share-settings";
import { compactTokens, formatDateTime } from "@/lib/utils";

const PRICE_APPS = [
  { key: "claude", label: "Claude" },
  { key: "codex", label: "Codex" },
  { key: "gemini", label: "Gemini" },
] as const;
type ShareAppKey = (typeof PRICE_APPS)[number]["key"];

function statusTone(online: boolean) {
  return online ? "success" : "default";
}

function tokenLabel(value: number) {
  return value < 0 ? "∞" : compactTokens(value);
}

function shareAccessApps(share: ShareView): ShareAppKey[] {
  const apps = PRICE_APPS.map((app) => app.key);
  const bound = apps.filter((app) => Boolean(share.bindings?.[app]));
  return bound.length ? bound : [...apps];
}

function accessByAppFromShare(share: ShareView): ShareAccessByApp {
  if (share.accessByApp && Object.keys(share.accessByApp).length > 0) return share.accessByApp;
  const result: ShareAccessByApp = {};
  for (const app of shareAccessApps(share)) {
    result[app] = {
      sharedWithEmails: share.sharedWithEmails || [],
      marketAccessMode: share.marketAccessMode === "all" ? "all" : "selected",
    };
  }
  return result;
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
  markets,
  editable,
  onSaved,
}: {
  share: ShareView;
  markets: PublicMarket[];
  editable: boolean;
  onSaved: () => Promise<void>;
}) {
  const [draft, setDraft] = React.useState<ShareSettingsDraft>(() => draftFromShare(share));
  const [sharedTextByApp, setSharedTextByApp] = React.useState<Record<ShareAppKey, string>>(() => {
    const access = accessByAppFromShare(share);
    return {
      claude: (access.claude?.sharedWithEmails || []).join(", "),
      codex: (access.codex?.sharedWithEmails || []).join(", "),
      gemini: (access.gemini?.sharedWithEmails || []).join(", "),
    };
  });
  const [expiryPermanent, setExpiryPermanent] = React.useState(() => draft.expiresAt === PERMANENT_EXPIRES_AT_ISO || new Date(draft.expiresAt).getUTCFullYear() >= 2099);
  const [expiryLocal, setExpiryLocal] = React.useState(() => toDateTimeLocal(draft.expiresAt));
  const [tokenUnlimited, setTokenUnlimited] = React.useState(draft.tokenLimit === UNLIMITED_TOKEN_LIMIT);
  const [parallelUnlimited, setParallelUnlimited] = React.useState(draft.parallelLimit === UNLIMITED_PARALLEL_LIMIT);
  const [busy, setBusy] = React.useState(false);
  const [notice, setNotice] = React.useState("");
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    const next = draftFromShare(share);
    const access = accessByAppFromShare(share);
    setDraft(next);
    setSharedTextByApp({
      claude: (access.claude?.sharedWithEmails || []).join(", "),
      codex: (access.codex?.sharedWithEmails || []).join(", "),
      gemini: (access.gemini?.sharedWithEmails || []).join(", "),
    });
    setExpiryPermanent(next.expiresAt === PERMANENT_EXPIRES_AT_ISO || new Date(next.expiresAt).getUTCFullYear() >= 2099);
    setExpiryLocal(toDateTimeLocal(next.expiresAt));
    setTokenUnlimited(next.tokenLimit === UNLIMITED_TOKEN_LIMIT);
    setParallelUnlimited(next.parallelLimit === UNLIMITED_PARALLEL_LIMIT);
  }, [share]);

  const effectiveDraft = React.useMemo<ShareSettingsDraft>(() => {
    const expiresAt = expiryPermanent ? PERMANENT_EXPIRES_AT_ISO : fromDateTimeLocal(expiryLocal);
    const accessByApp: ShareAccessByApp = {};
    for (const app of shareAccessApps(share)) {
      accessByApp[app] = {
        sharedWithEmails: normalizeEmailList(sharedTextByApp[app] || ""),
        marketAccessMode: draft.marketAccessMode,
      };
    }
    const sharedWithEmails = normalizeEmailList(
      Object.values(accessByApp).flatMap((access) => access?.sharedWithEmails ?? []),
    );
    return {
      ...draft,
      sharedWithEmails,
      accessByApp,
      tokenLimit: tokenUnlimited ? UNLIMITED_TOKEN_LIMIT : draft.tokenLimit,
      parallelLimit: parallelUnlimited ? UNLIMITED_PARALLEL_LIMIT : draft.parallelLimit,
      expiresAt,
    };
  }, [draft, expiryLocal, expiryPermanent, parallelUnlimited, share, sharedTextByApp, tokenUnlimited]);
  const validationErrors = validateShareSettingsDraft(effectiveDraft);

  const save = async () => {
    if (!editable || busy || validationErrors.length) return;
    setBusy(true);
    setError("");
    setNotice("");
    try {
      const result = await updateSharePageSettings(buildShareSettingsPatch(effectiveDraft));
      setNotice(result.appliedSynchronously ? "Settings applied." : "Settings queued; waiting for desktop sync.");
      await onSaved();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const selectedMarketEmails = new Set(
    Object.values(effectiveDraft.accessByApp).flatMap((access) =>
      (access?.sharedWithEmails ?? []).map((email) => email.toLowerCase()),
    ),
  );

  return (
    <div className="grid gap-4">
      {notice ? <Alert status="success" className="!text-slate-900">{notice}</Alert> : null}
      {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
      {validationErrors.length ? (
        <Alert status="warning" className="!text-slate-900">{validationErrors[0]}</Alert>
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

        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">For sale</span>
          <select
            className="h-10 rounded-lg border border-border bg-white px-3 text-sm"
            value={draft.forSale}
            disabled={!editable}
            onChange={(event) => setDraft((current) => ({ ...current, forSale: event.target.value as "Yes" | "No" | "Free" }))}
          >
            <option value="No">No</option>
            <option value="Yes">Yes</option>
            <option value="Free">Free</option>
          </select>
        </label>

        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">Market access</span>
          <select
            className="h-10 rounded-lg border border-border bg-white px-3 text-sm"
            value={draft.marketAccessMode}
            disabled={!editable || draft.forSale !== "Yes"}
            onChange={(event) => setDraft((current) => ({ ...current, marketAccessMode: event.target.value as "selected" | "all" }))}
          >
            <option value="selected">Selected markets</option>
            <option value="all">All markets</option>
          </select>
        </label>
      </div>

      {draft.forSale === "Yes" && draft.marketAccessMode === "selected" ? (
        <div className="grid gap-2">
          <div className="text-sm font-medium text-foreground">Authorized markets</div>
          <div className="flex flex-wrap gap-2">
            {markets.map((market) => {
              const checked = selectedMarketEmails.has(market.email.toLowerCase());
              return (
                <label key={market.id} className="inline-flex items-center gap-2 rounded-full border border-border px-3 py-1.5 text-xs">
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={!editable}
                    onChange={(event) => {
                      const next = new Set(selectedMarketEmails);
                      if (event.target.checked) next.add(market.email.toLowerCase());
                      else next.delete(market.email.toLowerCase());
                      setSharedTextByApp((current) => {
                        const result = { ...current };
                        for (const app of shareAccessApps(share)) {
                          const appEmails = new Set(normalizeEmailList(current[app] || ""));
                          if (event.target.checked) appEmails.add(market.email.toLowerCase());
                          else appEmails.delete(market.email.toLowerCase());
                          result[app] = Array.from(appEmails).sort().join(", ");
                        }
                        return result;
                      });
                    }}
                  />
                  {market.displayName || market.subdomain}
                </label>
              );
            })}
          </div>
        </div>
      ) : null}

      <div className="grid gap-2">
        <span className="text-sm font-medium text-foreground">Shared with emails</span>
        {shareAccessApps(share).map((app) => {
          const label = PRICE_APPS.find((item) => item.key === app)?.label ?? app;
          return (
            <label key={app} className="grid gap-1 text-sm">
              <span className="text-xs text-muted-foreground">{label}</span>
              <Input
                value={sharedTextByApp[app] || ""}
                disabled={!editable}
                placeholder="friend@example.com, teammate@example.com"
                onChange={(event) =>
                  setSharedTextByApp((current) => ({ ...current, [app]: event.target.value }))
                }
              />
            </label>
          );
        })}
      </div>

      <div className="grid gap-4 lg:grid-cols-3">
        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">Token limit</span>
          <Input
            type="number"
            value={tokenUnlimited ? "" : String(draft.tokenLimit)}
            placeholder="Unlimited"
            disabled={!editable || tokenUnlimited}
            onChange={(event) => setDraft((current) => ({ ...current, tokenLimit: Number.parseInt(event.target.value, 10) || 0 }))}
          />
          <Checkbox isSelected={tokenUnlimited} isDisabled={!editable} onChange={(value: boolean) => setTokenUnlimited(value)}>
            <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
            <Checkbox.Content><span className="text-xs text-muted-foreground">Unlimited</span></Checkbox.Content>
          </Checkbox>
        </label>

        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">Parallel limit</span>
          <Input
            type="number"
            min={MIN_PARALLEL_LIMIT}
            value={parallelUnlimited ? "" : String(draft.parallelLimit)}
            placeholder="Unlimited"
            disabled={!editable || parallelUnlimited}
            onChange={(event) => setDraft((current) => ({ ...current, parallelLimit: Number.parseInt(event.target.value, 10) || 0 }))}
          />
          <Checkbox isSelected={parallelUnlimited} isDisabled={!editable} onChange={(value: boolean) => setParallelUnlimited(value)}>
            <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
            <Checkbox.Content><span className="text-xs text-muted-foreground">Unlimited</span></Checkbox.Content>
          </Checkbox>
        </label>

        <label className="grid gap-1 text-sm">
          <span className="font-medium text-foreground">Expires at</span>
          <Input
            type="datetime-local"
            value={expiryLocal}
            disabled={!editable || expiryPermanent}
            onChange={(event) => setExpiryLocal(event.target.value)}
          />
          <Checkbox isSelected={expiryPermanent} isDisabled={!editable} onChange={(value: boolean) => setExpiryPermanent(value)}>
            <Checkbox.Control><Checkbox.Indicator /></Checkbox.Control>
            <Checkbox.Content><span className="text-xs text-muted-foreground">Permanent</span></Checkbox.Content>
          </Checkbox>
        </label>
      </div>

      <div className="grid gap-3">
        <div className="text-sm font-medium text-foreground">Model pricing percent</div>
        <div className="grid gap-3 sm:grid-cols-3">
          {PRICE_APPS.map((app) => {
            const supported = Boolean(share.support?.[app.key]);
            return (
              <label key={app.key} className="grid gap-1 text-sm">
                <span className="text-xs text-muted-foreground">{app.label}</span>
                <Input
                  type="number"
                  min={1}
                  max={100}
                  value={draft.pricing[app.key] == null ? "" : String(draft.pricing[app.key])}
                  placeholder={supported ? "Unset" : "No node"}
                  disabled={!editable || !supported}
                  onChange={(event) => {
                    const raw = event.target.value;
                    setDraft((current) => {
                      const pricing = { ...current.pricing };
                      if (!raw.trim()) delete pricing[app.key];
                      else pricing[app.key] = Number.parseInt(raw, 10) || 0;
                      return { ...current, pricing };
                    });
                  }}
                />
              </label>
            );
          })}
        </div>
      </div>

      {editable ? (
        <div className="flex justify-end">
          <Button variant="primary" isDisabled={busy || validationErrors.length > 0} onClick={() => void save()}>
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
  const [markets, setMarkets] = React.useState<PublicMarket[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [currentHost, setCurrentHost] = React.useState("");
  const editable = Boolean(auth?.canManage && share?.canEditSettings);

  const load = React.useCallback(async () => {
    setLoading(true);
    try {
      const shareContext = await getShareContext();
      setContext(shareContext);
      const [shareResponse, marketsResponse] = await Promise.all([
        getSharePageShare(),
        getPublicMarkets().catch(() => ({ markets: [] })),
      ]);
      setShare(shareResponse.share);
      setAuth(shareResponse.auth);
      setMarkets(marketsResponse.markets || []);
      setError("");
    } catch (err) {
      if (err instanceof ShareApiError && (err.status === 401 || err.status === 403)) {
        setShare(null);
        setAuth({ authenticated: false, canManage: false });
        setMarkets([]);
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
            <ShareSettingsForm share={share} markets={markets} editable={editable} onSaved={load} />
          </section>
        ) : null}
      </div>
    </main>
  );
}
