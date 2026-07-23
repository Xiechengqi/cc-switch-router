"use client";

import { cn } from "@/lib/utils";

/** Minimal `>_` terminal glyph — cleaner than Lucide TerminalSquare. */
export function WebTerminalGlyph({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={cn("h-4 w-4", className)}
      aria-hidden
    >
      <path
        d="M3.25 4.75 6.5 8 3.25 11.25"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M8.25 11.25h4.5"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
      />
    </svg>
  );
}
