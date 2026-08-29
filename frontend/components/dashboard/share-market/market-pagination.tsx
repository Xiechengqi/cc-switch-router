"use client";

import { Button } from "@heroui/react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function MarketPagination({
  page,
  pageCount,
  onPageChange,
  hasMore = false,
}: {
  page: number;
  pageCount: number;
  onPageChange: (page: number) => void;
  hasMore?: boolean;
}) {
  const { t } = useLocaleText();
  if (pageCount <= 1 && !hasMore) return null;
  return (
    <div className="flex items-center justify-center gap-2">
      <Button
        variant="outline"
        className="h-9"
        isDisabled={page <= 1}
        onClick={() => onPageChange(page - 1)}
        aria-label={t("shareMarket.catalog.previousPage")}
      >
        <ChevronLeft className="h-4 w-4" />
        {t("shareMarket.catalog.previousPage")}
      </Button>
      <span className="text-xs tabular-nums text-slate-500">{t("shareMarket.catalog.page", { page, pages: Math.max(pageCount, page) })}</span>
      <Button
        variant="outline"
        className="h-9"
        isDisabled={page >= pageCount && !hasMore}
        onClick={() => onPageChange(page + 1)}
        aria-label={t("shareMarket.catalog.nextPage")}
      >
        {t("shareMarket.catalog.nextPage")}
        <ChevronRight className="h-4 w-4" />
      </Button>
    </div>
  );
}
