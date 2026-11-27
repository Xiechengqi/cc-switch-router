"use client";
/*
 * The card's "Ops" (运维) button: console, terminal, upgrade and subdomain
 * switch behind one entry. It renders only when the viewer may use at least one
 * of them (`clientOpsAccess`), so visitors see the card's public actions (logs,
 * chat) and nothing else.
 *
 * The menu opens on hover with a mouse and on click or tap everywhere. The
 * popover is non-modal: a modal one lays an underlay over the trigger, which
 * ends the hover the moment the menu opens.
 */
import { Dropdown } from "@heroui/react";
import { ArrowRightLeft, ChevronDown, Terminal, Wrench } from "lucide-react";
import * as React from "react";
import { useClientConsole } from "@/components/dashboard/client-console";
import { clientOpsAccess } from "@/components/dashboard/client-ops";
import { clientUpgradeInput, UpgradeStateIcon, useClientUpgrade } from "@/components/dashboard/client-upgrade-button";
import { clientTunnelDisplayUrl } from "@/components/dashboard/data-tables";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { clientWebTerminalUrl } from "@/lib/client-web-view";
import type { DashboardClient } from "@/lib/types";
import { cn } from "@/lib/utils";

const HOVER_OPEN_DELAY_MS = 120;
// Long enough to cross the gap between the trigger and the menu.
const HOVER_CLOSE_DELAY_MS = 200;

export function ClientConsoleIcon({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 16 16" fill="none" aria-hidden="true" className={className}>
      <rect x="1.5" y="2.5" width="13" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.25" />
      <path d="M5.5 12.5h5" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <path d="M8 12.5V14" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <path d="M4.5 6.25h7" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
      <path d="M4.5 8.75h4.5" stroke="currentColor" strokeWidth="1.25" strokeLinecap="round" />
    </svg>
  );
}

/**
 * Open state for a menu that opens on hover and on press. "hover" menus close
 * when the pointer leaves; a press pins the menu, which then closes on an
 * action, Escape, a second press or a press outside.
 */
function useHoverMenu() {
  // false, or how the menu was opened. The ref mirrors it for event handlers.
  const [state, setState] = React.useState<false | "hover" | "press">(false);
  const current = React.useRef<false | "hover" | "press">(false);
  const set = React.useCallback((next: false | "hover" | "press") => {
    current.current = next;
    setState(next);
  }, []);
  const open = state !== false;
  const timer = React.useRef<number | undefined>(undefined);
  // Set while a press on the trigger is closing the menu, see below.
  const closingByPress = React.useRef(false);
  const triggerRef = React.useRef<HTMLSpanElement>(null);
  const contentRef = React.useRef<HTMLDivElement>(null);

  const clearTimer = () => window.clearTimeout(timer.current);
  React.useEffect(() => clearTimer, []);

  const onPointerEnter = (event: React.PointerEvent) => {
    if (event.pointerType !== "mouse") return;
    clearTimer();
    if (current.current) return;
    timer.current = window.setTimeout(() => set("hover"), HOVER_OPEN_DELAY_MS);
  };
  const onPointerLeave = (event: React.PointerEvent) => {
    if (event.pointerType !== "mouse") return;
    clearTimer();
    if (current.current !== "hover") return;
    timer.current = window.setTimeout(() => {
      // Leaving the menu straight onto the trigger fires no enter on the
      // trigger's wrapper (the portaled menu is its React child), so check.
      if (triggerRef.current?.matches(":hover") || contentRef.current?.matches(":hover")) return;
      set(false);
    }, HOVER_CLOSE_DELAY_MS);
  };
  // A mouse or pen press only ever *opens* a React Aria menu trigger, so the
  // second half of the toggle lives here. Capture runs before the trigger's own
  // press handling, which then finds the menu already in its new state. Touch
  // toggles through `onOpenChange` by itself.
  const onTriggerPointerDownCapture = (event: React.PointerEvent) => {
    if (event.pointerType === "touch" || event.button !== 0) return;
    clearTimer();
    // Clicking a menu the hover opened pins it instead of shutting it under the cursor.
    if (current.current === "hover") set("press");
    else if (current.current === "press") {
      // The trigger's press then blurs the menu (closing it) and calls
      // open() on the now-closed state, which would reopen it at once.
      closingByPress.current = true;
      window.setTimeout(() => {
        closingByPress.current = false;
      }, 0);
      set(false);
    }
  };
  const onOpenChange = (next: boolean) => {
    clearTimer();
    if (next) {
      if (!current.current && !closingByPress.current) set("press");
      return;
    }
    set(false);
  };

  // A non-modal popover does not close on outside presses by itself.
  React.useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (target && (triggerRef.current?.contains(target) || contentRef.current?.contains(target))) return;
      set(false);
    };
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => document.removeEventListener("pointerdown", onPointerDown, true);
  }, [open, set]);

  return {
    open,
    pressed: state === "press",
    onOpenChange,
    triggerProps: { ref: triggerRef, onPointerEnter, onPointerLeave, onPointerDownCapture: onTriggerPointerDownCapture },
    contentProps: { ref: contentRef, onPointerEnter, onPointerLeave },
  };
}

export function ClientOpsMenu({ client, onOpenTakeover }: { client: DashboardClient; onOpenTakeover?: () => void }) {
  const { t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const access = clientOpsAccess(client, !!onOpenTakeover);
  // Mounted with the card, not the menu, so a running upgrade keeps being
  // tracked and its confirm dialog survives the menu closing.
  const upgrade = useClientUpgrade(clientUpgradeInput(client));
  const menu = useHoverMenu();
  const tunnelUrl = clientTunnelDisplayUrl(client.clientTunnel?.tunnelUrl);
  // Without a tunnel nothing can be upgraded; only an upgrade already under way
  // (not the silent status check every card starts with) keeps the item.
  const showUpgrade = access.upgrade && upgrade.visible && (!!tunnelUrl || upgrade.attention);
  if (!access.console && !access.terminal && !showUpgrade && !access.takeover) return upgrade.dialog;

  const title = client.clientTunnel?.subdomain || tunnelUrl;
  const upgradeIconTone = upgrade.state.phase === "failed"
    ? "text-rose-600"
    : upgrade.state.statusUnavailable || upgrade.state.phase === "recovering" ? "text-amber-600" : "text-violet-600";
  const label = t("dashboard.clientOps");
  const flagged = showUpgrade && upgrade.attention;

  return (
    <>
      <span className="inline-flex shrink-0" data-no-row-drawer {...menu.triggerProps}>
        <Dropdown isOpen={menu.open} onOpenChange={menu.onOpenChange}>
          <Dropdown.Trigger
            aria-label={flagged ? `${label} · ${upgrade.label}` : label}
            className={cn(
              "inline-flex h-6 shrink-0 items-center gap-1 rounded-full border bg-white px-2.5 text-[11px] font-medium text-slate-700 outline-none transition-colors hover:border-slate-300 hover:bg-slate-50 aria-expanded:border-slate-300 aria-expanded:bg-slate-50 data-[focus-visible]:ring-2 data-[focus-visible]:ring-primary/30",
              flagged && upgrade.state.phase === "failed" ? "border-rose-200" : "border-slate-200",
            )}
          >
            {/* A running or failed upgrade shows on the closed button too. */}
            {flagged ? (
              <span className={upgradeIconTone}><UpgradeStateIcon state={upgrade.state} /></span>
            ) : (
              <Wrench className="h-3 w-3 shrink-0" aria-hidden />
            )}
            <span>{label}</span>
            <ChevronDown className={cn("h-3 w-3 shrink-0 text-slate-400 transition-transform", menu.open && "rotate-180")} aria-hidden />
          </Dropdown.Trigger>
          <Dropdown.Popover placement="bottom start" isNonModal className="min-w-[10rem]">
            <div data-no-row-drawer {...menu.contentProps}>
              <Dropdown.Menu
                aria-label={t("dashboard.clientOpsMenu")}
                // A hover-opened menu leaves focus where it is; a press or key
                // opens it the React Aria way (ArrowUp focuses the last item).
                {...(menu.pressed ? {} : { autoFocus: false })}
              >
                {access.console ? (
                  <Dropdown.Item
                    id="console"
                    textValue={t("dashboard.clientConsole")}
                    className="gap-2 text-sm font-medium"
                    onAction={() => openConsole({
                      clientId: client.rental?.controlId || client.installation.id,
                      kind: "console",
                      url: tunnelUrl,
                      title,
                    })}
                  >
                    <ClientConsoleIcon className="h-3.5 w-3.5 shrink-0 text-sky-600" />
                    {t("dashboard.clientConsole")}
                  </Dropdown.Item>
                ) : null}
                {access.terminal ? (
                  <Dropdown.Item
                    id="terminal"
                    textValue={t("dashboard.clientTerminal")}
                    className="gap-2 text-sm font-medium"
                    onAction={() => openConsole({
                      clientId: client.installation.id,
                      kind: "terminal",
                      url: clientWebTerminalUrl(tunnelUrl),
                      title: t("dashboard.clientTerminalTitle", { target: title }),
                    })}
                  >
                    <Terminal className="h-3.5 w-3.5 shrink-0 text-emerald-600" aria-hidden />
                    {t("dashboard.clientTerminal")}
                  </Dropdown.Item>
                ) : null}
                {showUpgrade ? (
                  <Dropdown.Item
                    id="upgrade"
                    textValue={upgrade.label}
                    aria-label={upgrade.ariaLabel}
                    isDisabled={upgrade.disabled}
                    className="items-start gap-2 text-sm font-medium"
                    onAction={upgrade.request}
                  >
                    <span className={cn("mt-0.5", upgradeIconTone)}><UpgradeStateIcon state={upgrade.state} /></span>
                    <span className="grid min-w-0 gap-0.5">
                      <span>{upgrade.label}</span>
                      {/* No hover on touch screens: show why the upgrade failed. */}
                      {upgrade.errorMessage ? (
                        <span className="line-clamp-2 max-w-[16rem] text-xs font-normal text-slate-500">{upgrade.errorMessage}</span>
                      ) : null}
                    </span>
                  </Dropdown.Item>
                ) : null}
                {access.takeover && onOpenTakeover ? (
                  <Dropdown.Item
                    id="takeover"
                    textValue={t("dashboard.subdomainTakeover.action")}
                    className="gap-2 text-sm font-medium"
                    onAction={onOpenTakeover}
                  >
                    <ArrowRightLeft className="h-3.5 w-3.5 shrink-0 text-amber-600" aria-hidden />
                    {t("dashboard.subdomainTakeover.action")}
                  </Dropdown.Item>
                ) : null}
              </Dropdown.Menu>
            </div>
          </Dropdown.Popover>
        </Dropdown>
      </span>
      {upgrade.dialog}
    </>
  );
}
