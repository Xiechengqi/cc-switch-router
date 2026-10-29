import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { ProvidersPage } from "@/components/dashboard/providers-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton />}>
      <ProvidersPage />
    </Suspense>
  );
}
