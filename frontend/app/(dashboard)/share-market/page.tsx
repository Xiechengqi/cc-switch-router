import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { ShareMarketPage } from "@/components/dashboard/share-market-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton />}>
      <ShareMarketPage />
    </Suspense>
  );
}
