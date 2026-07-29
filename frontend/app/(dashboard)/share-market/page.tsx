import { Suspense } from "react";
import { ShareMarketPage } from "@/components/dashboard/share-market-page";

export default function Page() {
  return (
    <Suspense fallback={null}>
      <ShareMarketPage />
    </Suspense>
  );
}
