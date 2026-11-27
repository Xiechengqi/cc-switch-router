"use client";

import * as React from "react";
import { ApiError, cancelRouterUpgrade, getRouterUpgradeStatus, startUpgrade } from "@/lib/api";
import { readAuthState } from "@/lib/auth";
import { loginContextKey, sameLoginContext } from "@/lib/password-auth";
import {
  isRouterUpgradeTerminal, mergeRouterUpgradeLogs, mergeRouterUpgradeTask, routerUpgradeDismissedKey,
} from "@/lib/router-upgrade";
import type { RouterUpgradeLog, RouterUpgradeTask } from "@/lib/types";

type UpgradeView = {
  scope: string;
  task: RouterUpgradeTask | null;
  discovering: boolean;
  unavailable: boolean;
  creating: boolean;
  cancelling: boolean;
  reconnecting: boolean;
  actionError: string;
  statusError: string;
  dismissedId: string;
};

type UpgradeCommands = { start: () => void; cancel: () => void; retry: () => void; dismiss: () => void };

function initialView(scope: string): UpgradeView {
  return { scope, task: null, discovering: true, unavailable: false, creating: false,
    cancelling: false, reconnecting: false, actionError: "", statusError: "", dismissedId: "" };
}

/** The server owns the operation. Unmounting aborts only HTTP/subscriptions,
 * never the upgrade, and every late callback is fenced to its login context. */
export function useRouterUpgrade(enabled: boolean, accountId: string, accountEmail: string) {
  const [, updateAuthScope] = React.useReducer((value: number) => value + 1, 0);
  const auth = readAuthState();
  const authorized = enabled && !!accountId && !!accountEmail
    && auth.email?.trim().toLowerCase() === accountEmail.trim().toLowerCase();
  // A session without a login epoch is fenced on its tokens (`sameLoginContext`),
  // so its rotation must restart the subscription with a fresh context, or
  // every later callback is dropped as stale.
  const scope = `${accountId}:${loginContextKey(auth)}${auth.loginEpoch ? "" : `:${auth.accessToken || ""}:${auth.refreshToken || ""}`}`;
  const owner = `${accountId}:${accountEmail.trim().toLowerCase()}`;
  const [view, setView] = React.useState<UpgradeView>(() => initialView(scope));
  const commands = React.useRef<UpgradeCommands | null>(null);
  // The account's last task, kept across a token-rotation restart so the
  // panel does not blink away while the new context rediscovers it.
  const carried = React.useRef<{ owner: string; task: RouterUpgradeTask | null }>({ owner: "", task: null });

  React.useEffect(() => {
    const changed = () => updateAuthScope();
    const stored = (event: StorageEvent) => { if (event.key === "cc_switch_router_auth_v2" || event.key === null) changed(); };
    window.addEventListener("router-auth-changed", changed);
    window.addEventListener("storage", stored);
    return () => {
      window.removeEventListener("router-auth-changed", changed);
      window.removeEventListener("storage", stored);
    };
  }, []);

  React.useEffect(() => {
    if (!authorized) {
      commands.current = null;
      setView({ ...initialView(scope), discovering: false });
      return;
    }
    const context = readAuthState();
    const storageKey = routerUpgradeDismissedKey(accountId);
    let dismissedId = "";
    try { dismissedId = localStorage.getItem(storageKey) || ""; } catch { /* Storage is optional. */ }
    let latest: RouterUpgradeTask | null = carried.current.owner === owner ? carried.current.task : null;
    setView({ ...initialView(scope), dismissedId, task: latest });
    let stopped = false;
    let polling = false;
    let actionPending = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    let source: EventSource | null = null;
    let sourceTaskId = "";
    const requests = new Set<AbortController>();
    const current = () => !stopped && sameLoginContext(context, readAuthState());
    const patch = (change: Partial<UpgradeView>) => {
      if (!current()) return;
      if ("task" in change) carried.current = { owner, task: change.task ?? null };
      setView((value) => value.scope === scope ? { ...value, ...change } : value);
    };
    const adopt = (task: RouterUpgradeTask) => {
      if (!current()) return;
      latest = mergeRouterUpgradeTask(latest, task);
      patch({ task: latest });
    };
    const closeSource = () => { source?.close(); source = null; sourceTaskId = ""; };

    function schedule(delay: number) {
      clearTimeout(timer);
      if (current()) timer = setTimeout(() => void poll(), delay);
    }

    function connect(task: RouterUpgradeTask) {
      if (isRouterUpgradeTerminal(task.status)) {
        closeSource();
        patch({ reconnecting: false });
        return;
      }
      if (task.status === "restarting" && task.phase === "restarting") {
        closeSource();
        patch({ reconnecting: true });
        return;
      }
      if (source && sourceTaskId === task.taskId) return;
      closeSource();
      const params = new URLSearchParams({ taskId: task.taskId,
        afterSeq: String(task.logs.at(-1)?.seq || 0) });
      const token = readAuthState().accessToken;
      if (token) params.set("accessToken", token);
      const connection = new EventSource(`/v1/admin/upgrade/stream?${params}`);
      source = connection;
      sourceTaskId = task.taskId;
      patch({ reconnecting: true });
      const valid = () => current() && source === connection;
      connection.onopen = () => { if (valid()) patch({ reconnecting: false }); };
      connection.addEventListener("log", (event) => {
        if (!valid() || !latest) return;
        try {
          const entry = JSON.parse((event as MessageEvent).data) as RouterUpgradeLog;
          if (entry.taskId !== latest.taskId) return;
          latest = { ...latest, logs: mergeRouterUpgradeLogs(latest.logs, [entry], latest.taskId) };
          patch({ task: latest });
        } catch { schedule(0); }
      });
      connection.addEventListener("state", (event) => {
        if (!valid()) return;
        try {
          const task = JSON.parse((event as MessageEvent).data) as RouterUpgradeTask;
          if (task.taskId !== sourceTaskId) return;
          adopt(task);
          patch({ unavailable: false, discovering: false, reconnecting: false });
          if (isRouterUpgradeTerminal(task.status)) closeSource();
          else if (task.status === "restarting" && task.phase === "restarting") {
            closeSource();
            patch({ reconnecting: true });
            schedule(1000);
          }
        } catch { schedule(0); }
      });
      connection.addEventListener("done", () => {
        if (!valid()) return;
        closeSource();
        schedule(0);
      });
      connection.onerror = () => {
        if (!valid()) return;
        closeSource();
        patch({ reconnecting: true });
        schedule(1000);
      };
    }

    async function request<T>(run: (signal: AbortSignal) => Promise<T>, timeout = 10_000) {
      const controller = new AbortController();
      requests.add(controller);
      const deadline = setTimeout(() => controller.abort(), timeout);
      try { return await run(controller.signal); }
      finally { clearTimeout(deadline); requests.delete(controller); }
    }

    async function poll() {
      if (!current() || polling) return;
      polling = true;
      const taskAtRequest = latest?.taskId;
      try {
        const { task } = await request((signal) => getRouterUpgradeStatus(signal, context));
        if (!current()) return;
        if (!task && latest && !isRouterUpgradeTerminal(latest.status)) {
          if (latest.taskId !== taskAtRequest) return;
          closeSource();
          patch({ discovering: false, unavailable: true, reconnecting: true });
          return;
        }
        if (task) {
          adopt(task);
          connect(latest!);
        }
        patch({ discovering: false, unavailable: false, statusError: "" });
      } catch (error) {
        if (!current()) return;
        closeSource();
        if (error instanceof ApiError && [401, 403].includes(error.status)) {
          latest = null;
          patch({ task: null });
        }
        patch({ discovering: false, unavailable: true, statusError: error instanceof Error ? error.message : String(error),
          reconnecting: !!latest && !isRouterUpgradeTerminal(latest.status) });
      } finally {
        polling = false;
        const active = latest && !isRouterUpgradeTerminal(latest.status);
        schedule(active ? (source ? 5000 : 2000) : 10_000);
      }
    }

    async function start() {
      if (!current() || actionPending || (latest && !isRouterUpgradeTerminal(latest.status))) return;
      actionPending = true;
      patch({ creating: true, actionError: "" });
      try {
        const { task } = await request((signal) => startUpgrade(signal, context), 30_000);
        if (!current()) return;
        adopt(task);
        patch({ unavailable: false });
        connect(latest!);
      } catch (error) {
        // A rejection (e.g. 409 while a restart is pending) says nothing about
        // status availability; the poll scheduled below decides that.
        patch({ actionError: error instanceof Error ? error.message : String(error) });
      } finally {
        actionPending = false;
        patch({ creating: false });
        schedule(0);
      }
    }

    async function cancel() {
      if (!current() || actionPending || !latest?.canCancel) return;
      actionPending = true;
      const taskId = latest.taskId;
      patch({ cancelling: true, actionError: "" });
      try {
        const task = await request((signal) => cancelRouterUpgrade(taskId, signal, context));
        if (current()) adopt(task);
      } catch (error) {
        patch({ actionError: error instanceof Error ? error.message : String(error) });
      } finally {
        actionPending = false;
        patch({ cancelling: false });
        schedule(0);
      }
    }

    function dismiss() {
      if (!current() || !latest || !isRouterUpgradeTerminal(latest.status)) return;
      try { localStorage.setItem(storageKey, latest.taskId); } catch { /* Retain in-memory dismissal. */ }
      patch({ dismissedId: latest.taskId, actionError: "" });
    }
    const onStorage = (event: StorageEvent) => {
      if (event.key === storageKey) patch({ dismissedId: event.newValue || "" });
    };
    const onVisible = () => { if (document.visibilityState === "visible") schedule(0); };
    const controller: UpgradeCommands = { start: () => void start(), cancel: () => void cancel(), retry: () => schedule(0), dismiss };
    commands.current = controller;
    window.addEventListener("storage", onStorage);
    document.addEventListener("visibilitychange", onVisible);
    void poll();
    return () => {
      stopped = true;
      clearTimeout(timer);
      closeSource();
      for (const request of requests) request.abort();
      if (commands.current === controller) commands.current = null;
      window.removeEventListener("storage", onStorage);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, [authorized, accountId, owner, scope]);

  const state = view.scope === scope ? view : initialView(scope);
  const active = !!state.task && !isRouterUpgradeTerminal(state.task.status);
  return {
    ...state,
    active,
    locked: enabled && (!authorized || state.discovering || state.unavailable || state.creating || state.cancelling || active || !!state.task?.persistenceError),
    visible: authorized && (state.creating || (!!state.task && (active || state.dismissedId !== state.task.taskId))),
    start: () => commands.current?.start(),
    cancel: () => commands.current?.cancel(),
    retry: () => commands.current?.retry(),
    dismiss: () => commands.current?.dismiss(),
  };
}
