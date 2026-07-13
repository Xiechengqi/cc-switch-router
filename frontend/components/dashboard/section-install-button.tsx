"use client";

import { Button } from "@heroui/react";

export function SectionInstallButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <Button
      type="button"
      size="sm"
      variant="outline"
      className="h-7 shrink-0 border-slate-200 bg-white px-2.5 text-[11px] font-medium normal-case tracking-normal text-foreground shadow-sm hover:bg-slate-50"
      onClick={onClick}
    >
      {label}
    </Button>
  );
}
