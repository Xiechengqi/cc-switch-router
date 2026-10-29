import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountSharePage } from "@/components/dashboard/account-share-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton inset />}>
      <AccountSharePage />
    </Suspense>
  );
}
