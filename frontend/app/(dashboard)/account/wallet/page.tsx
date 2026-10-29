import { Suspense } from "react";
import { PageSkeleton } from "@/components/common/page-skeleton";
import { AccountWalletPage } from "@/components/dashboard/account-wallet-page";

export default function WalletPage() {
  return <Suspense fallback={<PageSkeleton inset />}><AccountWalletPage /></Suspense>;
}
