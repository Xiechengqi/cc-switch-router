"use client";

import * as React from "react";

export function usePersistentState<T>(key: string, initialValue: T) {
  const [value, setValue] = React.useState<T>(initialValue);
  const [hydrated, setHydrated] = React.useState(false);

  React.useEffect(() => {
    try {
      const stored = window.localStorage.getItem(key);
      if (stored != null) setValue(JSON.parse(stored) as T);
    } catch {
      // Invalid or inaccessible local preferences should never block the dashboard.
    } finally {
      setHydrated(true);
    }
  }, [key]);

  React.useEffect(() => {
    if (!hydrated) return;
    try {
      window.localStorage.setItem(key, JSON.stringify(value));
    } catch {
      // Storage can be unavailable in private browsing or locked-down contexts.
    }
  }, [hydrated, key, value]);

  return [value, setValue] as const;
}
