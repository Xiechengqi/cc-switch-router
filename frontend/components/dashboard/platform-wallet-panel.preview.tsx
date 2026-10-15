"use client";

/* Hallmark · preview: platform wallet primary action · 8 states · not routed */
import { WalletActionButton } from "./platform-wallet-panel";

const states = [
  { name: "default", label: "Continue to payment" },
  { name: "hover", label: "Continue to payment", previewState: "hover" as const },
  { name: "focus", label: "Continue to payment", previewState: "focus" as const },
  { name: "active", label: "Continue to payment", previewState: "active" as const },
  { name: "disabled", label: "Continue to payment", disabled: true },
  { name: "loading", label: "Creating order…", phase: "loading" as const },
  { name: "error", label: "Try again", phase: "error" as const },
  { name: "success", label: "Order created", phase: "success" as const },
];

export function PlatformWalletActionPreview() {
  return (
    <section className="grid max-w-md gap-3 rounded-xl border border-border bg-card p-5" aria-label="Wallet action states">
      <h2 className="text-base font-semibold">Wallet action · 8 states</h2>
      {states.map(({ name, label, ...props }) => (
        <div key={name} className="grid grid-cols-[5rem_minmax(0,1fr)] items-center gap-3">
          <span className="text-xs text-muted-foreground">{name}</span>
          <WalletActionButton label={label} {...props} />
        </div>
      ))}
    </section>
  );
}
