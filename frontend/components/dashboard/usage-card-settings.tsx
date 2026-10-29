"use client";

import * as React from "react";
import { Checkbox } from "@heroui/react";
import { ChevronRight, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { AccountInlineLoading, AccountSectionError } from "@/components/dashboard/account-section";
import { UsageEmbedBuilder } from "@/components/dashboard/usage-embed-builder";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMyUsageCardSettings, updateMyUsageCardSettings } from "@/lib/api";
import type { UsageCardSettingsResponse } from "@/lib/types";
import {
  DEFAULT_EMBED_OPTIONS,
  loadUsageCardPreferences,
  saveUsageCardPreferences,
  type UsageEmbedOptions,
} from "@/lib/usage-card-preferences";

/**
 * The public usage card: whether it shows anything, and the snippet to embed
 * it. Collapsed by default; it is about sharing usage, not reading it, so its
 * settings load only once opened.
 */
export function UsageCardSettings() {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  return (
    <details
      className="group min-w-0 rounded-xl border border-border bg-card"
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="flex min-h-11 cursor-pointer list-none items-center gap-2 px-4 py-3 text-sm font-semibold text-foreground focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring sm:px-5">
        <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-90 motion-reduce:transition-none" aria-hidden />
        {t("account.usage.cardSummary")}
      </summary>
      {open ? <UsageCardSettingsBody /> : null}
    </details>
  );
}

function UsageCardSettingsBody() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const [cardSettings, setCardSettings] = React.useState<UsageCardSettingsResponse | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [attempt, setAttempt] = React.useState(0);
  const [host, setHost] = React.useState("router.example.com");
  const [embedOptions, setEmbedOptions] = React.useState<UsageEmbedOptions>({ ...DEFAULT_EMBED_OPTIONS });
  const mutationRef = React.useRef(0);

  React.useEffect(() => {
    if (window.location.host) setHost(window.location.host);
  }, []);

  React.useEffect(() => {
    let cancelled = false;
    mutationRef.current += 1;
    setCardSettings(null);
    setBusy(false);
    setEmbedOptions({ ...DEFAULT_EMBED_OPTIONS });
    setLoading(true);
    setError("");
    getMyUsageCardSettings()
      .then((data) => {
        if (cancelled) return;
        setCardSettings(data);
        setEmbedOptions(loadUsageCardPreferences(window.localStorage, data.userId));
      })
      .catch((err) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [attempt, session?.user?.id]);

  const updatePublicStats = async (enabled: boolean) => {
    if (!cardSettings || busy) return;
    const previous = cardSettings;
    const mutationId = mutationRef.current + 1;
    mutationRef.current = mutationId;
    setCardSettings({ ...cardSettings, publicStatsEnabled: enabled });
    setBusy(true);
    setError("");
    try {
      const next = await updateMyUsageCardSettings({ publicStatsEnabled: enabled });
      if (mutationRef.current === mutationId) setCardSettings(next);
    } catch (err) {
      if (mutationRef.current === mutationId) {
        setCardSettings(previous);
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (mutationRef.current === mutationId) setBusy(false);
    }
  };

  const updateEmbedOptions = (next: UsageEmbedOptions) => {
    setEmbedOptions(next);
    if (cardSettings?.userId) saveUsageCardPreferences(window.localStorage, cardSettings.userId, next);
  };

  return (
    <div className="grid gap-3 border-t border-border px-4 py-4 sm:px-5">
      <p className="text-sm text-muted-foreground">{t("account.consumerUsage.cardHint")}</p>
      {error ? <AccountSectionError message={error} onRetry={cardSettings ? undefined : () => setAttempt((value) => value + 1)} /> : null}
      {loading && !cardSettings ? <AccountInlineLoading className="py-2" /> : null}
      {cardSettings ? (
        <>
          <div className="grid gap-1 text-sm">
            <span className="text-muted-foreground">{t("account.consumerUsage.cardIdentity")}</span>
            <span className="break-all font-mono text-foreground">{cardSettings.email}</span>
          </div>
          <Checkbox
            isSelected={cardSettings.publicStatsEnabled}
            isDisabled={busy}
            onChange={(value: boolean) => void updatePublicStats(value)}
          >
            <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
              <Checkbox.Indicator />
            </Checkbox.Control>
            <Checkbox.Content>
              <span className="inline-flex items-center gap-2 text-sm text-foreground">
                {t("account.consumerUsage.publicToggle")}
                {busy ? <Loader2 className="h-3.5 w-3.5 animate-spin motion-reduce:animate-none" /> : null}
              </span>
            </Checkbox.Content>
          </Checkbox>
        </>
      ) : null}
      <div className="grid gap-2">
        <h4 className="text-sm font-semibold text-foreground">{t("account.consumerUsage.embed")}</h4>
        <p className="text-sm text-muted-foreground">{t("account.consumerUsage.embedHint")}</p>
        <UsageEmbedBuilder
          host={host}
          userId={cardSettings?.userId || ""}
          enabled={!!cardSettings?.userId && !!cardSettings.publicStatsEnabled}
          options={embedOptions}
          onChange={updateEmbedOptions}
          t={t}
        />
      </div>
    </div>
  );
}
