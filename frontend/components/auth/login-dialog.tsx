"use client";

import * as React from "react";
import { Alert, Button, Form, Input, InputOTP, Modal, REGEXP_ONLY_DIGITS } from "@heroui/react";
import { Loader2, Mail } from "lucide-react";
import { requestEmailCode, resetInstallationIdentityState, shouldResetInstallationIdentity, verifyEmailCode } from "@/lib/auth";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";

function fallbackMask(email: string) {
  const trimmed = email.trim();
  const at = trimmed.indexOf("@");
  if (at <= 0) return trimmed;
  const local = trimmed.slice(0, at);
  const domain = trimmed.slice(at);
  if (local.length <= 1) return `${local}***${domain}`;
  return `${local[0]}${"*".repeat(Math.max(3, local.length - 1))}${domain}`;
}

export function LoginDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { refresh } = useAuth();
  const { t } = useLocaleText();
  const [step, setStep] = React.useState<"email" | "code">("email");
  const [email, setEmail] = React.useState("");
  const [code, setCode] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [resending, setResending] = React.useState(false);
  const [maskedDestination, setMaskedDestination] = React.useState("");
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (open) {
      setStep("email");
      setCode("");
      setMaskedDestination("");
      setError("");
    }
  }, [open]);

  async function sendCode(options?: { resend?: boolean }) {
    const isResend = !!options?.resend;
    const source = isResend ? email : email.trim().toLowerCase();
    if (!source) return;
    if (isResend) setResending(true);
    else setBusy(true);
    setError("");
    try {
      let data;
      try {
        data = await requestEmailCode(source);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        if (!shouldResetInstallationIdentity(msg)) throw err;
        resetInstallationIdentityState();
        data = await requestEmailCode(source);
      }
      setEmail(source);
      setMaskedDestination(data.maskedDestination || fallbackMask(source));
      setStep("code");
      if (isResend) setCode("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      if (isResend) setResending(false);
      else setBusy(false);
    }
  }

  async function verify() {
    if (!email.trim() || code.trim().length < 6) return;
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

  const isCodeStep = step === "code";

  return (
    <Modal isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Backdrop>
        <Modal.Container placement="center">
          <Modal.Dialog>
            <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
            <Modal.Header>
              <div>
                <Modal.Heading>{isCodeStep ? t("auth.verifyTitle") : t("auth.title")}</Modal.Heading>
                <p className="mt-1 text-sm text-muted-foreground">
                  {isCodeStep
                    ? t("auth.verifySubtitle", { destination: maskedDestination || fallbackMask(email) })
                    : t("auth.subtitle")}
                </p>
              </div>
            </Modal.Header>
            <Form
              className="grid gap-4"
              onSubmit={(event) => {
                event.preventDefault();
                if (!isCodeStep) {
                  if (!busy && email.trim()) sendCode().catch(console.error);
                } else if (!busy && code.trim().length >= 6) {
                  verify().catch(console.error);
                }
              }}
            >
              <Modal.Body className="grid gap-4">
                {isCodeStep ? (
                  <div className="grid justify-items-center gap-3 pt-6">
                    <InputOTP
                      value={code}
                      onChange={(value) => {
                        setCode(value);
                        if (value.length === 6 && !busy) verify().catch(console.error);
                      }}
                      maxLength={6}
                      pattern={REGEXP_ONLY_DIGITS}
                      inputMode="numeric"
                      autoFocus
                    >
                      <InputOTP.Group>
                        {Array.from({ length: 6 }).map((_, index) => <InputOTP.Slot key={index} index={index} />)}
                      </InputOTP.Group>
                    </InputOTP>
                    <p className="text-xs text-muted-foreground">
                      {t("auth.noCode")}{" "}
                      <button
                        type="button"
                        onClick={() => sendCode({ resend: true }).catch(console.error)}
                        disabled={busy || resending}
                        className="font-medium text-foreground underline decoration-dotted underline-offset-4 transition-colors hover:text-primary disabled:cursor-not-allowed disabled:opacity-50"
                      >
                        {resending ? t("auth.resending") : t("auth.resend")}
                      </button>
                    </p>
                    <button
                      type="button"
                      onClick={() => {
                        setStep("email");
                        setCode("");
                        setError("");
                      }}
                      disabled={busy || resending}
                      className="text-xs text-muted-foreground underline-offset-4 transition-colors hover:text-foreground hover:underline disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {t("auth.useDifferentEmail")}
                    </button>
                  </div>
                ) : (
                  <label className="grid gap-2 text-sm">
                    <span className="mono-label text-muted-foreground">{t("auth.email")}</span>
                    <Input value={email} onChange={(event) => setEmail(event.target.value)} placeholder="email@example.com" type="email" autoFocus />
                  </label>
                )}
                {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
              </Modal.Body>
              <Modal.Footer>
                {isCodeStep ? (
                  <Button type="submit" variant="primary" isDisabled={busy || code.trim().length < 6}>
                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {t("common.verify")}
                  </Button>
                ) : (
                  <Button type="submit" variant="primary" isDisabled={busy || !email.trim()}>
                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Mail className="h-4 w-4" />}
                    {t("auth.sendCode")}
                  </Button>
                )}
              </Modal.Footer>
            </Form>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}
