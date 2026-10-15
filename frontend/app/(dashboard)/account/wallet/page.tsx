import { Suspense } from "react";
import { AccountWalletPage } from "@/components/dashboard/account-wallet-page";

export default function WalletPage() {
  return <Suspense fallback={null}><AccountWalletPage /></Suspense>;
}
