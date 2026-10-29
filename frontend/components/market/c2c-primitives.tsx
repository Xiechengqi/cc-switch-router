"use client";
/* Hallmark · component: C2C controls · genre: modern-minimal · theme: existing Router tokens
 * audience: wallet holders · use: explicit credit exchange · tone: concise, factual
 * states: default · hover · focus · active · disabled · loading · error · success
 * contrast: inherited Router tokens · responsive code reviewed; no visual verification in this revision
 * pre-emit critique: P5 H5 E4 S5 R5 V4 · component-scope: no macrostructure
 */
import * as React from "react";
import { Check, CircleAlert, Loader2, X } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import type { C2CStatus } from "@/lib/market-c2c";

export const C2C_INPUT = "min-h-11 w-full min-w-0 rounded-md border border-input bg-background px-3 py-2 text-sm outline-2 outline-transparent hover:bg-muted/30 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive";
export function C2CAction({ phase = "default", previewState, secondary, children, className = "", ...props }: React.ButtonHTMLAttributes<HTMLButtonElement> & {
  phase?: "default" | "loading" | "error" | "success"; previewState?: "hover" | "focus" | "active"; secondary?: boolean;
}) {
  return <button {...props} data-state={phase} data-preview={previewState} aria-busy={phase === "loading"} disabled={props.disabled || phase === "loading"}
    className={`inline-flex min-h-11 min-w-0 items-center justify-center gap-2 whitespace-nowrap rounded-md px-3 py-2 text-sm font-semibold focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring active:translate-y-px disabled:cursor-not-allowed disabled:opacity-50 data-[preview=focus]:outline-2 data-[preview=focus]:outline-offset-2 data-[preview=focus]:outline-ring data-[preview=active]:translate-y-px data-[state=error]:outline-2 data-[state=error]:outline-destructive motion-reduce:transform-none ${secondary ? "bg-muted text-foreground hover:bg-muted/75 data-[preview=hover]:bg-muted/75" : "bg-primary text-primary-foreground hover:bg-primary/90 data-[preview=hover]:bg-primary/90"} ${className}`}>
    {phase === "loading" ? <Loader2 className="size-4 shrink-0 animate-spin motion-reduce:animate-none" aria-hidden="true" /> : phase === "error" ? <CircleAlert className="size-4 shrink-0" aria-hidden="true" /> : phase === "success" ? <Check className="size-4 shrink-0" aria-hidden="true" /> : null}{children}
  </button>;
}
export function C2CError({ children }: { children?: React.ReactNode }) { return children ? <p role="alert" className="flex min-w-0 items-start gap-2 rounded-md bg-destructive/5 p-3 text-sm text-foreground"><CircleAlert className="mt-0.5 size-4 shrink-0 text-destructive" aria-hidden="true" /><span className="min-w-0 break-words">{children}</span></p> : null; }
export function C2CCheck({ children, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return <label className="flex min-h-11 cursor-pointer items-start gap-3 py-2 text-sm leading-6"><input {...props} type="checkbox" className="mt-1 size-4 shrink-0 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed" /><span className="min-w-0 break-words">{children}</span></label>;
}
export function C2CStatusLabel({ status }: { status: C2CStatus }) {
  const { t } = useLocaleText();
  return <span className={`inline-flex w-fit max-w-full rounded-md px-2 py-1 text-xs font-medium ${status === "disputed" ? "bg-destructive/5 text-foreground" : status === "released" ? "bg-primary/10 text-primary" : "bg-muted text-foreground"}`}>{t(`c2c.status.${status}`)}</span>;
}
export function C2CDialog({ open, title, busy = false, inline = false, onClose, children }: { open: boolean; title: string; busy?: boolean; inline?: boolean; onClose: () => void; children: React.ReactNode }) {
  const { t } = useLocaleText(); const dialog = React.useRef<HTMLDialogElement>(null); const heading = React.useId();
  const inlineRegion = React.useRef<HTMLElement>(null);
  React.useEffect(() => {
    if (!inline || !open) return;
    const previous = document.activeElement as HTMLElement | null;
    inlineRegion.current?.focus({ preventScroll: true });
    inlineRegion.current?.scrollIntoView({ block: "nearest" });
    return () => { if (previous?.isConnected) previous.focus({ preventScroll: true }); };
  }, [inline, open]);
  const backdropPointerDown = React.useRef(false);
  const outsideDialog = (event: React.MouseEvent<HTMLDialogElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    return event.clientX < bounds.left || event.clientX > bounds.right || event.clientY < bounds.top || event.clientY > bounds.bottom;
  };
  React.useEffect(() => {
    const d = dialog.current; if (!d) return;
    if (open && !d.open) d.showModal(); else if (!open && d.open) d.close();
    return () => { if (d.open) d.close(); };
  }, [inline, open]);
  const content = <div className="grid min-w-0 gap-4 p-4 sm:p-6">
    <header className="flex min-w-0 items-start justify-between gap-2"><h2 id={heading} className="min-w-0 break-words text-lg font-semibold tracking-tight">{title}</h2><C2CAction type="button" secondary aria-label={t("common.close")} disabled={busy} onClick={onClose}><X className="size-4" aria-hidden="true" /></C2CAction></header>
    {children}
  </div>;
  if (inline) return open ? <section ref={inlineRegion} role="region" tabIndex={-1} aria-labelledby={heading} className="min-w-0 border-t border-border outline-none focus-visible:outline-2 focus-visible:outline-ring">{content}</section> : null;
  return <dialog ref={dialog} aria-labelledby={heading} tabIndex={-1}
    onPointerDownCapture={e => { backdropPointerDown.current = e.target === e.currentTarget && outsideDialog(e); }}
    onPointerCancel={() => { backdropPointerDown.current = false; }}
    onKeyDown={e => {
      if (e.key !== "Tab" || e.defaultPrevented) return;
      const d = e.currentTarget;
      const targets = [...d.querySelectorAll<HTMLElement>('a[href],button,input,select,textarea,[tabindex]')].filter(el => el.tabIndex >= 0 && !el.matches(":disabled") && !el.closest("[inert]") && el.getClientRects().length > 0);
      const first = targets[0]; const last = targets[targets.length - 1];
      if (!first) { e.preventDefault(); d.focus(); }
      else if (e.shiftKey && (document.activeElement === first || document.activeElement === d)) { e.preventDefault(); last.focus(); }
      else if (!e.shiftKey && (document.activeElement === last || document.activeElement === d)) { e.preventDefault(); first.focus(); }
    }}
    onCancel={e => { e.preventDefault(); if (!busy) onClose(); }} onClick={e => {
      // A Select opens on pointerdown and makes its siblings inert. The browser
      // may then retarget pointerup/click to this dialog; that is not a backdrop
      // click. Dismiss only a gesture that started and ended on the backdrop.
      const backdrop = backdropPointerDown.current;
      backdropPointerDown.current = false;
      if (backdrop && e.target === e.currentTarget && outsideDialog(e) && !busy) onClose();
    }}
    className="m-auto max-h-[90dvh] w-[calc(100%-2rem)] max-w-lg overflow-y-auto overscroll-contain rounded-xl bg-card p-0 text-foreground shadow-lg backdrop:bg-foreground/30">
    {content}
  </dialog>;
}
