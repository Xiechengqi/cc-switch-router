"use client";

import { Download } from "lucide-react";

export function SectionInstallButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="inline-flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-slate-200 bg-white text-slate-600 shadow-sm transition-colors hover:border-sky-200 hover:bg-sky-50 hover:text-sky-700"
      aria-label={label}
      title={label}
    >
      <Download className="h-3.5 w-3.5" aria-hidden />
    </button>
  );
}
