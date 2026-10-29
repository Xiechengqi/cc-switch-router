import { Suspense } from "react";
import { AccountProviderCenter } from "@/components/dashboard/account-provider-center";

export default function Page() {
  return (
    <Suspense fallback={null}>
      <AccountProviderCenter />
    </Suspense>
  );
}
