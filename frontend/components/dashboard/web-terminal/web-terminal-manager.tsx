"use client";

import { toast } from "@heroui/react";
import { usePathname, useRouter } from "next/navigation";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  CONSOLE_BASE_Z_INDEX,
  CONSOLE_DOCK_HEIGHT,
} from "@/components/dashboard/client-console/client-console-manager";
import {
  DASHBOARD_CLIENT_MARKET_PATH,
  isClientMarketRoute,
} from "@/lib/dashboard-nav";

export {
  CONSOLE_DOCK_BOTTOM_INSET,
  CONSOLE_DOCK_HEIGHT,
  CONSOLE_DOCK_RESERVED_HEIGHT,
} from "@/components/dashboard/client-console/client-console-manager";

export const MAX_WEB_TERMINAL_WINDOWS = 5;
export const WEB_TERMINAL_BASE_Z_INDEX = CONSOLE_BASE_Z_INDEX + 20;
const SESSION_STORAGE_KEY = "cc_switch_router_web_terminal_windows_v1";
const CHROME_HEIGHT = 52;
const DEFAULT_BODY_HEIGHT = 420;

export type WebTerminalWindowState = "normal" | "minimized" | "maximized";

export type WebTerminalRect = {
  x: number;
  y: number;
  width: number;
  height: number;
};

export type WebTerminalWindow = {
  id: string;
  hostId: string;
  title: string;
  state: WebTerminalWindowState;
  zIndex: number;
  normalRect: WebTerminalRect;
  activated: boolean;
};

type PersistedWindow = Pick<WebTerminalWindow, "id" | "hostId" | "title" | "state" | "normalRect" | "zIndex">;

type ManagerState = {
  windows: WebTerminalWindow[];
  nextZIndex: number;
  focusedId: string | null;
};

type OpenPayload = {
  hostId: string;
  title: string;
};

type WebTerminalContextValue = {
  windows: WebTerminalWindow[];
  focusedId: string | null;
  dockCount: number;
  dockVisible: boolean;
  openTerminal: (payload: OpenPayload) => void;
  closeTerminal: (id: string) => void;
  minimizeTerminal: (id: string) => void;
  restoreTerminal: (id: string) => void;
  toggleMaximizeTerminal: (id: string) => void;
  focusTerminal: (id: string) => void;
  updateTerminalRect: (id: string, rect: WebTerminalRect) => void;
  closeAllTerminals: () => void;
};

const WebTerminalContext = React.createContext<WebTerminalContextValue | null>(null);

function defaultRect(index: number): WebTerminalRect {
  if (typeof window === "undefined") {
    return { x: 48, y: 48, width: 860, height: CHROME_HEIGHT + DEFAULT_BODY_HEIGHT };
  }
  const width = Math.min(860, window.innerWidth - 40);
  const bodyHeight = Math.min(window.innerHeight * 0.5, DEFAULT_BODY_HEIGHT);
  const height = bodyHeight + CHROME_HEIGHT;
  const x = Math.max(12, (window.innerWidth - width) / 2 + index * 24);
  const y = Math.max(12, (window.innerHeight - height) / 2 + index * 24 - CONSOLE_DOCK_HEIGHT / 2);
  return { x, y, width, height };
}

function createWindow(payload: OpenPayload, index: number, zIndex: number): WebTerminalWindow {
  return {
    id: crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`,
    hostId: payload.hostId,
    title: payload.title,
    state: "normal",
    zIndex,
    normalRect: defaultRect(index),
    activated: true,
  };
}

function isDocked(window: WebTerminalWindow) {
  return window.state === "minimized" || !window.activated;
}

function readPersistedWindows(): PersistedWindow[] {
  if (typeof window === "undefined") return [];
  try {
    const raw = window.sessionStorage.getItem(SESSION_STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((item): item is PersistedWindow => {
      return (
        !!item &&
        typeof item === "object" &&
        typeof (item as PersistedWindow).id === "string" &&
        typeof (item as PersistedWindow).hostId === "string"
      );
    });
  } catch {
    return [];
  }
}

function writePersistedWindows(windows: WebTerminalWindow[]) {
  if (typeof window === "undefined") return;
  try {
    const payload: PersistedWindow[] = windows.map(({ id, hostId, title, state, normalRect, zIndex }) => ({
      id,
      hostId,
      title,
      state,
      normalRect,
      zIndex,
    }));
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

function minimizeOtherVisibleWindows(windows: WebTerminalWindow[], exceptId: string): WebTerminalWindow[] {
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
      const persisted = action.payload as PersistedWindow[];
      if (!persisted.length) return state;
      const maxZ = persisted.reduce(
        (max, window) => Math.max(max, window.zIndex || WEB_TERMINAL_BASE_Z_INDEX),
        WEB_TERMINAL_BASE_Z_INDEX,
      );
      return {
        windows: persisted.map((window, index) => ({
          ...window,
          state: "minimized" as const,
          zIndex: window.zIndex || WEB_TERMINAL_BASE_Z_INDEX + index,
          normalRect: window.normalRect || defaultRect(index),
          activated: false,
        })),
        nextZIndex: maxZ + 1,
        focusedId: null,
      };
    }
    case "OPEN": {
      const payload = action.payload as OpenPayload;
      const existing = state.windows.find((window) => window.hostId === payload.hostId);
      if (existing) {
        const nextState = existing.state === "minimized" || !existing.activated ? "normal" : existing.state;
        const bumped = bumpFocus(state, existing.id);
        return {
          ...bumped,
          windows: minimizeOtherVisibleWindows(bumped.windows, existing.id).map((window) =>
            window.id === existing.id
              ? { ...window, title: payload.title, state: nextState, activated: true }
              : window,
          ),
        };
      }
      if (state.windows.length >= MAX_WEB_TERMINAL_WINDOWS) return state;
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
        focusedId:
          state.focusedId === id ? windows.find((window) => !isDocked(window))?.id ?? null : state.focusedId,
      };
    }
    case "CLOSE_ALL":
      if (!state.windows.length) return state;
      return {
        windows: [],
        nextZIndex: WEB_TERMINAL_BASE_Z_INDEX,
        focusedId: null,
      };
    case "MINIMIZE": {
      const id = action.payload as string;
      return {
        ...state,
        windows: state.windows.map((window) =>
          window.id === id ? { ...window, state: "minimized" as const } : window,
        ),
        focusedId:
          state.focusedId === id
            ? state.windows.find((window) => window.id !== id && !isDocked(window))?.id ?? null
            : state.focusedId,
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
          if (nextState === "maximized" && window.state === "maximized") {
            return { ...window, state: "normal" as const };
          }
          return window;
        }),
      };
    }
    case "FOCUS":
      return bumpFocus(state, action.payload as string);
    case "MINIMIZE_ALL_VISIBLE": {
      const hasVisible = state.windows.some((window) => window.activated && window.state !== "minimized");
      if (!hasVisible) return state;
      return {
        ...state,
        windows: state.windows.map((window) =>
          window.activated && window.state !== "minimized"
            ? { ...window, state: "minimized" as const }
            : window,
        ),
        focusedId: null,
      };
    }
    case "UPDATE_RECT": {
      const { id, rect } = action.payload as { id: string; rect: WebTerminalRect };
      return {
        ...state,
        windows: state.windows.map((window) => (window.id === id ? { ...window, normalRect: rect } : window)),
      };
    }
    default:
      return state;
  }
}

export function WebTerminalManagerProvider({ children }: { children: React.ReactNode }) {
  const { t } = useLocaleText();
  const router = useRouter();
  const pathname = usePathname() || DASHBOARD_CLIENT_MARKET_PATH;
  const [state, dispatch] = React.useReducer(reducer, {
    windows: [],
    nextZIndex: WEB_TERMINAL_BASE_Z_INDEX,
    focusedId: null,
  });
  const [hydrated, setHydrated] = React.useState(false);
  const autoMinimizeNotifiedRef = React.useRef(false);
  const prevPathRef = React.useRef(pathname);
  const windowsRef = React.useRef(state.windows);
  windowsRef.current = state.windows;

  const ensureClientMarketRoute = React.useCallback(() => {
    if (!isClientMarketRoute(pathname)) {
      router.push(DASHBOARD_CLIENT_MARKET_PATH);
    }
  }, [pathname, router]);

  React.useLayoutEffect(() => {
    const previousPath = prevPathRef.current;
    prevPathRef.current = pathname;
    if (isClientMarketRoute(pathname)) {
      autoMinimizeNotifiedRef.current = false;
      return;
    }
    if (!isClientMarketRoute(previousPath)) return;
    const hadVisible = windowsRef.current.some(
      (window) => window.activated && window.state !== "minimized",
    );
    if (!hadVisible) return;
    dispatch({ type: "MINIMIZE_ALL_VISIBLE" });
    if (!autoMinimizeNotifiedRef.current) {
      autoMinimizeNotifiedRef.current = true;
      toast.info(t("clientMarket.terminal.autoMinimized"));
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

  const openTerminal = React.useCallback(
    (payload: OpenPayload) => {
      const existing = state.windows.find((window) => window.hostId === payload.hostId);
      if (!existing && state.windows.length >= MAX_WEB_TERMINAL_WINDOWS) {
        toast.warning(t("clientMarket.terminal.maxWindows", { count: MAX_WEB_TERMINAL_WINDOWS }));
        return;
      }
      ensureClientMarketRoute();
      dispatch({ type: "OPEN", payload });
    },
    [ensureClientMarketRoute, state.windows, t],
  );

  const restoreTerminal = React.useCallback(
    (id: string) => {
      ensureClientMarketRoute();
      dispatch({ type: "RESTORE", payload: id });
    },
    [ensureClientMarketRoute],
  );

  const value = React.useMemo<WebTerminalContextValue>(() => {
    const dockCount = state.windows.length;
    return {
      windows: state.windows,
      focusedId: state.focusedId,
      dockCount,
      dockVisible: dockCount > 0 && isClientMarketRoute(pathname),
      openTerminal,
      closeTerminal: (id) => dispatch({ type: "CLOSE", payload: id }),
      minimizeTerminal: (id) => dispatch({ type: "MINIMIZE", payload: id }),
      restoreTerminal,
      toggleMaximizeTerminal: (id) => {
        if (!isClientMarketRoute(pathname)) {
          ensureClientMarketRoute();
        }
        dispatch({ type: "TOGGLE_MAXIMIZE", payload: id });
      },
      focusTerminal: (id) => dispatch({ type: "FOCUS", payload: id }),
      updateTerminalRect: (id, rect) => dispatch({ type: "UPDATE_RECT", payload: { id, rect } }),
      closeAllTerminals: () => dispatch({ type: "CLOSE_ALL" }),
    };
  }, [ensureClientMarketRoute, openTerminal, pathname, restoreTerminal, state.focusedId, state.windows]);

  return <WebTerminalContext.Provider value={value}>{children}</WebTerminalContext.Provider>;
}

export function useWebTerminal() {
  const value = React.useContext(WebTerminalContext);
  if (!value) throw new Error("useWebTerminal must be used within WebTerminalManagerProvider");
  return value;
}

export function useWebTerminalOptional() {
  return React.useContext(WebTerminalContext);
}
