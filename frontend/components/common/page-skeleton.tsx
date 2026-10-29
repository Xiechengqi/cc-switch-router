"use client";

import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";

/**
 * Placeholder for pages that read the address on the client. A static export
 * prerenders the Suspense fallback, so this is what a full page load paints
 * until hydration; soft navigations never show it.
 */
export function PageSkeleton({ inset = false }: { inset?: boolean }) {
  const { t } = useLocaleText();
  return (
    <div
      className={cn("grid gap-4", inset ? "min-w-0" : "mx-auto w-[calc(100%-2rem)] max-w-7xl pb-6")}
      role="status"
      aria-busy="true"
    >
      <span className="sr-only">{t("common.loading")}</span>
      <div className="h-6 w-48 animate-pulse rounded bg-slate-100 motion-reduce:animate-none" aria-hidden />
      <div className="grid gap-3 rounded-xl border border-slate-200 bg-white p-4" aria-hidden>
        {[0, 1, 2].map((index) => (
          <div key={index} className="grid gap-2 border-t border-slate-100 pt-3 first:border-t-0 first:pt-0">
            <div className="h-4 w-40 animate-pulse rounded bg-slate-100 motion-reduce:animate-none" />
            <div className="h-3 w-full max-w-xl animate-pulse rounded bg-slate-100 motion-reduce:animate-none" />
          </div>
        ))}
      </div>
    </div>
  );
}
