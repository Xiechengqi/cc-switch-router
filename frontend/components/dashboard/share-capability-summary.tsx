"use client";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { OPERATION_LABELS, type ShareOperation } from "@/lib/share-capabilities";

export function ShareCapabilitySummary({ operations }: { operations?: ShareOperation[] }) {
  const { locale } = useLocaleText();
  if (!operations?.length) return null;
  const zh = locale.startsWith("zh");
  return <span className="text-[10px] text-muted-foreground">
    {zh ? "我的媒体与工具权限" : "My media and tool permissions"} ·{" "}
    {operations.map(operation => OPERATION_LABELS[operation][zh ? 0 : 1]).join(zh ? "、" : ", ")}
  </span>;
}
