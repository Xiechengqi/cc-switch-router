"use client";

import { toast } from "@heroui/react";
import { usePathname, useRouter } from "next/navigation";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_CLIENTS_PATH, isClientsRoute } from "@/lib/dashboard-nav";

export const MAX_CONSOLE_WINDOWS = 5;
export const CONSOLE_DOCK_HEIGHT = 56;
/** Keep the dock above the dashboard presence footer (~72px tall). */
export const CONSOLE_DOCK_BOTTOM_INSET = 80;
export const CONSOLE_DOCK_RESERVED_HEIGHT = CONSOLE_DOCK_BOTTOM_INSET + CONSOLE_DOCK_HEIGHT;
export const CONSOLE_BASE_Z_INDEX = 45;
const SESSION_STORAGE_KEY = "cc_switch_router_console_windows_v3";
const LEGACY_SESSION_STORAGE_KEYS = [
  "cc_switch_router_console_windows_v2",
  "cc_switch_router_console_windows_v1",
] as const;

export type ConsoleWindowKind = "console" | "terminal";

export function inferConsoleWindowKind(url: string | undefined): ConsoleWindowKind {
  if (!url) return "console";
  try {
    const parsed = new URL(url, "https://cc-switch.invalid");
    return parsed.searchParams.get("view") === "terminal" ? "terminal" : "console";
  } catch {
    return /[?&]view=terminal(?:&|$)/.test(url) ? "terminal" : "console";
  }
}
const CHROME_HEIGHT = 88;
const DEFAULT_IFRAME_HEIGHT = 460;

export type ConsoleWindowState = "normal" | "minimized" | "maximized";

export type NormalRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type ConsoleWindow = {
  id: string;
  clientId: string;
  kind: ConsoleWindowKind;
  url: string;
  title: string;
  state: ConsoleWindowState;
  zIndex: number;
  normalRect: NormalRect;
  /** false after session hydrate until user explicitly resumes loading the iframe */
  activated: boolean;
  reloadKey: number;
};

type PersistedConsoleWindow = Pick<
  ConsoleWindow,
  "id" | "clientId" | "kind" | "url" | "title" | "state" | "normalRect" | "zIndex"
>;

type ManagerState = {
  windows: ConsoleWindow[];
  nextZIndex: number;
  focusedId: string | null;
};

type OpenPayload = {
  clientId: string;
  kind?: ConsoleWindowKind;
  url: string;
  title: string;
};

type ClientConsoleContextValue = {
  windows: ConsoleWindow[];
  focusedId: string | null;
  dockCount: number;
  dockVisible: boolean;
  openConsole: (payload: OpenPayload) => void;
  closeConsole: (id: string) => void;
  minimizeConsole: (id: string) => void;
  restoreConsole: (id: string) => void;
  toggleMaximizeConsole: (id: string) => void;
  focusConsole: (id: string) => void;
  refreshConsole: (id: string) => void;
  updateConsoleRect: (id: string, rect: NormalRect) => void;
  closeAllConsoles: () => void;
};

const ClientConsoleContext = React.createContext<ClientConsoleContextValue | null>(null);

function defaultRect(index: number): NormalRect {
  if (typeof window === "undefined") {
    return { x: 40, y: 40, width: 880, height: CHROME_HEIGHT + DEFAULT_IFRAME_HEIGHT };
  }
  const width = Math.min(880, window.innerWidth - 40);
  const iframeHeight = Math.min(window.innerHeight * 0.48, DEFAULT_IFRAME_HEIGHT);
  const height = iframeHeight + CHROME_HEIGHT;
  const x = Math.max(12, (window.innerWidth - width) / 2 + index * 24);
  const y = Math.max(12, (window.innerHeight - height) / 2 + index * 24 - CONSOLE_DOCK_HEIGHT / 2);
  return { x, y, width, height };
}

function createWindow(payload: OpenPayload, index: number, zIndex: number): ConsoleWindow {
  return {
    id: crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`,
    clientId: payload.clientId,
    kind: payload.kind ?? inferConsoleWindowKind(payload.url),
    url: payload.url,
    title: payload.title,
    state: "normal",
    zIndex,
    normalRect: defaultRect(index),
    activated: true,
    reloadKey: 0,
  };
}

function isDocked(window: ConsoleWindow) {
  return window.state === "minimized" || !window.activated;
}

function readPersistedWindows(): PersistedConsoleWindow[] {
  if (typeof window === "undefined") return [];
  try {
    const raw =
      window.sessionStorage.getItem(SESSION_STORAGE_KEY) ??
      LEGACY_SESSION_STORAGE_KEYS.map((key) => window.sessionStorage.getItem(key)).find(Boolean);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.flatMap((item) => {
      if (
        !item ||
        typeof item !== "object" ||
        typeof (item as PersistedConsoleWindow).id !== "string" ||
        typeof (item as PersistedConsoleWindow).clientId !== "string" ||
        typeof (item as PersistedConsoleWindow).url !== "string"
      ) {
        return [];
      }
      const windowItem = item as PersistedConsoleWindow;
      return [
        {
          ...windowItem,
          kind:
            windowItem.kind === "terminal" || windowItem.kind === "console"
              ? windowItem.kind
              : inferConsoleWindowKind(windowItem.url),
        },
      ];
    });
  } catch {
    return [];
  }
}

function writePersistedWindows(windows: ConsoleWindow[]) {
  if (typeof window === "undefined") return;
  try {
    const payload: PersistedConsoleWindow[] = windows.map(
      ({ id, clientId, kind, url, title, state, normalRect, zIndex }) => ({
        id,
        clientId,
        kind,
        url,
        title,
        state,
        normalRect,
        zIndex,
      }),
    );
    window.sessionStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(payload));
  } catch {
    // sessionStorage may be unavailable.
  }
}

function bumpFocus(state: ManagerState, id: string): ManagerState {
  const nextZ = state.nextZIndex + 1;
  return {
    ...state,
    focusedId: id,
    nextZIndex: nextZ,
    windows: state.windows.map((window) => (window.id === id ? { ...window, zIndex: nextZ } : window)),
  };
}

function minimizeOtherVisibleWindows(windows: ConsoleWindow[], exceptId: string): ConsoleWindow[] {
  return windows.map((window) => {
    if (window.id === exceptId) return window;
    if (window.activated && window.state !== "minimized") {
      return { ...window, state: "minimized" as const };
    }
    return window;
  });
}

function reducer(state: ManagerState, action: { type: string; payload?: unknown }): ManagerState {
  switch (action.type) {
    case "HYDRATE": {
      const persisted = action.payload as PersistedConsoleWindow[];
      if (!persisted.length) return state;
      const maxZ = persisted.reduce((max, window) => Math.max(max, window.zIndex || CONSOLE_BASE_Z_INDEX), CONSOLE_BASE_Z_INDEX);
      return {
        windows: persisted.map((window, index) => ({
          ...window,
          state: "minimized" as const,
          zIndex: window.zIndex || CONSOLE_BASE_Z_INDEX + index,
          normalRect: window.normalRect || defaultRect(index),
          activated: false,
          reloadKey: 0,
        })),
        nextZIndex: maxZ + 1,
        focusedId: null,
      };
    }
    case "OPEN": {
      const payload = action.payload as OpenPayload;
      const kind = payload.kind ?? inferConsoleWindowKind(payload.url);
      const existing = state.windows.find(
        (window) => window.clientId === payload.clientId && window.kind === kind,
      );
      if (existing) {
        const nextState = existing.state === "minimized" || !existing.activated ? "normal" : existing.state;
        const bumped = bumpFocus(state, existing.id);
        return {
          ...bumped,
          windows: minimizeOtherVisibleWindows(bumped.windows, existing.id).map((window) =>
            window.id === existing.id
              ? { ...window, kind, url: payload.url, title: payload.title, state: nextState, activated: true }
              : window,
          ),
        };
      }
      if (state.windows.length >= MAX_CONSOLE_WINDOWS) return state;
      const minimizedOthers = minimizeOtherVisibleWindows(state.windows, "");
      const visibleCount = minimizedOthers.filter((window) => !isDocked(window)).length;
      const created = createWindow(payload, visibleCount, state.nextZIndex);
      return {
        windows: [...minimizedOthers, created],
        nextZIndex: state.nextZIndex + 1,
        focusedId: created.id,
      };
    }
    case "CLOSE": {
      const id = action.payload as string;
      const windows = state.windows.filter((window) => window.id !== id);
      return {
        ...state,
        windows,
        focusedId: state.focusedId === id ? windows.find((window) => !isDocked(window))?.id ?? null : state.focusedId,
      };
    }
    case "CLOSE_ALL":
      if (!state.windows.length) return state;
      return {
        windows: [],
        nextZIndex: CONSOLE_BASE_Z_INDEX,
        focusedId: null,
      };
    case "CLOSE_CLIENTS": {
      const clientIds = action.payload as Set<string>;
      if (!clientIds.size || !state.windows.some((window) => clientIds.has(window.clientId))) return state;
      const windows = state.windows.filter((window) => !clientIds.has(window.clientId));
      return {
        ...state,
        windows,
        focusedId: state.focusedId && clientIds.has(state.windows.find((window) => window.id === state.focusedId)?.clientId || "")
          ? windows.find((window) => !isDocked(window))?.id ?? null
          : state.focusedId,
      };
    }
    case "MINIMIZE": {
      const id = action.payload as string;
      return {
        ...state,
        windows: state.windows.map((window) => (window.id === id ? { ...window, state: "minimized" as const } : window)),
        focusedId: state.focusedId === id ? state.windows.find((window) => window.id !== id && !isDocked(window))?.id ?? null : state.focusedId,
      };
    }
    case "RESTORE": {
      const id = action.payload as string;
      const target = state.windows.find((window) => window.id === id);
      if (!target) return state;
      const bumped = bumpFocus(state, id);
      return {
        ...bumped,
        windows: minimizeOtherVisibleWindows(bumped.windows, id).map((window) =>
          window.id === id ? { ...window, state: "normal" as const, activated: true } : window,
        ),
      };
    }
    case "TOGGLE_MAXIMIZE": {
      const id = action.payload as string;
      const target = state.windows.find((window) => window.id === id);
      if (!target || !target.activated) return state;
      const nextState = target.state === "maximized" ? "normal" : "maximized";
      const bumped = bumpFocus(state, id);
      return {
        ...bumped,
        windows: bumped.windows.map((window) => {
          if (window.id === id) return { ...window, state: nextState };
          if (nextState === "maximized" && window.state === "maximized") return { ...window, state: "normal" as const };
          return window;
        }),
      };
    }
    case "FOCUS":
      return bumpFocus(state, action.payload as string);
    case "REFRESH": {
      const id = action.payload as string;
      return {
        ...state,
        windows: state.windows.map((window) =>
          window.id === id ? { ...window, reloadKey: window.reloadKey + 1 } : window,
        ),
      };
    }
    case "MINIMIZE_ALL_VISIBLE": {
      const hasVisible = state.windows.some((window) => window.activated && window.state !== "minimized");
      if (!hasVisible) return state;
      return {
        ...state,
        windows: state.windows.map((window) =>
          window.activated && window.state !== "minimized" ? { ...window, state: "minimized" as const } : window,
        ),
        focusedId: null,
      };
    }
    case "SUSPEND_ALL": {
      const hasActive = state.windows.some((window) => window.activated);
      if (!hasActive) return state;
      return {
        ...state,
        windows: state.windows.map((window) =>
          window.activated
            ? { ...window, activated: false, state: "minimized" as const }
            : window,
        ),
        focusedId: null,
      };
    }
    case "UPDATE_RECT": {
      const { id, rect } = action.payload as { id: string; rect: NormalRect };
      return {
        ...state,
        windows: state.windows.map((window) => (window.id === id ? { ...window, normalRect: rect } : window)),
      };
    }
    default:
      return state;
  }
}

export function ClientConsoleManagerProvider({ children }: { children: React.ReactNode }) {
  const { t } = useLocaleText();
  const router = useRouter();
  const pathname = usePathname() || DASHBOARD_CLIENTS_PATH;
  const [state, dispatch] = React.useReducer(reducer, { windows: [], nextZIndex: CONSOLE_BASE_Z_INDEX, focusedId: null });
  const [hydrated, setHydrated] = React.useState(false);
  const autoSuspendNotifiedRef = React.useRef(false);
  const prevPathRef = React.useRef(pathname);
  const windowsRef = React.useRef(state.windows);
  windowsRef.current = state.windows;

  const ensureClientsRoute = React.useCallback(() => {
    if (!isClientsRoute(pathname)) {
      router.push(DASHBOARD_CLIENTS_PATH);
    }
  }, [pathname, router]);

  // Soft-nav away from Clients: keep dock entries, suspend iframes as「待恢复」.
  React.useLayoutEffect(() => {
    const previousPath = prevPathRef.current;
    prevPathRef.current = pathname;
    if (isClientsRoute(pathname)) {
      autoSuspendNotifiedRef.current = false;
      return;
    }
    if (!isClientsRoute(previousPath)) return;
    const hadActive = windowsRef.current.some((window) => window.activated);
    if (!hadActive) return;
    dispatch({ type: "SUSPEND_ALL" });
    if (!autoSuspendNotifiedRef.current) {
      autoSuspendNotifiedRef.current = true;
      toast.info(t("dashboard.clientConsole.autoSuspended"));
    }
  }, [pathname, t]);

  React.useEffect(() => {
    const persisted = readPersistedWindows();
    if (persisted.length) dispatch({ type: "HYDRATE", payload: persisted });
    setHydrated(true);
  }, []);

  React.useEffect(() => {
    if (!hydrated) return;
    writePersistedWindows(state.windows);
  }, [hydrated, state.windows]);

  React.useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape" || !state.focusedId) return;
      const focused = state.windows.find((window) => window.id === state.focusedId);
      if (!focused || isDocked(focused)) return;
      event.preventDefault();
      if (focused.state === "maximized") {
        dispatch({ type: "TOGGLE_MAXIMIZE", payload: focused.id });
      } else {
        dispatch({ type: "CLOSE", payload: focused.id });
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [state.focusedId, state.windows]);

  const openConsole = React.useCallback(
    (payload: OpenPayload) => {
      const kind = payload.kind ?? inferConsoleWindowKind(payload.url);
      const existing = state.windows.find(
        (window) => window.clientId === payload.clientId && window.kind === kind,
      );
      if (!existing && state.windows.length >= MAX_CONSOLE_WINDOWS) {
        toast.warning(t("dashboard.clientConsole.maxWindows", { count: MAX_CONSOLE_WINDOWS }));
        return;
      }
      ensureClientsRoute();
      dispatch({ type: "OPEN", payload });
    },
    [ensureClientsRoute, state.windows, t],
  );

  const restoreConsole = React.useCallback(
    (id: string) => {
      ensureClientsRoute();
      dispatch({ type: "RESTORE", payload: id });
    },
    [ensureClientsRoute],
  );

  const value = React.useMemo<ClientConsoleContextValue>(() => {
    const dockCount = state.windows.length;
    return {
      windows: state.windows,
      focusedId: state.focusedId,
      dockCount,
      // Keep dock across dashboard tabs so suspended consoles stay recoverable.
      dockVisible: dockCount > 0,
      openConsole,
      closeConsole: (id) => dispatch({ type: "CLOSE", payload: id }),
      minimizeConsole: (id) => dispatch({ type: "MINIMIZE", payload: id }),
      restoreConsole,
      toggleMaximizeConsole: (id) => {
        if (!isClientsRoute(pathname)) {
          ensureClientsRoute();
        }
        dispatch({ type: "TOGGLE_MAXIMIZE", payload: id });
      },
      focusConsole: (id) => dispatch({ type: "FOCUS", payload: id }),
      refreshConsole: (id) => dispatch({ type: "REFRESH", payload: id }),
      updateConsoleRect: (id, rect) => dispatch({ type: "UPDATE_RECT", payload: { id, rect } }),
      closeAllConsoles: () => dispatch({ type: "CLOSE_ALL" }),
    };
  }, [ensureClientsRoute, openConsole, pathname, restoreConsole, state.focusedId, state.windows]);

  return <ClientConsoleContext.Provider value={value}>{children}</ClientConsoleContext.Provider>;
}

export function useClientConsole() {
  const value = React.useContext(ClientConsoleContext);
  if (!value) throw new Error("useClientConsole must be used within ClientConsoleManagerProvider");
  return value;
}
