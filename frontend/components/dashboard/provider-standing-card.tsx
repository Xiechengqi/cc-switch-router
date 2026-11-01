"use client";

/* Hallmark · component: Provider standing · genre: modern-minimal · theme: existing Router system
 * states: loading · error · not a Provider · not listed · collecting · ranked · renaming · saved
 * Where a Provider stands in the public ranking, and the name it is shown
 * under. Someone who is not selling yet gets the two ways to start instead.
 */

import * as React from "react";
import Link from "next/link";
import { Button } from "@heroui/react";
import { ArrowUpRight, Loader2, Pencil, Server, Share2, Trophy } from "lucide-react";
import { AccountInlineLoading, AccountSectionError } from "@/components/dashboard/account-section";
import {
  ProviderBadges,
  RankMark,
  RankProgressChips,
  useProviderName,
} from "@/components/dashboard/provider-rank-parts";
import { formatBasisPointScore, isFormallyRanked } from "@/components/dashboard/providers-page-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, getMyMarketProvider, updateMyMarketProviderDisplayName } from "@/lib/api";
import { clientMarketAddHostHref, providerRankingHref, shareMarketHref } from "@/lib/dashboard-nav";
import type { MyMarketProvider } from "@/lib/types";

const NAME_ERROR_CODES = [
  "provider_name_length",
  "provider_name_invalid",
  "provider_name_reserved",
  "provider_name_taken",
  "provider_profile_missing",
] as const;
type NameErrorCode = (typeof NAME_ERROR_CODES)[number];

function nameErrorCode(reason: unknown): NameErrorCode | null {
  return reason instanceof ApiError && NAME_ERROR_CODES.includes(reason.code as NameErrorCode)
    ? reason.code as NameErrorCode
    : null;
}

/** Mounted per signed-in actor by the Provider center, so state never crosses accounts. */
export function ProviderStandingCard() {
  const { locale, t } = useLocaleText();
  const nameOf = useProviderName();
  const [view, setView] = React.useState<MyMarketProvider | null>(null);
  const [failed, setFailed] = React.useState(false);
  const [attempt, setAttempt] = React.useState(0);
  const [editing, setEditing] = React.useState(false);
  const [draft, setDraft] = React.useState("");
  const [saving, setSaving] = React.useState(false);
  const [nameError, setNameError] = React.useState<NameErrorCode | "generic" | null>(null);
  const [saved, setSaved] = React.useState(false);
  const inputId = React.useId();

  React.useEffect(() => {
    const controller = new AbortController();
    setFailed(false);
    void getMyMarketProvider(controller.signal)
      .then((next) => { if (!controller.signal.aborted) setView(next); })
      .catch(() => { if (!controller.signal.aborted) setFailed(true); });
    return () => controller.abort();
  }, [attempt]);

  if (!view) {
    return failed
      ? <AccountSectionError message={t("providerStanding.loadError")} onRetry={() => setAttempt((value) => value + 1)} />
      : <AccountInlineLoading />;
  }

  const profile = view.profile;
  if (!profile) {
    return (
      <section className="grid gap-4 rounded-xl border border-border bg-card p-5 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
        <div className="grid gap-1">
          <h3 className="text-base font-semibold">{t("providerStanding.start.title")}</h3>
          <p className="max-w-2xl text-sm leading-6 text-muted-foreground">{t("providerStanding.start.body")}</p>
        </div>
        <div className="flex flex-wrap gap-2">
          <Link
            href={shareMarketHref({ workspace: "selling" })}
            className="inline-flex min-h-11 items-center gap-2 rounded-xl bg-gradient-to-r from-accent to-accent-secondary px-4 text-sm font-medium text-white shadow-sm outline-none transition-all duration-200 hover:-translate-y-0.5 hover:shadow-[0_8px_24px_rgba(0,82,255,0.35)] focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 active:scale-[0.98] motion-reduce:transform-none"
          >
            <Share2 className="h-4 w-4" aria-hidden="true" />
            {t("providerStanding.start.share")}
          </Link>
          <Link
            href={clientMarketAddHostHref()}
            className="inline-flex min-h-11 items-center gap-2 rounded-xl border border-border px-4 text-sm font-medium text-foreground outline-none transition-colors hover:border-primary/30 hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
          >
            <Server className="h-4 w-4" aria-hidden="true" />
            {t("providerStanding.start.host")}
          </Link>
        </div>
      </section>
    );
  }

  const standing = view.standing;
  const generation = view.generation;
  const ranked = standing ? isFormallyRanked(standing) : false;
  const limits = { min: view.displayNameMinChars, max: view.displayNameMaxChars };

  const startEditing = () => {
    setDraft(profile.displayName);
    setNameError(null);
    setSaved(false);
    setEditing(true);
  };
  const save = async (event: React.FormEvent) => {
    event.preventDefault();
    if (saving) return;
    setSaving(true);
    setNameError(null);
    try {
      setView(await updateMyMarketProviderDisplayName(draft));
      setEditing(false);
      setSaved(true);
    } catch (reason) {
      setNameError(nameErrorCode(reason) ?? "generic");
    } finally {
      setSaving(false);
    }
  };

  return (
    <section className="grid gap-4 rounded-xl border border-border bg-card p-5" aria-labelledby="provider-standing-title">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-4">
        <div className="grid min-w-0 gap-1.5">
          <h3 id="provider-standing-title" className="flex items-center gap-2 text-xs font-medium uppercase tracking-[0.12em] text-muted-foreground">
            <Trophy className="h-3.5 w-3.5" aria-hidden="true" />
            {t("providerStanding.title")}
          </h3>
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <strong className="min-w-0 text-lg font-semibold [overflow-wrap:anywhere]">
              {standing ? nameOf(standing) : profile.official ? t("providers.officialName") : profile.displayName}
            </strong>
            {standing ? <ProviderBadges provider={standing} /> : null}
            {!editing && !profile.official ? (
              <Button variant="ghost" size="sm" className="min-h-9" onPress={startEditing}>
                <Pencil className="h-3.5 w-3.5" aria-hidden="true" />
                {t("providerStanding.name.edit")}
              </Button>
            ) : null}
          </div>
          {profile.official ? <p className="text-xs text-muted-foreground">{t("providerStanding.name.officialNote")}</p> : null}
          {saved ? <p role="status" className="text-xs text-emerald-700">{t("providerStanding.name.saved")}</p> : null}
        </div>
        {standing ? (
          <div className="flex items-center gap-4">
            {ranked ? (
              <span className="text-right">
                <span className="block text-[11px] text-muted-foreground">{t("providers.score")}</span>
                <strong className="block font-mono text-lg font-semibold tabular-nums">{formatBasisPointScore(standing.scoreBps, locale)}</strong>
              </span>
            ) : null}
            <RankMark provider={standing} />
          </div>
        ) : null}
      </div>

      {editing ? (
        <form className="grid gap-2" onSubmit={(event) => void save(event)}>
          <label htmlFor={inputId} className="text-sm font-medium">{t("providerStanding.name.label")}</label>
          <div className="flex flex-wrap gap-2">
            <input
              id={inputId}
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              maxLength={limits.max * 2}
              autoFocus
              aria-invalid={!!nameError}
              aria-describedby={`${inputId}-hint`}
              className="min-h-11 min-w-0 flex-1 basis-60 rounded-lg border border-border bg-card px-3 text-sm outline-2 outline-offset-1 outline-transparent focus-visible:outline-ring"
            />
            <Button type="submit" variant="primary" className="min-h-11" isDisabled={saving}>
              {saving ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden="true" /> : null}
              {t("common.save")}
            </Button>
            <Button variant="ghost" className="min-h-11" isDisabled={saving} onPress={() => setEditing(false)}>
              {t("common.cancel")}
            </Button>
          </div>
          <p id={`${inputId}-hint`} className={nameError ? "text-xs text-danger" : "text-xs text-muted-foreground"} role={nameError ? "alert" : undefined}>
            {nameError === "generic"
              ? t("providerStanding.loadError")
              : nameError
                ? t(`providerStanding.name.error.${nameError}`, limits)
                : t("providerStanding.name.hint", { ...limits, fallback: profile.defaultDisplayName })}
          </p>
        </form>
      ) : null}

      {!standing ? (
        <p className="text-sm text-muted-foreground">
          {generation ? t("providerStanding.notListed") : t("providerStanding.notReady")}
        </p>
      ) : !ranked && generation ? (
        <div className="grid gap-1.5">
          <span className="text-xs font-medium text-muted-foreground">{t("providers.progress.label")}</span>
          <RankProgressChips provider={standing} generation={generation} />
        </div>
      ) : null}

      {standing ? (
        <Link
          href={providerRankingHref(standing.id)}
          className="inline-flex min-h-11 w-fit items-center gap-1 text-sm font-medium text-primary outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring"
        >
          {t("providerStanding.viewRanking")}
          <ArrowUpRight className="h-4 w-4" aria-hidden="true" />
        </Link>
      ) : null}
    </section>
  );
}
