"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import type { CountryBoard } from "@/lib/types";
import { cn } from "@/lib/utils";

function countryFlag(code?: string) {
  const cc = (code || "").trim().slice(0, 2).toUpperCase();
  if (!/^[A-Z]{2}$/.test(cc)) return "·";
  return String.fromCodePoint(...[...cc].map((ch) => 127397 + ch.charCodeAt(0)));
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
  const title = board.countryName || board.countryCode;

  return (
    <div
      className={cn(
        "pointer-events-none w-[min(92vw,280px)] select-none rounded-xl border border-slate-200/90 bg-white/95 p-3 text-left shadow-[0_16px_40px_rgba(15,23,42,0.14)] backdrop-blur-md",
        className,
      )}
      style={style}
      data-map-country-tooltip
    >
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="text-sm font-semibold text-foreground">
            {countryFlag(board.countryCode)} {title}
          </div>
          <div className="mt-2 space-y-1 text-[11px] text-muted-foreground">
            <div>{t("map.countryClients", { count: board.clientCount })}</div>
            <div>{t("map.countryShares", { count: board.shareCount })}</div>
            <div>{t("map.countryInflight", { count: board.inflightRequests })}</div>
          </div>
        </div>
        <div className="shrink-0 rounded-md bg-slate-100 px-2 py-1 text-[10px] font-medium text-slate-600">
          {t("map.onlineShares", { count: board.onlineShareCount })}
        </div>
      </div>
    </div>
  );
}
