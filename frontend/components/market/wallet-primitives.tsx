"use client";

/* Shared wallet primitives (Minimalist Modern on the existing Router tokens):
 * copy button, chain-explorer link, pill section label and status badge.
 * Reused by the buyer wallet, the direct checkout page and the Owner console.
 */

import * as React from "react";
import { Check, Copy, ExternalLink } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { tronTransactionUrl } from "@/lib/market-explorer";
import { cn } from "@/lib/utils";

const FOCUS_RING = "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";

/** Copies `value`; the accessible name flips to "Copied" for two seconds. */
export function CopyTextButton({ value, label, className, size = "md" }: {
  value: string;
  /** Accessible label, e.g. "Copy address". */
  label?: string;
  className?: string;
  size?: "sm" | "md";
}) {
  const { t } = useLocaleText();
  const [copied, setCopied] = React.useState(false);
  const timer = React.useRef<number | undefined>(undefined);
  React.useEffect(() => () => window.clearTimeout(timer.current), []);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };
  const name = copied ? t("marketWallet.copied") : label ?? t("marketWallet.copy");
  return (
    <button
      type="button"
      onClick={() => void copy()}
      aria-label={name}
      title={name}
      className={cn(
        "inline-flex shrink-0 items-center justify-center rounded-lg border border-border bg-card text-muted-foreground transition-all duration-200 ease-out hover:-translate-y-0.5 hover:border-accent/30 hover:text-foreground hover:shadow-sm active:scale-[0.98] motion-reduce:transition-none motion-reduce:hover:translate-y-0",
        size === "sm" ? "size-9" : "size-11",
        FOCUS_RING,
        className,
      )}
    >
      {copied ? <Check className="size-4 text-emerald-700" aria-hidden="true" /> : <Copy className="size-4" aria-hidden="true" />}
      <span className="sr-only" aria-live="polite">{copied ? t("marketWallet.copied") : ""}</span>
    </button>
  );
}

/** A transaction hash rendered as a TRON explorer link when it is one. */
export function TronTransactionLink({ hash, className }: { hash: string; className?: string }) {
  const { t } = useLocaleText();
  const href = tronTransactionUrl(hash);
  const short = hash.length > 20 ? `${hash.slice(0, 10)}…${hash.slice(-8)}` : hash;
  if (!href) return <span className={cn("break-all font-mono text-xs", className)}>{hash}</span>;
  return (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      title={hash}
      aria-label={t("marketWallet.viewOnExplorer", { hash: short })}
      className={cn("inline-flex min-h-11 items-center gap-1 break-all font-mono text-xs text-accent underline-offset-2 hover:underline", FOCUS_RING, className)}
    >
      {short}<ExternalLink className="size-3 shrink-0" aria-hidden="true" />
    </a>
  );
}

/** Pill section label with an accent dot (optionally pulsing). */
export function WalletSectionLabel({ children, pulse = false, tone = "accent", className }: {
  children: React.ReactNode;
  pulse?: boolean;
  tone?: "accent" | "inverted";
  className?: string;
}) {
  return (
    <span className={cn(
      "inline-flex w-fit items-center gap-2 rounded-full border px-3 py-1 font-mono text-[0.68rem] uppercase tracking-[0.15em]",
      tone === "accent" ? "border-accent/30 bg-accent/5 text-accent" : "border-white/20 bg-white/5 text-white/80",
      className,
    )}>
      <span className={cn("size-1.5 rounded-full", tone === "accent" ? "bg-accent" : "bg-[#4D7CFF]", pulse && "pulse-dot")} aria-hidden="true" />
      {children}
    </span>
  );
}

export type WalletBadgeTone = "neutral" | "accent" | "success" | "warning" | "danger";

const BADGE_TONE: Record<WalletBadgeTone, string> = {
  neutral: "bg-muted text-foreground",
  accent: "bg-accent/10 text-accent",
  success: "bg-emerald-100 text-emerald-900",
  warning: "bg-amber-100 text-amber-900",
  danger: "bg-red-100 text-red-800",
};

export function WalletBadge({ children, tone = "neutral", pulse = false, className }: {
  children: React.ReactNode;
  tone?: WalletBadgeTone;
  /** Adds a live dot; animation is disabled under prefers-reduced-motion. */
  pulse?: boolean;
  className?: string;
}) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 whitespace-nowrap rounded-full px-2.5 py-1 text-xs font-medium", BADGE_TONE[tone], className)}>
      {pulse ? <span className="size-1.5 rounded-full bg-current pulse-dot" aria-hidden="true" /> : null}
      {children}
    </span>
  );
}

/** Gradient primary action (Electric Blue), 44px+ target. */
export const WALLET_PRIMARY_BUTTON = cn(
  "group inline-flex min-h-11 items-center justify-center gap-2 whitespace-nowrap rounded-xl bg-gradient-to-r from-[#0052FF] to-[#4D7CFF] px-5 text-sm font-semibold text-white shadow-sm transition-all duration-200 ease-out hover:-translate-y-0.5 hover:brightness-110 hover:shadow-[0_8px_24px_rgba(0,82,255,0.35)] active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50 motion-reduce:transition-none motion-reduce:hover:translate-y-0",
  FOCUS_RING,
);

/** Outline secondary action, 44px+ target. */
export const WALLET_SECONDARY_BUTTON = cn(
  "inline-flex min-h-11 items-center justify-center gap-2 whitespace-nowrap rounded-xl border border-border bg-transparent px-4 text-sm font-medium text-foreground transition-all duration-200 ease-out hover:border-accent/30 hover:bg-muted hover:shadow-sm active:scale-[0.98] disabled:pointer-events-none disabled:opacity-50 motion-reduce:transition-none",
  FOCUS_RING,
);

/** Standard text input styling used by the new wallet forms. */
export const WALLET_INPUT = "h-11 min-w-0 rounded-lg border border-input bg-background px-3 text-sm outline-2 outline-transparent transition-colors focus-visible:outline-ring aria-invalid:border-destructive disabled:cursor-not-allowed disabled:opacity-50 placeholder:text-muted-foreground/50";
