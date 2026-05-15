"use client";

import * as React from "react";
import { Loader2, Mail } from "lucide-react";
import { Alert } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { requestEmailCode, resetInstallationIdentityState, shouldResetInstallationIdentity, verifyEmailCode } from "@/lib/auth";
import { useAuth } from "@/components/auth/auth-provider";

export function LoginDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { refresh } = useAuth();
  const [step, setStep] = React.useState<"email" | "code">("email");
  const [email, setEmail] = React.useState("");
  const [code, setCode] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [message, setMessage] = React.useState("");
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setStep("email");
      setCode("");
      setMessage("");
      setError("");
    }
  }, [open]);

  async function sendCode() {
    const normalized = email.trim().toLowerCase();
    if (!normalized) return;
    setBusy(true);
    setError("");
    setMessage("");
    try {
      let data;
      try {
        data = await requestEmailCode(normalized);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        if (!shouldResetInstallationIdentity(msg)) throw err;
        resetInstallationIdentityState();
        data = await requestEmailCode(normalized);
      }
      setEmail(normalized);
      setStep("code");
      setMessage(`Verification code sent to ${data.maskedDestination || normalized}.`);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  async function verify() {
    if (!email.trim() || !code.trim()) return;
    setBusy(true);
    setError("");
    try {
      await verifyEmailCode(email.trim().toLowerCase(), code.trim());
      await refresh();
      onOpenChange(false);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Share Email Login</DialogTitle>
          <DialogDescription>Sign in with an email verification code.</DialogDescription>
        </DialogHeader>
        <div className="grid gap-4">
          <label className="grid gap-2 text-sm">
            <span className="mono-label text-muted-foreground">Email</span>
            <Input value={email} onChange={(event) => setEmail(event.target.value)} placeholder="email@example.com" type="email" />
          </label>
          {step === "code" ? (
            <label className="grid gap-2 text-sm">
              <span className="mono-label text-muted-foreground">Code</span>
              <Input value={code} onChange={(event) => setCode(event.target.value)} placeholder="123456" inputMode="numeric" />
            </label>
          ) : null}
          {message ? <Alert variant="success">{message}</Alert> : null}
          {error ? <Alert variant="destructive">{error}</Alert> : null}
        </div>
        <DialogFooter>
          {step === "email" ? (
            <Button onClick={sendCode} disabled={busy || !email.trim()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Mail className="h-4 w-4" />}
              Send Code
            </Button>
          ) : (
            <Button onClick={verify} disabled={busy || !code.trim()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              Verify
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
