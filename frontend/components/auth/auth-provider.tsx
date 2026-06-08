"use client";

import * as React from "react";
import { clearSessionTokens, ensureInstallationIdentity, logoutSession, sessionStatus } from "@/lib/auth";
import type { SessionStatus } from "@/lib/types";

type AuthContextValue = {
  session: SessionStatus | null;
  loading: boolean;
  refresh: () => Promise<void>;
  logout: () => Promise<void>;
};

const AuthContext = React.createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [session, setSession] = React.useState<SessionStatus | null>(null);
  const [loading, setLoading] = React.useState(true);

  const refresh = React.useCallback(async () => {
    setLoading(true);
    try {
      await ensureInstallationIdentity();
      setSession(await sessionStatus());
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    refresh().catch(() => setLoading(false));
    const handler = () => refresh().catch(() => setLoading(false));
    window.addEventListener("router-auth-changed", handler);
    return () => window.removeEventListener("router-auth-changed", handler);
  }, [refresh]);

  const logout = React.useCallback(async () => {
    await logoutSession();
    clearSessionTokens();
    setSession(await sessionStatus());
  }, []);

  return <AuthContext.Provider value={{ session, loading, refresh, logout }}>{children}</AuthContext.Provider>;
}

export function useAuth() {
  const value = React.useContext(AuthContext);
  if (!value) throw new Error("useAuth must be used within AuthProvider");
  return value;
}
