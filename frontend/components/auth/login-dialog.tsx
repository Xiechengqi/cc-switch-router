"use client";

import * as React from "react";
import { Alert, Button, Form, Input, InputOTP, Modal, REGEXP_ONLY_DIGITS } from "@heroui/react";
import { Loader2, Mail } from "lucide-react";
import { AuthError, loginWithPassword, requestEmailCode, verifyEmailCode } from "@/lib/auth";
import { passwordErrorKey } from "@/lib/password-auth";
import { PasswordField } from "./password-field";
import type { AuthDeviceIdentity } from "@/lib/auth";
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

export function LoginDialog({ open, onOpenChange, resetPassword = false }: { open: boolean; onOpenChange: (open: boolean) => void; resetPassword?: boolean }) {
  const { refresh, emailLoginCompleted } = useAuth();
  const { t } = useLocaleText();
  const [step, setStep] = React.useState<"email" | "code">("email");
  const [mode, setMode] = React.useState<"email" | "password">("email");
  const [password, setPassword] = React.useState("");
  const [resetIntent, setResetIntent] = React.useState(false);
  const [unavailable, setUnavailable] = React.useState(false);
  const [email, setEmail] = React.useState("");
  const [code, setCode] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [resending, setResending] = React.useState(false);
  const [maskedDestination, setMaskedDestination] = React.useState("");
  const [challengeIdentity, setChallengeIdentity] = React.useState<AuthDeviceIdentity | null>(null);
  const [error, setError] = React.useState("");
  const sendingRef = React.useRef(false);
  const verifyingRef = React.useRef(false);

  React.useEffect(() => {
    setPassword("");
    if (open) {
      setMode("email");
      setResetIntent(resetPassword);
      setUnavailable(false);
      setStep("email");
      setCode("");
      setMaskedDestination("");
      setChallengeIdentity(null);
      setError("");
    }
  }, [open, resetPassword]);

  async function sendCode(options?: { resend?: boolean }) {
    const isResend = !!options?.resend;
    const source = isResend ? email : email.trim().toLowerCase();
    if (!source || sendingRef.current || verifyingRef.current) return;
    sendingRef.current = true;
    if (isResend) setResending(true);
    else setBusy(true);
    setError("");
    try {
      const data = await requestEmailCode(source);
      setEmail(source);
      setMaskedDestination(data.maskedDestination || fallbackMask(source));
      setChallengeIdentity(data.identity);
      setStep("code");
      if (isResend) setCode("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      sendingRef.current = false;
      if (isResend) setResending(false);
      else setBusy(false);
    }
  }

  async function verify(candidateCode: string) {
    const submittedCode = candidateCode.trim();
    if (!email.trim() || submittedCode.length < 6 || !challengeIdentity || verifyingRef.current || sendingRef.current) return;
    verifyingRef.current = true;
    setBusy(true);
    setError("");
    try {
      const result = await verifyEmailCode(email.trim().toLowerCase(), submittedCode, challengeIdentity);
      await refresh();
      onOpenChange(false);
      emailLoginCompleted(result, resetIntent);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      verifyingRef.current = false;
      setBusy(false);
    }
  }

  const isCodeStep = step === "code";
  const switchToEmail = (reset = false) => {
    setMode("email"); setStep("email"); setPassword(""); setError(""); setUnavailable(false); setResetIntent(reset);
  };
  async function passwordLogin() {
    if (verifyingRef.current || sendingRef.current || !email.trim() || !password) return;
    verifyingRef.current = true; setBusy(true); setError(""); setUnavailable(false);
    try {
      await loginWithPassword(email.trim().toLowerCase(), password);
      setPassword("");
      await refresh(); onOpenChange(false);
    } catch (err) {
      const key = passwordErrorKey(err instanceof AuthError ? err.code : undefined);
      setUnavailable(err instanceof AuthError && err.code === "password_unavailable");
      setError(key ? t(key) : err instanceof Error ? err.message : String(err));
      setPassword("");
    } finally { verifyingRef.current = false; setBusy(false); }
  }

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => { if (!busy && !resending) onOpenChange(next); }}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light !bg-white !text-slate-900">
            {!busy && !resending ? <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" /> : null}
            <Modal.Header>
              <div>
                <Modal.Heading className="!text-slate-900">{isCodeStep ? t("auth.verifyTitle") : t("auth.title")}</Modal.Heading>
                {isCodeStep ? (
                  <p className="mt-1 text-sm text-muted-foreground">
                    {t("auth.verifySubtitle", { destination: maskedDestination || fallbackMask(email) })}
                  </p>
                ) : null}
              </div>
            </Modal.Header>
            <Form
              className="grid gap-4"
              onSubmit={(event) => {
                event.preventDefault();
                if (mode === "password") {
                  if (!busy) void passwordLogin();
                } else if (!isCodeStep) {
                  if (!busy && email.trim()) sendCode().catch(console.error);
                } else if (!busy && code.trim().length >= 6) {
                  verify(code).catch(console.error);
                }
              }}
            >
              <Modal.Body className="grid gap-4 !text-slate-900">
                {!isCodeStep ? <div className="flex gap-1 rounded-lg bg-slate-100 p-1" aria-label={t("auth.title")}>
                  {(["email", "password"] as const).map((item) => <button key={item} type="button" disabled={busy}
                    aria-pressed={mode === item} className={`flex-1 rounded-md px-3 py-2 text-sm ${mode === item ? "bg-white font-medium shadow-sm" : "text-slate-500"}`}
                    onClick={() => { if (item === mode) return; if (item === "email") switchToEmail(); else { setMode(item); setError(""); setPassword(""); setResetIntent(false); } }}>
                    {t(item === "email" ? "auth.password.emailTab" : "auth.password.passwordTab")}
                  </button>)}
                </div> : null}
                {isCodeStep ? (
                  <div className="grid justify-items-center gap-3 pt-6">
                    <InputOTP
                      value={code}
                      onChange={(value) => {
                        setCode(value);
                        if (value.length === 6 && !busy) verify(value).catch(console.error);
                      }}
                      maxLength={6}
                      pattern={REGEXP_ONLY_DIGITS}
                      inputMode="numeric"
                      autoComplete="one-time-code"
                      aria-label={t("auth.code")}
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
                        setChallengeIdentity(null);
                        setError("");
                      }}
                      disabled={busy || resending}
                      className="text-xs text-muted-foreground underline-offset-4 transition-colors hover:text-foreground hover:underline disabled:cursor-not-allowed disabled:opacity-50"
                    >
                      {t("auth.useDifferentEmail")}
                    </button>
                  </div>
                ) : (
                  <Input
                    className="mt-3"
                    value={email}
                    onChange={(event) => setEmail(event.target.value)}
                    placeholder="email@example.com"
                    aria-label={t("auth.email")}
                    type="email"
                    name="email"
                    autoComplete="email"
                    disabled={busy || resending}
                    autoCapitalize="none"
                    spellCheck={false}
                    autoFocus
                  />
                )}
                {mode === "password" ? <>
                  <PasswordField label={t("auth.password.label")} value={password} onChange={setPassword} autoComplete="current-password" disabled={busy} />
                  <button type="button" disabled={busy} className="justify-self-start text-sm text-accent hover:underline" onClick={() => switchToEmail(true)}>{t("auth.password.forgot")}</button>
                </> : null}
                {error ? <Alert status="danger" className="!text-slate-900" role="alert">{error}</Alert> : null}
                {unavailable ? <button type="button" className="justify-self-start text-sm text-accent hover:underline" onClick={() => switchToEmail()}>{t("auth.password.useEmail")}</button> : null}
              </Modal.Body>
              <Modal.Footer>
                {mode === "password" ? <Button type="submit" variant="primary" isDisabled={busy || !email.trim() || !password}>
                  {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("auth.password.signIn")}
                </Button> : isCodeStep ? (
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
  );
}
