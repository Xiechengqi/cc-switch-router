"use client";

import * as React from "react";
import { useRouter } from "next/navigation";
import { legacyClientsHref } from "@/lib/home-entry";

export default function Page() {
  const router = useRouter();
  React.useEffect(() => {
    const destination = legacyClientsHref(window.location.pathname, window.location.search, window.location.hash);
    if (destination) router.replace(destination);
  }, [router]);
  return null;
}
