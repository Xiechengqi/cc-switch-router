import { Suspense } from "react";
import { AccountClientPage } from "@/components/dashboard/account-client-page";

export default function Page() {
  return (
    <Suspense fallback={null}>
      <AccountClientPage />
    </Suspense>
  );
}
