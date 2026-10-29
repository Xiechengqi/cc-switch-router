"use client";

import { Button } from "@heroui/react";
import { House, Loader2, Monitor, Network, Share2, Trophy, WalletCards } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { ChoiceCard } from "@/components/common/choice-card";
import { AccountInlineLoading, AccountSectionError } from "@/components/dashboard/account-section";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { homeEntryStatus, saveHomeEntry } from "@/lib/auth";
import { useCachedResource } from "@/lib/cached-resource";
import { HOME_ENTRY_SECTION_ID } from "@/lib/dashboard-nav";
import { HOME_ENTRIES, rememberHomeEntryStatus, type HomeEntry } from "@/lib/home-entry";

const OPTIONS: Record<HomeEntry, {
  icon: typeof House;
  labelKey: "start.home.clients" | "nav.clientMarketTab" | "nav.shareMarketTab" | "account.nav.wallet" | "nav.providersTab";
}> = {
  clients: { icon: Monitor, labelKey: "start.home.clients" },
  "client-market": { icon: Network, labelKey: "nav.clientMarketTab" },
  "share-market": { icon: Share2, labelKey: "nav.shareMarketTab" },
  wallet: { icon: WalletCards, labelKey: "account.nav.wallet" },
  providers: { icon: Trophy, labelKey: "nav.providersTab" },
};

/** Which page the product home opens for this account. */
export function HomeEntryCard() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const actor = session?.user?.id || "";
  const status = useCachedResource(actor ? `home-entry:${actor}` : null, homeEntryStatus);
  const [choice, setChoice] = React.useState<HomeEntry | null>(null);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const [saveError, setSaveError] = React.useState(false);
  const current = status.data?.entry ?? null;
  const selected = choice ?? current ?? "clients";

  React.useEffect(() => {
    if (actor && status.data) rememberHomeEntryStatus(actor, status.data);
  }, [actor, status.data]);

  const save = () => {
    setSaving(true);
    setSaved(false);
    setSaveError(false);
    void saveHomeEntry(selected, "choice")
      .then((next) => {
        rememberHomeEntryStatus(actor, next);
        status.mutate(() => next);
        setChoice(null);
        setSaved(true);
      })
      .catch(() => setSaveError(true))
      .finally(() => setSaving(false));
  };

  return (
    <section id={HOME_ENTRY_SECTION_ID} aria-labelledby="home-entry-title" className="grid min-w-0 scroll-mt-20 gap-4 rounded-xl border border-border bg-card p-4 sm:p-5">
      <div>
        <h3 id="home-entry-title" className="flex items-center gap-2 text-sm font-semibold text-foreground">
          <House className="h-4 w-4 text-muted-foreground" aria-hidden />
          {t("start.home.title")}
        </h3>
        <p className="mt-0.5 text-sm text-muted-foreground">{t("account.security.homeHint")}</p>
      </div>
      {status.data === undefined ? (
        status.error ? <AccountSectionError message={t("start.error")} onRetry={() => void status.refresh()} /> : <AccountInlineLoading />
      ) : (
        <>
          <div role="radiogroup" aria-labelledby="home-entry-title" className="grid gap-2 sm:grid-cols-2">
            {HOME_ENTRIES.map((entry) => (
              <ChoiceCard
                key={entry}
                name="home-entry"
                value={entry}
                checked={selected === entry}
                onChange={(value) => { setChoice(value as HomeEntry); setSaved(false); }}
                icon={OPTIONS[entry].icon}
                title={t(OPTIONS[entry].labelKey)}
                selectedLabel={t("common.selected")}
              />
            ))}
          </div>
          {saveError ? <AccountSectionError message={t("start.error")} /> : null}
          <div className="flex flex-wrap items-center gap-3">
            <Button isDisabled={saving || selected === current} onPress={save}>
              {saving ? <Loader2 className="h-4 w-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
              {t("start.home.save")}
            </Button>
            {saved ? <p className="text-sm text-muted-foreground" role="status">{t("start.home.saved")}</p> : null}
          </div>
        </>
      )}
    </section>
  );
}
