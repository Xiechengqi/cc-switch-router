"use client";

import { toast } from "@heroui/react";
import { Copy } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function SubdomainCopyButton({ subdomain }: { subdomain: string }) {
  const { t } = useLocaleText();

  const copySubdomain = async () => {
    try {
      await navigator.clipboard.writeText(subdomain);
      toast.success(t("common.copySuccess"));
    } catch {
      toast.danger(t("common.copyFailed"));
    }
  };

  return (
    <button
      type="button"
      data-no-row-drawer
      aria-label={t("dashboard.copySubdomain")}
      title={t("dashboard.copySubdomain")}
      className="inline-flex h-6 shrink-0 items-center gap-1 rounded-md border border-slate-200 bg-white px-2 text-[10px] font-semibold text-slate-700 transition-colors hover:border-slate-300 hover:bg-slate-50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30"
      onClick={(event) => {
        event.stopPropagation();
        void copySubdomain();
      }}
    >
      <Copy className="h-3 w-3" aria-hidden />
      <span>{t("common.copy")}</span>
    </button>
  );
}
