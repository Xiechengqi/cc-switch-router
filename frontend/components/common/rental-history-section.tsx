import type { ReactNode } from "react";

/**
 * The one "租用历史" block of both markets: the Share buyer's history (the
 * reference), the Client buyer's history, and each market's seller history.
 * A quiet `text-sm` heading with the total, then an ExpandableRecordTable and
 * its pagination as children.
 */
export function RentalHistorySection({
  id,
  title,
  count,
  children,
  testId,
}: {
  id: string;
  title: ReactNode;
  /** Omitted while the first page is still loading. */
  count?: number | null;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <section className="grid min-w-0 gap-2" aria-labelledby={id} data-testid={testId}>
      <h4 id={id} className="text-sm font-semibold text-muted-foreground">
        {title}
        {count != null ? <span className="tabular-nums"> · {count}</span> : null}
      </h4>
      {children}
    </section>
  );
}
