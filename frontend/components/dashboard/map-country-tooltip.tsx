"use client";

import * as React from "react";
import { ChevronDown, ChevronRight } from "lucide-react";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import type { CountryBoard } from "@/lib/types";
import { cn } from "@/lib/utils";

function countryFlag(code?: string) {
  const cc = (code || "").trim().slice(0, 2).toUpperCase();
  if (!/^[A-Z]{2}$/.test(cc)) return "·";
  return String.fromCodePoint(...[...cc].map((ch) => 127397 + ch.charCodeAt(0)));
}

function stateTone(state: string) {
  if (state === "offline") return "text-rose-700";
  if (state === "degraded") return "text-amber-700";
  return "text-emerald-700";
}

export function MapCountryTooltip({
  board,
  className,
  style,
}: {
  board: CountryBoard;
  className?: string;
  style?: React.CSSProperties;
}) {
  const { t } = useLocaleText();
  const focus = useDashboardFocus();
  const defaultExpanded = board.clientCount <= 3;
  const [expandedClients, setExpandedClients] = React.useState<Record<string, boolean>>(() => {
    if (!defaultExpanded) return {};
    return Object.fromEntries(board.clients.map((client) => [client.installationId, true]));
  });

  React.useEffect(() => {
    if (!defaultExpanded) {
      setExpandedClients({});
      return;
    }
    setExpandedClients(Object.fromEntries(board.clients.map((client) => [client.installationId, true])));
  }, [board.countryCodeIso3, board.clients, defaultExpanded]);

  const title = board.countryName || board.countryCode;

  return (
    <div
      className={cn(
        "pointer-events-auto w-[min(92vw,360px)] select-text rounded-xl border border-slate-200/90 bg-white/95 p-3 text-left shadow-[0_16px_40px_rgba(15,23,42,0.14)] backdrop-blur-md",
        className,
      )}
      style={style}
      data-map-control
    >
      <div className="mb-2 flex items-start justify-between gap-3">
        <div>
          <div className="text-sm font-semibold text-foreground">
            {countryFlag(board.countryCode)} {title}
          </div>
          <div className="mt-1 text-[11px] text-muted-foreground">
            {t("map.countrySummary", {
              clients: board.clientCount,
              shares: board.shareCount,
              inflight: board.inflightRequests,
            })}
          </div>
        </div>
        <div className="rounded-md bg-slate-100 px-2 py-1 text-[10px] font-medium text-slate-600">
          {t("map.onlineShares", { count: board.onlineShareCount })}
        </div>
      </div>

      <div className="max-h-[min(52vh,320px)] space-y-2 overflow-auto pr-1">
        {board.clients.map((client) => {
          const expanded = expandedClients[client.installationId] ?? false;
          return (
            <div key={client.installationId} className="rounded-lg border border-slate-200/80 bg-slate-50/70">
              <button
                type="button"
                className="flex w-full items-center gap-2 px-2.5 py-2 text-left"
                onClick={() => {
                  setExpandedClients((current) => ({
                    ...current,
                    [client.installationId]: !expanded,
                  }));
                }}
              >
                {expanded ? <ChevronDown className="h-3.5 w-3.5 shrink-0 text-slate-500" /> : <ChevronRight className="h-3.5 w-3.5 shrink-0 text-slate-500" />}
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[12px] font-semibold text-foreground">{client.label}</div>
                  <div className="truncate text-[10px] text-muted-foreground">
                    {t("map.clientShareCount", { shares: client.shareCount })}
                    {client.ownerEmail ? ` · ${client.ownerEmail}` : ""}
                  </div>
                </div>
                <span className={cn("shrink-0 text-[10px] font-medium capitalize", stateTone(client.operationalState))}>
                  {client.operationalState}
                </span>
              </button>
              {expanded ? (
                <div className="space-y-1 border-t border-slate-200/70 px-2.5 py-2">
                  <button
                    type="button"
                    className="text-[10px] font-medium text-primary hover:underline"
                    onClick={() => {
                      focus.setFocus({ kind: "client", id: client.installationId, source: "map" });
                    }}
                  >
                    {t("map.openClient")}
                  </button>
                  {client.shares.map((share) => (
                    <button
                      key={share.shareId}
                      type="button"
                      className="flex w-full items-center justify-between gap-2 rounded-md px-2 py-1.5 text-left hover:bg-white"
                      onClick={() => {
                        focus.setFocus({ kind: "share", id: share.shareId, source: "map" });
                        focus.openDrawer("share", share.shareId);
                      }}
                    >
                      <div className="min-w-0">
                        <div className="truncate text-[11px] font-medium text-foreground">{share.subdomain || share.shareName}</div>
                        <div className="truncate text-[10px] text-muted-foreground">{share.appType}</div>
                      </div>
                      <div className="shrink-0 text-right text-[10px]">
                        <div className={cn("font-medium capitalize", stateTone(share.operationalState))}>{share.operationalState}</div>
                        {share.activeRequests > 0 ? (
                          <div className="text-primary">{t("map.active", { count: share.activeRequests })}</div>
                        ) : null}
                      </div>
                    </button>
                  ))}
                  {client.overflowShareCount ? (
                    <div className="px-2 text-[10px] text-muted-foreground">
                      {t("map.moreShares", { count: client.overflowShareCount })}
                    </div>
                  ) : null}
                </div>
              ) : null}
            </div>
          );
        })}
        {board.overflowClientCount ? (
          <div className="px-1 text-[10px] text-muted-foreground">
            {t("map.moreClients", { count: board.overflowClientCount })}
          </div>
        ) : null}
      </div>
    </div>
  );
}
