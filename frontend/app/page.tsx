"use client";

import { RootPage } from "@/components/root-page";
import { StartGate } from "@/components/start/start-gate";

export default function Page() {
  return (
    <StartGate>
      <RootPage />
    </StartGate>
  );
}
