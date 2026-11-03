"use client";

import Image from "next/image";
import { usePathname, useRouter } from "next/navigation";
import { Button } from "@heroui/react";
import { Loader2 } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { SharePage } from "@/components/share/share-page";
import { getUiContext, rootTaskHref, type UiContext } from "@/lib/home-entry";

/** Host resolution sits outside the shared shell and never interprets a
 * network/404 failure as permission to show a Router dashboard. */
export function RootPage({ children }: { children: React.ReactNode }) {
  const pathname = usePathname() || "/";
  const router = useRouter();
  const { t } = useLocaleText();
  const [context, setContext] = React.useState<UiContext | null>(null);
  const [error, setError] = React.useState(false);
  const [attempt, setAttempt] = React.useState(0);
  React.useEffect(() => {
    const controller = new AbortController();
    setError(false);
    getUiContext(controller.signal).then(v => { if (!controller.signal.aborted) setContext(v); })
      .catch(() => { if (!controller.signal.aborted) setError(true); });
    return () => controller.abort();
  }, [attempt]);
  React.useEffect(() => {
    if (context?.mode !== "router" || pathname !== "/") return;
    const destination = rootTaskHref(window.location.search);
    if (destination) router.replace(destination + window.location.hash);
  }, [context, pathname, router]);
  if (!context) return (
    <main className="grid min-h-dvh place-items-center px-4" aria-busy={!error}>
      <div className="grid justify-items-center gap-3 text-center" role={error ? "alert" : "status"}>
        <Image src="/router-logo.svg" alt="" width={40} height={40} priority />
        {error ? <><p>{t("home.loadError")}</p><Button onPress={() => setAttempt(v => v + 1)}>{t("common.retry")}</Button></> : <><Loader2 className="h-4 w-4 animate-spin" aria-hidden /><span>{t("common.loading")}</span></>}
      </div>
    </main>
  );
  if (context.mode === "share") return <SharePage />;
  return children;
}
