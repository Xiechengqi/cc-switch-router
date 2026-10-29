"use client";

import Image from "next/image";
import { useRouter } from "next/navigation";
import { Alert, Button } from "@heroui/react";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { saveHomeEntry } from "@/lib/auth";
import { pathForHomeEntry, rememberHomeEntryStatus, type HomeEntry, type HomeEntryStatus } from "@/lib/home-entry";

const CHOICES: { entry: Exclude<HomeEntry, "clients">; titleKey: "start.rentClient" | "start.buyShare" | "start.addBalance" | "start.sell"; subtitleKey: "nav.clientMarketTab" | "nav.shareMarketTab" | "account.nav.wallet" | "nav.providersTab" }[] = [
  { entry: "client-market", titleKey: "start.rentClient", subtitleKey: "nav.clientMarketTab" },
  { entry: "share-market", titleKey: "start.buyShare", subtitleKey: "nav.shareMarketTab" },
  { entry: "wallet", titleKey: "start.addBalance", subtitleKey: "account.nav.wallet" },
  { entry: "providers", titleKey: "start.sell", subtitleKey: "nav.providersTab" },
];

export function StartPage({ onSaved }: { onSaved?: (status: HomeEntryStatus) => void }) {
  const { t } = useLocaleText();
  const { session, logout } = useAuth();
  const router = useRouter();
  const [pending, setPending] = React.useState<HomeEntry | "skip" | null>(null);
  const [error, setError] = React.useState(false);
  const recommended = CHOICES[0];
  const rest = CHOICES.slice(1);

  const choose = (entry: HomeEntry, source: "choice" | "skip") => {
    if (pending) return;
    setPending(source === "skip" ? "skip" : entry);
    setError(false);
    void saveHomeEntry(entry, source)
      .then((status) => {
        rememberHomeEntryStatus(session?.user?.id || "", status);
        // The gate leaves this page itself, even when the chosen entry is
        // the address already open and the navigation changes nothing.
        onSaved?.(status);
        router.replace(pathForHomeEntry(entry));
      })
      .catch(() => {
        setPending(null);
        setError(true);
      });
  };

  return (
    <main className="mx-auto grid min-h-dvh w-[calc(100%-2rem)] max-w-3xl content-center gap-8 py-10">
      <div className="grid justify-items-center gap-3 text-center">
        <Image src="/router-logo.svg" alt="" width={40} height={40} priority />
        <h1 className="text-2xl font-semibold tracking-tight">{t("start.title")}</h1>
        <p className="max-w-xl text-sm text-muted-foreground">{t("start.summary")}</p>
      </div>
      <div className="grid gap-3">
        <button
          type="button"
          className="grid gap-1 rounded-lg bg-slate-950 px-5 py-5 text-left text-white disabled:opacity-60"
          onClick={() => choose(recommended.entry, "choice")}
          disabled={pending !== null}
          aria-describedby="start-recommended"
        >
          <span id="start-recommended" className="text-xs font-medium text-slate-300">{t("start.recommended")}</span>
          <span className="text-lg font-semibold">{t(recommended.titleKey)}</span>
          <span className="text-sm text-slate-300">{t(recommended.subtitleKey)}</span>
        </button>
        <div className="grid gap-3 sm:grid-cols-3">
          {rest.map((choice) => (
            <button
              key={choice.entry}
              type="button"
              className="grid min-w-0 gap-1 rounded-lg bg-white px-4 py-4 text-left ring-1 ring-slate-200 disabled:opacity-60"
              onClick={() => choose(choice.entry, "choice")}
              disabled={pending !== null}
            >
              <span className="text-sm font-semibold">{t(choice.titleKey)}</span>
              <span className="text-xs text-muted-foreground">{t(choice.subtitleKey)}</span>
            </button>
          ))}
        </div>
      </div>
      {error ? <Alert status="danger">{t("start.error")}</Alert> : null}
      <div className="flex items-center justify-center gap-4">
        <Button variant="ghost" isDisabled={pending !== null} onPress={() => choose("clients", "skip")}>
          {pending === "skip" ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : null}
          {t("start.skip")}
        </Button>
        <Button variant="ghost" isDisabled={pending !== null} onPress={() => void logout()}>
          {t("nav.logout")}
        </Button>
      </div>
    </main>
  );
}
