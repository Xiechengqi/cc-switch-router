import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountClientPage } from "@/components/dashboard/account-client-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton inset />}>
      <AccountClientPage />
    </Suspense>
  );
}
