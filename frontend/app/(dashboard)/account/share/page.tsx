import { Suspense } from "react";
import { AccountSharePage } from "@/components/dashboard/account-share-page";

export default function Page() {
  return (
    <Suspense fallback={null}>
      <AccountSharePage />
    </Suspense>
  );
}
