"use client";

import * as React from "react";

/**
 * A small stale-while-revalidate store for read models several mounted
 * components want at once (navigation badges, summaries). One entry per key:
 * concurrent readers share a single request and a single poll timer, and the
 * last good value survives soft navigations, so returning to a page renders
 * at once and refreshes in the background.
 *
 * Values live in memory only. Keys must name the account they belong to, so
 * one account never renders another account's data.
 */
export type CachedResourceSnapshot<T> = {
  data: T | undefined;
  error: unknown;
  updatedAt: number;
  loading: boolean;
};

type Entry<T> = {
  snapshot: CachedResourceSnapshot<T>;
  fetcher: () => Promise<T>;
  listeners: Set<() => void>;
  pollers: Map<symbol, number>;
  timer: number | null;
  timerMs: number;
  inFlight: Promise<void> | null;
  generation: number;
};

const EMPTY: CachedResourceSnapshot<never> = Object.freeze({ data: undefined, error: undefined, updatedAt: 0, loading: false });
const entries = new Map<string, Entry<unknown>>();

function entryFor<T>(key: string, fetcher: () => Promise<T>) {
  let entry = entries.get(key) as Entry<T> | undefined;
  if (!entry) {
    entry = {
      snapshot: EMPTY,
      fetcher,
      listeners: new Set(),
      pollers: new Map(),
      timer: null,
      timerMs: 0,
      inFlight: null,
      generation: 0,
    };
    entries.set(key, entry as Entry<unknown>);
  }
  return entry;
}

function publish<T>(entry: Entry<T>, next: Partial<CachedResourceSnapshot<T>>) {
  entry.snapshot = { ...entry.snapshot, ...next };
  entry.listeners.forEach((listener) => listener());
}

export function refreshCachedResource(key: string): Promise<void> {
  const entry = entries.get(key);
  if (!entry) return Promise.resolve();
  if (entry.inFlight) return entry.inFlight;
  const generation = entry.generation;
  publish(entry, { loading: true });
  entry.inFlight = entry.fetcher()
    .then((data) => {
      // A local mutation during the request is newer than this response.
      if (entry.generation !== generation) return;
      publish(entry, { data, error: undefined, updatedAt: Date.now() });
    })
    .catch((error: unknown) => {
      if (entry.generation !== generation) return;
      publish(entry, { error });
    })
    .finally(() => {
      entry.inFlight = null;
      if (entry.snapshot.loading) publish(entry, { loading: false });
    });
  return entry.inFlight;
}

export function mutateCachedResource<T>(key: string, update: (current: T | undefined) => T) {
  const entry = entries.get(key) as Entry<T> | undefined;
  if (!entry) return;
  const data = update(entry.snapshot.data);
  if (Object.is(data, entry.snapshot.data)) return;
  entry.generation += 1;
  publish(entry, { data, error: undefined, updatedAt: Date.now() });
}

let visibilityListening = false;

/** A hidden tab skips its polls; catch up as soon as it is visible again. */
function listenForVisibility() {
  if (visibilityListening || typeof document === "undefined") return;
  visibilityListening = true;
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState !== "visible") return;
    entries.forEach((entry, key) => {
      if (entry.timer !== null) void refreshCachedResource(key);
    });
  });
}

function schedule(key: string, entry: Entry<unknown>) {
  listenForVisibility();
  const intervals = [...entry.pollers.values()];
  const ms = intervals.length && entry.listeners.size ? Math.min(...intervals) : 0;
  if (ms === entry.timerMs) return;
  if (entry.timer !== null) window.clearInterval(entry.timer);
  entry.timer = null;
  entry.timerMs = ms;
  if (!ms) return;
  entry.timer = window.setInterval(() => {
    if (document.visibilityState === "hidden") return;
    void refreshCachedResource(key);
  }, ms);
}

export function useCachedResource<T>(
  key: string | null,
  fetcher: () => Promise<T>,
  { pollMs = 0 }: { pollMs?: number } = {},
) {
  const fetcherRef = React.useRef(fetcher);
  fetcherRef.current = fetcher;

  const subscribe = React.useCallback((listener: () => void) => {
    if (!key) return () => undefined;
    const entry = entryFor(key, () => fetcherRef.current());
    entry.listeners.add(listener);
    return () => {
      entry.listeners.delete(listener);
      schedule(key, entry as Entry<unknown>);
    };
  }, [key]);
  const getSnapshot = React.useCallback(
    () => (key ? (entries.get(key)?.snapshot as CachedResourceSnapshot<T> | undefined) ?? EMPTY : EMPTY),
    [key],
  );
  const snapshot = React.useSyncExternalStore(subscribe, getSnapshot, () => EMPTY);

  React.useEffect(() => {
    if (!key) return;
    const entry = entryFor(key, () => fetcherRef.current());
    const poller = Symbol(key);
    if (pollMs > 0) entry.pollers.set(poller, pollMs);
    schedule(key, entry as Entry<unknown>);
    // Revalidate on every mount; concurrent mounts share the request.
    void refreshCachedResource(key);
    return () => {
      entry.pollers.delete(poller);
      schedule(key, entry as Entry<unknown>);
    };
  }, [key, pollMs]);

  const refresh = React.useCallback(() => (key ? refreshCachedResource(key) : Promise.resolve()), [key]);
  const mutate = React.useCallback(
    (update: (current: T | undefined) => T) => { if (key) mutateCachedResource(key, update); },
    [key],
  );
  return { ...snapshot, loading: snapshot.loading && snapshot.data === undefined, refreshing: snapshot.loading, refresh, mutate };
}

const viewSnapshots = new Map<string, unknown>();

/**
 * Last good state of a page that drives its own loading (pagination, merges,
 * aborts) and so does not fit `useCachedResource`. A page seeds its state from
 * it to render at once on return, then refreshes silently. Keys must name the
 * account, like resource keys.
 */
export function rememberedView<T>(key: string | null): T | undefined {
  return key ? (viewSnapshots.get(key) as T | undefined) : undefined;
}

export function rememberView<T>(key: string | null, value: T) {
  if (key) viewSnapshots.set(key, value);
}

/**
 * Seed for a page that keeps its own state: `remembered` is read once, at
 * mount, and `takeSeed()` returns true exactly once when the page started
 * from it, so the first load can refresh without a spinner.
 */
export function useRememberedView<T>(key: string | null) {
  const [remembered] = React.useState(() => rememberedView<T>(key));
  const seededRef = React.useRef(remembered !== undefined);
  const takeSeed = React.useCallback(() => {
    const seeded = seededRef.current;
    seededRef.current = false;
    return seeded;
  }, []);
  return { remembered, takeSeed };
}
