import { AccountBillingPage } from "@/components/dashboard/account-billing-page";
import { Suspense } from "react";

export default function Page() {
  return <Suspense fallback={null}><AccountBillingPage /></Suspense>;
}
