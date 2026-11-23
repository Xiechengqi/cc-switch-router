"use client";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { OPERATION_LABELS, type ShareOperation } from "@/lib/share-capabilities";

export function ShareCapabilitySummary({ operations }: { operations?: ShareOperation[] }) {
  const { locale, t } = useLocaleText();
  if (!operations?.length) return null;
  const zh = locale.startsWith("zh");
  return <span className="text-[11px] text-muted-foreground">
    {t("dashboard.capabilitySummary.title")} ·{" "}
    {operations.map(operation => OPERATION_LABELS[operation][zh ? 0 : 1]).join(t("dashboard.capabilitySummary.separator"))}
  </span>;
}
