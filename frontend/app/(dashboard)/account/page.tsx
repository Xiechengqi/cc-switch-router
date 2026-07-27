"use client";

import { useRouter } from "next/navigation";
import * as React from "react";
import { resolveAccountEntryHref } from "@/lib/dashboard-nav";

/** Restore last account sidebar section from localStorage (static-export safe). */
export default function Page() {
  const router = useRouter();

  React.useEffect(() => {
    router.replace(resolveAccountEntryHref());
  }, [router]);

  return null;
}
