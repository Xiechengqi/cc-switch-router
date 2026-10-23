"use client";
import { useEffect } from "react";
import { useRouter } from "next/navigation";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function AccountLegacyRedirect({
  target,
}: {
  target: string;
}) {
  const router = useRouter();
  const { t } = useLocaleText();
  useEffect(() => {
    router.replace(target);
  }, [router, target]);
  return (
    <p role="status" className="p-5 text-sm text-muted-foreground">
      {t("common.loading")}
    </p>
  );
}
