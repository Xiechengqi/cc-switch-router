import { AccountBillingGate } from "@/components/dashboard/account-billing-gate";
import { Suspense } from "react";

export default function Page() {
  return <Suspense fallback={null}><AccountBillingGate /></Suspense>;
}
