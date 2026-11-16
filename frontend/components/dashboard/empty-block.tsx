import type { ReactNode } from "react";

export function EmptyBlock({ children }: { children: ReactNode }) {
  return (
    <div className="rounded-lg border bg-muted/20 p-4 text-sm text-muted-foreground">
      {children}
    </div>
  );
}
