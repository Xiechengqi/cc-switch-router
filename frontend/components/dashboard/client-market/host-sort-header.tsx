"use client";

import * as React from "react";
import { ArrowDown, ArrowUp } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { HOST_SORT_COLUMN_LABELS, HostSortKey, HostSortPrefs } from "@/components/dashboard/client-market/host-utils";

export function HostSortHeader({
  columnKey,
  sortPrefs,
  onSort,
  filter,
}: {
  columnKey: HostSortKey;
  sortPrefs: HostSortPrefs;
  onSort: (key: HostSortKey) => void;
  filter?: React.ReactNode;
}) {
  const { t } = useLocaleText();
  const active = sortPrefs.key === columnKey;
  const label = t(HOST_SORT_COLUMN_LABELS[columnKey]);
  const ariaSort = active ? (sortPrefs.dir === "asc" ? "ascending" : "descending") : "none";
  const sortStateLabel = active
    ? t(sortPrefs.dir === "asc" ? "clientMarket.sortAsc" : "clientMarket.sortDesc")
    : undefined;

  const sortIcons = active ? (
    sortPrefs.dir === "asc" ? (
      <ArrowUp className="h-3.5 w-3.5 text-accent" aria-hidden />
    ) : (
      <ArrowDown className="h-3.5 w-3.5 text-accent" aria-hidden />
    )
  ) : (
    <span className="inline-flex h-3.5 w-3.5 flex-col justify-center opacity-30" aria-hidden>
      <ArrowUp className="h-2.5 w-2.5 -mb-0.5" />
      <ArrowDown className="h-2.5 w-2.5" />
    </span>
  );

  return (
    <th
      scope="col"
      aria-sort={ariaSort}
      className="sticky top-0 z-10 border-b border-border bg-card px-2 py-2 text-left text-xs font-medium text-muted-foreground"
    >
      {filter ? (
        <div className="flex min-w-0 items-center gap-0.5">
          <div className="min-w-0 flex-1">{filter}</div>
          <button
            type="button"
            className="inline-flex shrink-0 items-center justify-center rounded-md p-0.5 transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
            onClick={() => onSort(columnKey)}
            aria-label={t("clientMarket.sortBy", { column: label })}
          >
            {sortIcons}
            {sortStateLabel ? <span className="sr-only">{sortStateLabel}</span> : null}
          </button>
        </div>
      ) : (
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-md px-1 py-0.5 text-left transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
          onClick={() => onSort(columnKey)}
          aria-label={t("clientMarket.sortBy", { column: label })}
        >
          <span className="whitespace-nowrap">{label}</span>
          {sortIcons}
          {sortStateLabel ? <span className="sr-only">{sortStateLabel}</span> : null}
        </button>
      )}
    </th>
  );
}
