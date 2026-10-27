"use client";

import * as React from "react";
import {
  clearSessionTokens,
  ensureAuthDeviceIdentity,
  logoutSession,
  readAuthState,
  sessionStatus,
} from "@/lib/auth";
import type { AuthResult, SessionStatus } from "@/lib/types";
import { sameLoginContext, shouldPromptForPassword } from "@/lib/password-auth";
import { PasswordDialog } from "./password-dialog";

type AuthContextValue = {
  session: SessionStatus | null;
  loading: boolean;
  refresh: () => Promise<void>;
  logout: () => Promise<void>;
  openPasswordSettings: () => void;
  emailLoginCompleted: (result: AuthResult, resetIntent?: boolean) => void;
  passwordSettingsOpen: boolean;
};

const AuthContext = React.createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: React.ReactNode }) {
  const [session, setSession] = React.useState<SessionStatus | null>(null);
  const [loading, setLoading] = React.useState(true);
  const bootstrappedRef = React.useRef(false);
  const [passwordTarget, setPasswordTarget] = React.useState<string | null>(
    null,
  );
  const promptedLogin = React.useRef<string | null>(null);
  const refreshGeneration = React.useRef(0);

  const refresh = React.useCallback(async () => {
    const generation = ++refreshGeneration.current;
    const initial = !bootstrappedRef.current;
    if (initial) setLoading(true);
    try {
      await ensureAuthDeviceIdentity();
      const next = await sessionStatus();
      if (generation !== refreshGeneration.current) return;
      setSession(next);
      setPasswordTarget((target) =>
        next.authenticated && next.user?.id === target ? target : null,
      );
      bootstrappedRef.current = true;
    } finally {
      if (generation === refreshGeneration.current) setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void refresh().catch(() => undefined);
    const handler = () => {
      void refresh().catch(() => undefined);
    };
    window.addEventListener("router-auth-changed", handler);
    const storageHandler = (event: StorageEvent) => {
      if (event.key === "cc_switch_router_auth_v2") handler();
    };
    window.addEventListener("storage", storageHandler);
    return () => {
      refreshGeneration.current++;
      window.removeEventListener("router-auth-changed", handler);
      window.removeEventListener("storage", storageHandler);
    };
  }, [refresh]);

  const logout = React.useCallback(async () => {
    const started = readAuthState();
    refreshGeneration.current++;
    setPasswordTarget(null);
    await logoutSession();
    if (sameLoginContext(started, readAuthState())) {
      clearSessionTokens();
      await refresh();
    }
  }, [refresh]);

  const emailLoginCompleted = React.useCallback(
    (result: AuthResult, resetIntent = false) => {
      const event = result.password?.loginEventId;
      if (
        !event ||
        readAuthState().email !== result.user.email ||
        promptedLogin.current === event ||
        !shouldPromptForPassword(result, resetIntent)
      )
        return;
      promptedLogin.current = event;
      setPasswordTarget(result.user.id);
    },
    [],
  );
  const openPasswordSettings = React.useCallback(() => {
    if (session?.authenticated && session.password)
      setPasswordTarget(session.user?.id || null);
  }, [session]);
  const passwordSettingsOpen =
    !!passwordTarget &&
    session?.user?.id === passwordTarget &&
    !!session.authenticated;
  return (
    <AuthContext.Provider
      value={{
        session,
        loading,
        refresh,
        logout,
        openPasswordSettings,
        emailLoginCompleted,
        passwordSettingsOpen,
      }}
    >
      {children}
      <PasswordDialog
        open={passwordSettingsOpen}
        onOpenChange={(open) => {
          if (!open) setPasswordTarget(null);
        }}
      />
    </AuthContext.Provider>
  );
}

export function useAuth() {
  const value = React.useContext(AuthContext);
  if (!value) throw new Error("useAuth must be used within AuthProvider");
  return value;
}
