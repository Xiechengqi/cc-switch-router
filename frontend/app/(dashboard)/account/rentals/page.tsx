import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountRentalsPage } from "@/components/dashboard/account-rentals-page";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton inset />}>
      <AccountRentalsPage />
    </Suspense>
  );
}
