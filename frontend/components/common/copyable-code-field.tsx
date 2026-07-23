"use client";

import { Button } from "@heroui/react";
import { Check, Copy } from "lucide-react";
import * as React from "react";

export function CopyableCodeField({
  label,
  value,
  copyLabel,
  copiedLabel,
}: {
  label: string;
  value: string;
  copyLabel: string;
  copiedLabel: string;
}) {
  const [copied, setCopied] = React.useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="rounded-lg border bg-slate-50 p-3">
      <div className="mb-2 font-mono text-[10px] uppercase text-muted-foreground">{label}</div>
      <div className="flex items-start gap-2">
        <pre className="min-w-0 flex-1 overflow-x-auto whitespace-pre-wrap break-all font-mono text-[12px] leading-6 text-foreground">
          {value}
        </pre>
        <Button
          variant="outline"
          size="sm"
          isIconOnly
          className="h-8 w-8 min-w-8 rounded-md p-0"
          aria-label={copied ? copiedLabel : copyLabel}
          onClick={() => void copy()}
        >
          {copied ? <Check className="h-4 w-4" /> : <Copy className="h-4 w-4" />}
        </Button>
      </div>
    </div>
  );
}
