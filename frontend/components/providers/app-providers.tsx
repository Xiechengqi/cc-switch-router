"use client";

import * as React from "react";
import { AuthProvider } from "@/components/auth/auth-provider";
import { LocaleProvider } from "@/components/i18n/locale-provider";

/** Root client providers so dashboard tab soft-nav keeps auth/locale mounted. */
export function AppProviders({ children }: { children: React.ReactNode }) {
  return (
    <LocaleProvider>
      <AuthProvider>{children}</AuthProvider>
    </LocaleProvider>
  );
}
