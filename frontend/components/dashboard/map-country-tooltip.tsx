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
        "pointer-events-none max-w-[min(92vw,240px)] select-none rounded-lg border border-slate-200/60 bg-white/55 px-2.5 py-2 text-left shadow-[0_8px_24px_rgba(15,23,42,0.10)] backdrop-blur-sm",
        className,
      )}
      style={style}
      data-map-country-tooltip
    >
      <div className="truncate text-[12px] font-semibold text-foreground">
        {countryFlag(board.countryCode)} {title}
      </div>
      <div className="mt-1 truncate text-[11px] text-muted-foreground">
        {t("map.countrySummary", {
          clients: board.clientCount,
          shares: board.shareCount,
          inflight: board.inflightRequests,
        })}
      </div>
    </div>
  );
}
