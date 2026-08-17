"use client";

import { BellRing, FileClock, ScrollText, ServerCog } from "lucide-react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { AdminAuditPanel } from "@/components/settings/admin-audit-panel";
import { ClientNotificationDeliveriesPanel } from "@/components/settings/client-notification-deliveries-panel";
import { LogsPanel } from "@/components/settings/logs-panel";
import { VersionPanel } from "@/components/settings/version-panel";

type OperationsSection = "service" | "logs" | "deliveries" | "audit";

const ITEMS = [
  { id: "service" as const, icon: ServerCog, label: "operations.service" as const },
  { id: "logs" as const, icon: ScrollText, label: "operations.logs" as const },
  { id: "deliveries" as const, icon: BellRing, label: "operations.deliveries" as const },
  { id: "audit" as const, icon: FileClock, label: "operations.audit" as const },
];

export function OperationsPage() {
  const { session, loading } = useAuth();
  const { t } = useLocaleText();
  const [active, setActive] = React.useState<OperationsSection>("service");

  if (loading) {
    return <main className="settings-surface mx-auto w-[calc(100%-2rem)] max-w-7xl py-12 text-muted-foreground">{t("common.loadingSession")}</main>;
  }
  if (!session?.isAdmin) {
    return (
      <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-4xl gap-4 py-12 text-foreground">
        <h1 className="font-display text-3xl">{t("settings.adminRequired")}</h1>
        <p className="text-muted-foreground">{t("settings.adminRequiredDesc")}</p>
      </main>
    );
  }

  return (
    <main className="settings-surface mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-10 text-foreground">
      <header className="border-b pb-5 pt-2">
        <h1 className="font-display text-3xl">{t("operations.title")}</h1>
        <p className="mt-2 max-w-3xl text-sm text-muted-foreground">{t("operations.description")}</p>
      </header>
      <section className="grid gap-5 lg:grid-cols-[230px_minmax(0,1fr)]">
        <aside className="h-fit border-r pr-4 lg:sticky lg:top-4">
          <nav aria-label={t("operations.navigation")} className="grid gap-1">
            {ITEMS.map((item) => {
              const Icon = item.icon;
              return (
                <button
                  key={item.id}
                  type="button"
                  onClick={() => setActive(item.id)}
                  className={`flex min-h-10 items-center gap-2 px-2.5 text-left text-sm transition-colors ${active === item.id ? "bg-primary/10 font-medium text-foreground" : "text-muted-foreground hover:bg-muted/50 hover:text-foreground"}`}
                >
                  <Icon className="h-4 w-4" />
                  {t(item.label)}
                </button>
              );
            })}
          </nav>
        </aside>
        <div className="min-w-0">
          {active === "service" ? <VersionPanel isAdmin /> : null}
          {active === "logs" ? <LogsPanel /> : null}
          {active === "deliveries" ? <ClientNotificationDeliveriesPanel /> : null}
          {active === "audit" ? <AdminAuditPanel /> : null}
        </div>
      </section>
    </main>
  );
}
