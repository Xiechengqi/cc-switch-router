"use client";

import * as React from "react";
import { getUserApiToken } from "./api";
import { readAuthState, type AuthState } from "./auth";
import { sameLoginContext } from "./password-auth";
import type { UserApiTokenResponse } from "./types";

let legacyLogin: AuthState | null = null;
let legacyRevision = 0;

export function shareConnectionActor(auth: AuthState = readAuthState()): string {
  if (!auth.accessToken || !auth.email) {
    legacyLogin = null;
    return "";
  }
  let epoch = auth.loginEpoch;
  if (!epoch) {
    // Older sessions have no login epoch. Conservatively invalidate on token
    // changes, like sameLoginContext, without putting raw tokens in React keys.
    if (!legacyLogin || legacyLogin.email !== auth.email || !sameLoginContext(legacyLogin, auth)) {
      legacyLogin = { email: auth.email, accessToken: auth.accessToken, refreshToken: auth.refreshToken };
      legacyRevision++;
    }
    epoch = `legacy:${legacyRevision}`;
  }
  return JSON.stringify([auth.email, epoch, auth.authDeviceId ?? ""]);
}

function subscribe(change: () => void) {
  window.addEventListener("router-auth-changed", change);
  window.addEventListener("storage", change);
  return () => {
    window.removeEventListener("router-auth-changed", change);
    window.removeEventListener("storage", change);
  };
}

type Snapshot = { identity: string; value?: UserApiTokenResponse; error?: boolean };

export function useShareConnectionToken(open: boolean, userId?: string, userEmail?: string) {
  const actor = React.useSyncExternalStore(subscribe, shareConnectionActor, () => "");
  const identity = JSON.stringify([userId ?? "", actor]);
  const matchesActor = !!actor && !!userId && !!userEmail && readAuthState().email === userEmail;
  const enabled = open && matchesActor;
  const [snapshot, setSnapshot] = React.useState<Snapshot | null>(null);
  const [attempt, retry] = React.useReducer((value: number) => value + 1, 0);

  React.useEffect(() => {
    setSnapshot(null);
    if (!enabled) return;
    const controller = new AbortController();
    void getUserApiToken(controller.signal).then(value => {
      if (!controller.signal.aborted && actor === shareConnectionActor()) setSnapshot({ identity, value });
    }).catch(() => {
      if (!controller.signal.aborted && actor === shareConnectionActor()) setSnapshot({ identity, error: true });
    });
    return () => controller.abort();
  }, [enabled, identity, actor, attempt]);

  const current = enabled && actor === shareConnectionActor() && snapshot?.identity === identity ? snapshot : null;
  return {
    actor: identity,
    value: current?.value,
    error: !!current?.error,
    loading: enabled && !current,
    matchesActor,
    isCurrent: () => enabled && actor === shareConnectionActor(),
    retry,
  };
}
