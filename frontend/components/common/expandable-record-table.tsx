"use client";

/* Hallmark · component: expandable record table · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · expanded · attention · danger · empty
 * Core facts stay on one row; everything else is rendered only when a row is
 * opened, so per-row detail requests never run for collapsed records.
 */

import * as React from "react";
import { Button } from "@heroui/react";
import { ChevronDown } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

export type RecordTableColumn<T> = {
  id: string;
  header: React.ReactNode;
  cell: (row: T) => React.ReactNode;
  /** Secondary columns hide on narrow screens; repeat their facts in the details. */
  minWidth?: "sm" | "md" | "lg";
  /** Width and alignment classes shared by the header and cells. */
  className?: string;
};

export type RecordTone = "attention" | "danger";

const HIDDEN_BELOW: Record<NonNullable<RecordTableColumn<unknown>["minWidth"]>, string> = {
  sm: "hidden sm:table-cell",
  md: "hidden md:table-cell",
  lg: "hidden lg:table-cell",
};

const INTERACTIVE = "a, button, input, select, textarea, label, [role='button'], [data-row-action]";

export function ExpandableRecordTable<T>({
  rows,
  rowKey,
  rowLabel,
  columns,
  renderDetail,
  rowTone,
  ariaLabel,
  empty,
  focusKey,
}: {
  rows: readonly T[];
  rowKey: (row: T) => string;
  /** Names the row for the expand button, e.g. a Share or Client name. */
  rowLabel: (row: T) => string;
  columns: RecordTableColumn<T>[];
  renderDetail: (row: T) => React.ReactNode;
  rowTone?: (row: T) => RecordTone | undefined;
  ariaLabel: string;
  empty?: React.ReactNode;
  /** Opens and scrolls to this row once it appears, for deep links. */
  focusKey?: string | null;
}) {
  const { t } = useLocaleText();
  const [expanded, setExpanded] = React.useState<ReadonlySet<string>>(() => new Set(focusKey ? [focusKey] : []));
  const focusedRef = React.useRef<string | null>(null);
  const rootRef = React.useRef<HTMLDivElement>(null);
  const tableId = React.useId();

  const toggle = React.useCallback((key: string) => {
    setExpanded((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  React.useEffect(() => {
    if (!focusKey || focusedRef.current === focusKey) return;
    if (!rows.some((row) => rowKey(row) === focusKey)) return;
    focusedRef.current = focusKey;
    setExpanded((current) => current.has(focusKey) ? current : new Set(current).add(focusKey));
    const target = rootRef.current?.querySelector<HTMLElement>(`[data-record-key="${CSS.escape(focusKey)}"]`);
    target?.scrollIntoView({ block: "center" });
    target?.querySelector<HTMLButtonElement>("[data-record-expand]")?.focus({ preventScroll: true });
  }, [focusKey, rowKey, rows]);

  if (!rows.length) return <>{empty}</>;

  return (
    <div ref={rootRef} className="min-w-0 overflow-hidden rounded-lg border border-border bg-card">
      <table className="w-full table-fixed text-left text-sm" aria-label={ariaLabel}>
        <thead className="bg-muted/60 text-xs text-muted-foreground">
          <tr>
            {columns.map((column) => (
              <th key={column.id} scope="col" className={cn("px-2 py-2 font-medium sm:px-3", column.minWidth && HIDDEN_BELOW[column.minWidth], column.className)}>
                {column.header}
              </th>
            ))}
            <th scope="col" className="w-11"><span className="sr-only">{t("recordTable.details")}</span></th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row, index) => {
            const key = rowKey(row);
            const open = expanded.has(key);
            const tone = rowTone?.(row);
            const detailId = `${tableId}-detail-${index}`;
            const label = rowLabel(row);
            return (
              <React.Fragment key={key}>
                <tr
                  data-record-key={key}
                  className={cn(
                    "cursor-pointer border-t border-border align-middle transition-colors hover:bg-muted/40 motion-reduce:transition-none",
                    open && "bg-muted/40",
                  )}
                  onClick={(event) => {
                    if ((event.target as HTMLElement).closest(INTERACTIVE) || window.getSelection()?.toString()) return;
                    toggle(key);
                  }}
                >
                  {columns.map((column, columnIndex) => (
                    <td
                      key={column.id}
                      className={cn(
                        "min-w-0 px-2 py-2.5 sm:px-3",
                        columnIndex === 0 && (tone === "danger" ? "text-destructive" : tone === "attention" ? "text-amber-800" : undefined),
                        column.minWidth && HIDDEN_BELOW[column.minWidth],
                        column.className,
                      )}
                    >
                      {column.cell(row)}
                    </td>
                  ))}
                  <td className="px-1 py-1.5 text-right">
                    <Button
                      size="sm" variant="ghost" isIconOnly className="min-h-11 min-w-11"
                      data-record-expand
                      aria-label={t(open ? "recordTable.collapse" : "recordTable.expand", { name: label })}
                      aria-expanded={open} aria-controls={detailId}
                      onPress={() => toggle(key)}
                    >
                      <ChevronDown aria-hidden className={cn("h-4 w-4 transition-transform motion-reduce:transition-none", open && "rotate-180")} />
                    </Button>
                  </td>
                </tr>
                {open ? (
                  <tr id={detailId} className="border-t border-border/60 bg-muted/20">
                    <td colSpan={columns.length + 1} className="px-3 py-3 sm:px-4">
                      {renderDetail(row)}
                    </td>
                  </tr>
                ) : null}
              </React.Fragment>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

/** A label/value grid for expanded rows. */
export function RecordDetailList({ items }: { items: { label: React.ReactNode; value: React.ReactNode }[] }) {
  return (
    <dl className="grid min-w-0 gap-x-6 gap-y-2 text-sm sm:grid-cols-2 lg:grid-cols-3">
      {items.map((item, index) => (
        <div key={index} className="grid min-w-0 gap-0.5">
          <dt className="text-xs text-muted-foreground">{item.label}</dt>
          <dd className="min-w-0 break-words">{item.value}</dd>
        </div>
      ))}
    </dl>
  );
}
