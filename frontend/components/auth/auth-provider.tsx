"use client";

import * as React from "react";
import { clearSessionTokens, ensureAuthDeviceIdentity, logoutSession, sessionStatus } from "@/lib/auth";
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
  const bootstrappedRef = React.useRef(false);

  const refresh = React.useCallback(async () => {
    const initial = !bootstrappedRef.current;
    if (initial) setLoading(true);
    try {
      await ensureAuthDeviceIdentity();
      setSession(await sessionStatus());
      bootstrappedRef.current = true;
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    refresh().catch(() => {
      bootstrappedRef.current = true;
      setLoading(false);
    });
    const handler = () => {
      refresh().catch(() => setLoading(false));
    };
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
