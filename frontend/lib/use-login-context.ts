"use client";

import { useSyncExternalStore } from "react";
import { readAuthState } from "./auth";
import { loginContextKey } from "./password-auth";

const anonymous = loginContextKey({});
const snapshot = () => loginContextKey(readAuthState());

function subscribe(changed: () => void) {
  const storageChanged = (event: StorageEvent) => {
    if (event.key === null || event.key === "cc_switch_router_auth_v2") changed();
  };
  window.addEventListener("router-auth-changed", changed);
  window.addEventListener("storage", storageChanged);
  return () => {
    window.removeEventListener("router-auth-changed", changed);
    window.removeEventListener("storage", storageChanged);
  };
}

/** Replaces private read models synchronously when the sign-in context changes. */
export function useLoginContextKey() {
  return useSyncExternalStore(subscribe, snapshot, () => anonymous);
}
