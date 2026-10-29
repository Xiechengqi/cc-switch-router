import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountProviderCenter } from "@/components/dashboard/account-provider-center";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton inset />}>
      <AccountProviderCenter />
    </Suspense>
  );
}
