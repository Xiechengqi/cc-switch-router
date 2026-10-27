"use client";

import { Alert, Button } from "@heroui/react";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { homeEntryStatus, saveHomeEntry } from "@/lib/auth";
import { HOME_ENTRIES, type HomeEntry } from "@/lib/home-entry";

const LABELS: Record<HomeEntry, "start.home.clients" | "nav.clientMarketTab" | "nav.shareMarketTab" | "account.nav.wallet" | "nav.providersTab"> = {
  clients: "start.home.clients",
  "client-market": "nav.clientMarketTab",
  "share-market": "nav.shareMarketTab",
  wallet: "account.nav.wallet",
  providers: "nav.providersTab",
};

export function AccountHomePage() {
  const { t } = useLocaleText();
  const { session, loading } = useAuth();
  const [selected, setSelected] = React.useState<HomeEntry>("clients");
  const [ready, setReady] = React.useState(false);
  const [saving, setSaving] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const [error, setError] = React.useState(false);

  React.useEffect(() => {
    if (loading || !session?.authenticated) return;
    let active = true;
    homeEntryStatus()
      .then((status) => { if (active && status.entry) setSelected(status.entry); })
      .catch(() => { if (active) setError(true); })
      .finally(() => { if (active) setReady(true); });
    return () => { active = false; };
  }, [loading, session?.authenticated]);

  const save = () => {
    setSaving(true);
    setSaved(false);
    setError(false);
    void saveHomeEntry(selected, "choice")
      .then(() => setSaved(true))
      .catch(() => setError(true))
      .finally(() => setSaving(false));
  };

  return (
    <section className="grid max-w-xl gap-4">
      <h1 className="text-lg font-semibold">{t("start.home.title")}</h1>
      <p className="text-sm text-muted-foreground">{t("start.home.lead")}</p>
      {!ready ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground" role="status">
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          {t("common.loading")}
        </div>
      ) : (
        <fieldset className="grid gap-2">
          <legend className="sr-only">{t("start.home.title")}</legend>
          {HOME_ENTRIES.map((entry) => (
            <label key={entry} className="flex items-center gap-2 text-sm">
              <input
                type="radio"
                name="home-entry"
                value={entry}
                checked={selected === entry}
                onChange={() => { setSelected(entry); setSaved(false); }}
              />
              {t(LABELS[entry])}
            </label>
          ))}
        </fieldset>
      )}
      {error ? <Alert status="danger">{t("start.error")}</Alert> : null}
      {saved ? <p className="text-sm text-muted-foreground" role="status">{t("start.home.saved")}</p> : null}
      <Button className="w-fit" isDisabled={!ready || saving} onPress={save}>
        {saving ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : null}
        {t("start.home.save")}
      </Button>
    </section>
  );
}
