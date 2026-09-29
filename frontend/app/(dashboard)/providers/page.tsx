import { Suspense } from "react";
import { ProvidersPage } from "@/components/dashboard/providers-page";

export default function Page() {
  return (
    <Suspense fallback={null}>
      <ProvidersPage />
    </Suspense>
  );
}
