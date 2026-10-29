import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { ClientMarketPage } from "@/components/dashboard/client-market-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton />}>
      <ClientMarketPage />
    </Suspense>
  );
}
