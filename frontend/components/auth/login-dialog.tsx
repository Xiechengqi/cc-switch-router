"use client";

import * as React from "react";
import { Alert, Button, Form, Input, InputOTP, Modal, REGEXP_ONLY_DIGITS } from "@heroui/react";
import { Loader2, Mail } from "lucide-react";
import { requestEmailCode, resetInstallationIdentityState, shouldResetInstallationIdentity, verifyEmailCode } from "@/lib/auth";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function LoginDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const { refresh } = useAuth();
  const { t } = useLocaleText();
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
      setMessage(t("auth.codeSent", { destination: data.maskedDestination || normalized }));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
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

  return (
    <Modal isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Backdrop>
        <Modal.Container placement="center">
          <Modal.Dialog>
            <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
            <Modal.Header>
              <div>
                <Modal.Heading>{t("auth.title")}</Modal.Heading>
                <p className="mt-1 text-sm text-muted-foreground">{t("auth.subtitle")}</p>
              </div>
            </Modal.Header>
            <Form
              className="grid gap-4"
              onSubmit={(event) => {
                event.preventDefault();
                if (step === "email") {
                  if (!busy && email.trim()) sendCode().catch(console.error);
                } else if (!busy && code.trim()) {
                  verify().catch(console.error);
                }
              }}
            >
              <Modal.Body className="grid gap-4">
                <label className="grid gap-2 text-sm">
                  <span className="mono-label text-muted-foreground">{t("auth.email")}</span>
                  <Input value={email} onChange={(event) => setEmail(event.target.value)} placeholder="email@example.com" type="email" />
                </label>
                {step === "code" ? (
                  <label className="grid gap-2 text-sm">
                    <span className="mono-label text-muted-foreground">{t("auth.code")}</span>
                    <InputOTP value={code} onChange={setCode} maxLength={6} pattern={REGEXP_ONLY_DIGITS} inputMode="numeric">
                      <InputOTP.Group>
                        {Array.from({ length: 6 }).map((_, index) => <InputOTP.Slot key={index} index={index} />)}
                      </InputOTP.Group>
                    </InputOTP>
                  </label>
                ) : null}
                {message ? <Alert status="success" className="!text-slate-900">{message}</Alert> : null}
                {error ? <Alert status="danger" className="!text-slate-900">{error}</Alert> : null}
              </Modal.Body>
              <Modal.Footer>
                {step === "email" ? (
                  <Button type="submit" variant="primary" isDisabled={busy || !email.trim()}>
                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Mail className="h-4 w-4" />}
                    {t("auth.sendCode")}
                  </Button>
                ) : (
                  <Button type="submit" variant="primary" isDisabled={busy || code.trim().length < 6}>
                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {t("common.verify")}
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
