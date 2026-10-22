import { Suspense } from "react";
import { AccountLegacyRedirect } from "@/components/dashboard/account-legacy-redirect";

export default function Page() {
  return <Suspense fallback={null}><AccountLegacyRedirect target="/account/provider-settings/" /></Suspense>;
}
