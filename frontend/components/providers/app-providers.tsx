"use client";

import * as React from "react";
import { AuthProvider } from "@/components/auth/auth-provider";
import { LocaleProvider } from "@/components/i18n/locale-provider";
import { UiUpdateNotice } from "@/components/layout/ui-update-notice";

/** Root client providers so dashboard tab soft-nav keeps auth/locale mounted. */
export function AppProviders({ children }: { children: React.ReactNode }) {
  return (
    <LocaleProvider>
      <AuthProvider>
        {children}
        <UiUpdateNotice />
      </AuthProvider>
    </LocaleProvider>
  );
}
