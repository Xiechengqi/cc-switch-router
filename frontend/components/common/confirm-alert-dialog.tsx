"use client";

import { AlertDialog, Button } from "@heroui/react";
import { Loader2 } from "lucide-react";
import * as React from "react";

const DIALOG_CLASS =
  "light !bg-white !text-slate-900 " +
  "[--foreground:rgb(var(--router-foreground))] [--muted:rgb(var(--router-muted-foreground))] " +
  "[--overlay:#fff] [--overlay-foreground:rgb(var(--router-foreground))] " +
  "[--surface:#fff] [--surface-foreground:rgb(var(--router-foreground))] " +
  "[--surface-secondary:rgb(var(--router-muted))] [--surface-secondary-foreground:rgb(var(--router-foreground))] " +
  "[--default:rgb(var(--router-muted))] [--default-foreground:rgb(var(--router-foreground))]";

type ConfirmTone = "primary" | "danger" | "warning";

export function ConfirmAlertDialog({
  open,
  title,
  description,
  confirmLabel,
  cancelLabel,
  busy = false,
  tone = "primary",
  onConfirm,
  onOpenChange,
}: {
  open: boolean;
  title: React.ReactNode;
  description: React.ReactNode;
  confirmLabel: React.ReactNode;
  cancelLabel: React.ReactNode;
  busy?: boolean;
  tone?: ConfirmTone;
  onConfirm: () => void;
  onOpenChange: (open: boolean) => void;
}) {
  const confirmClassName =
    tone === "danger"
      ? "!bg-destructive !text-destructive-foreground hover:brightness-110"
      : tone === "warning"
        ? "!border-amber-300 !bg-amber-500 !text-white hover:!bg-amber-600"
        : undefined;
  const confirmVariant = tone === "primary" ? "primary" : tone === "danger" ? "danger" : "outline";

  return (
    <AlertDialog isOpen={open} onOpenChange={onOpenChange}>
      <AlertDialog.Backdrop>
        <AlertDialog.Container>
          <AlertDialog.Dialog className={DIALOG_CLASS}>
            <AlertDialog.Header>
              <AlertDialog.Heading className="!text-slate-900">{title}</AlertDialog.Heading>
            </AlertDialog.Header>
            <AlertDialog.Body>
              <div className="text-sm leading-6 !text-slate-600">{description}</div>
            </AlertDialog.Body>
            <AlertDialog.Footer>
              <Button
                variant="ghost"
                className="!text-slate-700 hover:!bg-slate-100"
                isDisabled={busy}
                onClick={() => onOpenChange(false)}
              >
                {cancelLabel}
              </Button>
              <Button
                variant={confirmVariant}
                className={confirmClassName}
                isDisabled={busy}
                onClick={onConfirm}
              >
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {confirmLabel}
              </Button>
            </AlertDialog.Footer>
          </AlertDialog.Dialog>
        </AlertDialog.Container>
      </AlertDialog.Backdrop>
    </AlertDialog>
  );
}
