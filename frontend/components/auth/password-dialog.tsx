"use client";
import * as React from "react";
import { Button, Form, Modal } from "@heroui/react";
import { Check, Loader2 } from "lucide-react";
import { useAuth } from "./auth-provider";
import { PasswordField } from "./password-field";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { AuthError, updateLoginPassword } from "@/lib/auth";
import { passwordErrorKey, validLoginPassword } from "@/lib/password-auth";

export function PasswordDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const { session, refresh } = useAuth();
  const { t } = useLocaleText();
  const [password, setPassword] = React.useState("");
  const [confirm, setConfirm] = React.useState("");
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [saved, setSaved] = React.useState(false);
  const submitting = React.useRef(false);
  const target = React.useRef({ open, userId: session?.user?.id });
  target.current = { open, userId: session?.user?.id };
  const state = session?.password;
  React.useEffect(() => {
    setPassword("");
    setConfirm("");
    setError("");
    setSaved(false);
  }, [open, session?.user?.id]);
  async function save() {
    if (submitting.current || !state) return;
    if (!validLoginPassword(password)) {
      setError(t("auth.password.policy"));
      return;
    }
    if (password !== confirm) {
      setError(t("auth.password.mismatch"));
      return;
    }
    submitting.current = true;
    setBusy(true);
    setError("");
    const userId = session?.user?.id;
    const stillCurrent = () =>
      target.current.open && target.current.userId === userId;
    try {
      await updateLoginPassword(password, state.version);
      if (!stillCurrent()) return;
      setPassword("");
      setConfirm("");
      setSaved(true);
      await refresh();
    } catch (err) {
      if (!stillCurrent()) return;
      const key = passwordErrorKey(
        err instanceof AuthError ? err.code : undefined,
      );
      setError(key ? t(key) : err instanceof Error ? err.message : String(err));
      if (err instanceof AuthError && err.code === "password_changed")
        void refresh();
    } finally {
      submitting.current = false;
      setBusy(false);
    }
  }
  return (
    <Modal.Backdrop
      isOpen={open}
      onOpenChange={(next) => {
        if (!busy) onOpenChange(next);
      }}
    >
      <Modal.Container placement="center">
        <Modal.Dialog className="light !bg-white !text-slate-900">
          {!busy ? <Modal.CloseTrigger /> : null}
          <Modal.Header>
            <Modal.Heading>
              {t(
                saved
                  ? "auth.password.saved"
                  : state?.hasPassword
                    ? "auth.password.changeTitle"
                    : "auth.password.setTitle",
              )}
            </Modal.Heading>
          </Modal.Header>
          <Form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <Modal.Body className="grid gap-4">
              {saved ? (
                <p
                  className="flex items-center gap-2 text-sm text-emerald-700"
                  role="status"
                >
                  <Check className="h-4 w-4" />
                  {t("auth.password.savedHint")}
                </p>
              ) : (
                <>
                  <p className="text-sm text-slate-500">
                    {t(
                      state?.hasPassword
                        ? "auth.password.changeHint"
                        : "auth.password.setupHint",
                    )}
                  </p>
                  <p className="text-sm text-slate-500">
                    {t("auth.password.emailNotice")}
                  </p>
                  <PasswordField
                    label={t("auth.password.new")}
                    value={password}
                    onChange={setPassword}
                    autoFocus
                    disabled={busy}
                  />
                  <PasswordField
                    label={t("auth.password.confirm")}
                    value={confirm}
                    onChange={setConfirm}
                    disabled={busy}
                  />
                  <p className="text-xs text-slate-500">
                    {t("auth.password.policy")}
                  </p>
                </>
              )}
              {error ? (
                <p className="text-sm text-red-600" role="alert">
                  {error}
                </p>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button
                type="button"
                variant="ghost"
                isDisabled={busy}
                onClick={() => onOpenChange(false)}
              >
                {t(
                  saved
                    ? "common.close"
                    : state?.hasPassword
                      ? "common.cancel"
                      : "auth.password.later",
                )}
              </Button>
              {!saved ? (
                <Button
                  type="submit"
                  variant="primary"
                  isDisabled={busy || !state || !password || !confirm}
                >
                  {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("auth.password.save")}
                </Button>
              ) : null}
            </Modal.Footer>
          </Form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
