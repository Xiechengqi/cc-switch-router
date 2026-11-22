"use client";

/* Password confirmation for risky wallet operations. The signed fetch layer
 * asks for the password through `lib/financial-confirm`; this host renders one
 * dialog at a time and never stores the password. */

import * as React from "react";
import { Button, Form, Modal } from "@heroui/react";
import { LockKeyhole } from "lucide-react";
import { PasswordField } from "./password-field";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_PRIMARY_BUTTON } from "@/components/market/wallet-primitives";
import { registerFinancialConfirmHost, type FinancialPasswordRequest } from "@/lib/financial-confirm";
import type { MessageKey } from "@/lib/i18n";

type Pending = FinancialPasswordRequest & { resolve: (password: string | null) => void };

export function FinancialConfirmDialog({ onPasswordRequired }: { onPasswordRequired: () => void }) {
  const { t } = useLocaleText();
  const [pending, setPending] = React.useState<Pending | null>(null);
  const [password, setPassword] = React.useState("");
  const pendingRef = React.useRef<Pending | null>(null);
  const requiredRef = React.useRef(onPasswordRequired);
  requiredRef.current = onPasswordRequired;

  React.useEffect(() => registerFinancialConfirmHost({
    askPassword: (request) => new Promise<string | null>((resolve) => {
      // A newer request replaces an unanswered one, which counts as cancelled.
      pendingRef.current?.resolve(null);
      const next = { ...request, resolve };
      pendingRef.current = next;
      setPassword("");
      setPending(next);
    }),
    requirePassword: () => requiredRef.current(),
  }), []);

  React.useEffect(() => () => pendingRef.current?.resolve(null), []);

  const finish = (value: string | null) => {
    const current = pendingRef.current;
    pendingRef.current = null;
    setPending(null);
    setPassword("");
    current?.resolve(value);
  };

  const errorKey = pending?.error ? (`walletError.${pending.error}` as MessageKey) : null;
  return (
    <Modal.Backdrop isOpen={!!pending} onOpenChange={(open) => { if (!open) finish(null); }}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light !bg-white !text-slate-900 sm:max-w-md">
          <Modal.CloseTrigger />
          <Modal.Header>
            <Modal.Heading className="flex items-center gap-2">
              <LockKeyhole className="size-4 text-accent" aria-hidden />
              {t("financialConfirm.title")}
            </Modal.Heading>
          </Modal.Header>
          <Form
            className="grid gap-4"
            onSubmit={(event) => {
              event.preventDefault();
              if (password) finish(password);
            }}
          >
            <Modal.Body className="grid gap-4">
              <p className="text-sm leading-6 text-slate-600">
                {t(pending?.risk === "immediate" ? "financialConfirm.body.immediate" : "financialConfirm.body.confirm")}
              </p>
              <PasswordField
                label={t("financialConfirm.password")}
                value={password}
                onChange={setPassword}
                autoComplete="current-password"
                autoFocus
              />
              {errorKey ? <p role="alert" className="text-sm text-destructive">{t(errorKey)}</p> : null}
              <p className="text-xs text-slate-500">{t("financialConfirm.forgot")}</p>
            </Modal.Body>
            <Modal.Footer className="flex flex-col-reverse gap-2 sm:flex-row sm:justify-end">
              <Button variant="ghost" className="min-h-11" onPress={() => finish(null)}>{t("common.cancel")}</Button>
              <button type="submit" className={WALLET_PRIMARY_BUTTON} disabled={!password}>
                {t("financialConfirm.submit")}
              </button>
            </Modal.Footer>
          </Form>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
