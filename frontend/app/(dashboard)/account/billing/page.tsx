import { AccountLegacyRedirect } from "@/components/dashboard/account-legacy-redirect";
import { Suspense } from "react";

export default function Page() {
  return <Suspense fallback={null}><AccountLegacyRedirect target="/account/wallet/" /></Suspense>;
}
