"use client";
import { useEffect } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function AccountLegacyRedirect({
  target,
  billing = false,
}: {
  target: string;
  billing?: boolean;
}) {
  const router = useRouter();
  const params = useSearchParams();
  const { t } = useLocaleText();
  useEffect(() => {
    // Old Provider receivables bookmarks go to earnings, not buyer balance.
    const destination =
      billing && params.get("tab") === "receivables"
        ? "/account/earnings/"
        : target;
    router.replace(destination);
  }, [billing, params, router, target]);
  return (
    <p role="status" className="p-5 text-sm text-muted-foreground">
      {t("common.loading")}
    </p>
  );
}
