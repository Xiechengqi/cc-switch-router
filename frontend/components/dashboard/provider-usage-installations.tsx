"use client";

import * as React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { UsageModelTable } from "@/components/dashboard/account-usage-shared";
import type { ProviderInstallationUsage, ProviderShareUsage } from "@/lib/types";
import { compactTokens } from "@/lib/utils";

function ShareBlock({ share }: { share: ProviderShareUsage }) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  const sortedModels = React.useMemo(() => {
    const rows = [...(share.models || [])];
    rows.sort((a, b) => b.totalTokens - a.totalTokens);
    return rows;
  }, [share.models]);

  return (
    <div className="rounded-lg border border-border bg-background">
      <button
        type="button"
        aria-expanded={open}
        className="flex min-h-11 w-full items-center gap-2 px-3 py-2.5 text-left"
        onClick={() => setOpen((value) => !value)}
      >
        {open ? (
          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        )}
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-foreground">
          {share.shareName || share.shareId}
        </span>
        {share.shareName ? (
          <span className="hidden max-w-[140px] truncate font-mono text-[10px] text-muted-foreground sm:inline">
            {share.shareId}
          </span>
        ) : null}
        <span className="font-mono text-sm font-semibold tabular-nums text-foreground">
          {compactTokens(share.totalTokens)}
        </span>
      </button>
      {open ? (
        <div className="grid gap-3 border-t px-3 py-3">
          <div>
            <div className="mb-1.5 text-[11px] font-mono uppercase tracking-[0.08em] text-muted-foreground">
              {t("account.usage.models")}
            </div>
            <UsageModelTable models={sortedModels} t={t} emptyMessage={t("account.usage.empty")} />
          </div>
          {(share.callers || []).length ? (
            <div>
              <div className="mb-1.5 text-[11px] font-mono uppercase tracking-[0.08em] text-muted-foreground">
                {t("account.providerUsage.caller")}
              </div>
              <div className="overflow-hidden rounded-md border">
                <table className="w-full table-fixed border-collapse text-[11px]">
                  <colgroup>
                    <col className="w-[55%]" />
                    <col className="w-[45%]" />
                  </colgroup>
                  <thead className="bg-muted/50 text-left font-mono uppercase tracking-[0.08em] text-muted-foreground">
                    <tr>
                      <th className="px-1.5 py-2">{t("account.providerUsage.caller")}</th>
                      <th className="px-1.5 py-2 text-right">{t("account.usage.total")}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {(share.callers || []).map((row) => (
                      <tr key={row.email} className="border-t">
                        <td className="overflow-hidden px-1.5 py-2">{row.email || "-"}</td>
                        <td className="overflow-hidden px-1.5 py-2 text-right font-mono font-semibold tabular-nums">
                          {compactTokens(row.totalTokens)}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}

function InstallationBlock({ installation }: { installation: ProviderInstallationUsage }) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(true);
  const label = installation.label || installation.installationId;
  return (
    <section className="grid gap-2 rounded-xl border border-border bg-card p-3 sm:p-4">
      <button
        type="button"
        aria-expanded={open}
        className="flex w-full items-center gap-3 text-left"
        onClick={() => setOpen((value) => !value)}
      >
        {open ? (
          <ChevronDown className="h-4 w-4 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-4 w-4 shrink-0 text-muted-foreground" />
        )}
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-semibold text-foreground">{label}</div>
          <div className="truncate font-mono text-[11px] text-muted-foreground/80">
            {installation.installationId}
          </div>
        </div>
        <div className="text-right">
          <div className="font-mono text-base font-semibold tabular-nums tracking-tight text-foreground">
            {compactTokens(installation.totalTokens)}
          </div>
          <div className="text-[11px] text-muted-foreground">
            {t("account.providerUsage.shares")}: {installation.shares.length}
          </div>
        </div>
      </button>
      {open ? (
        <div className="grid gap-2 pl-1 sm:pl-2">
          {installation.shares.map((share) => (
            <ShareBlock key={share.shareId} share={share} />
          ))}
        </div>
      ) : null}
    </section>
  );
}

/** Provider usage by Client installation, then by Share, busiest first. */
export function ProviderUsageInstallations({ installations }: { installations: ProviderInstallationUsage[] }) {
  const { t } = useLocaleText();
  return (
    <section className="grid gap-2">
      <h3 className="text-sm font-semibold text-foreground">{t("account.providerUsage.installations")}</h3>
      {installations.length ? (
        <div className="grid gap-3">
          {installations.map((installation) => (
            <InstallationBlock key={installation.installationId} installation={installation} />
          ))}
        </div>
      ) : (
        <p className="rounded-lg border border-dashed border-border px-3 py-6 text-center text-sm text-muted-foreground">
          {t("account.providerUsage.noInstallations")}
        </p>
      )}
    </section>
  );
}
