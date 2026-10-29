import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountNotificationsPanel } from "@/components/dashboard/account-notifications-panel";

export default function Page() {
  return (
    <Suspense fallback={<PageSkeleton inset />}>
      <AccountNotificationsPanel />
    </Suspense>
  );
}
