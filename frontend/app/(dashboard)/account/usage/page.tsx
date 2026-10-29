import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountUsagePage } from "@/components/dashboard/account-usage-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton inset />}>
      <AccountUsagePage />
    </Suspense>
  );
}
