"use client";

import { Loader2, RotateCcw, Save } from "lucide-react";
import { Alert, Button, Modal } from "@heroui/react";
import * as React from "react";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ShareEditReadView } from "@/components/dashboard/share-edit/share-edit-read-view";
import { ShareEditFormBody, useShareEditForm } from "@/components/dashboard/share-edit/share-edit-form";
import { ShareEditStatusBanner } from "@/components/dashboard/share-edit/share-edit-section";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { resolveShareCoreApp, shareAccessApps, SHARE_APP_LABELS } from "@/lib/share-app";
import type { DashboardMarket, ShareView } from "@/lib/types";

export { FieldGroup } from "@/components/dashboard/share-edit/share-edit-shared";

export function ShareEditDialog({
  share,
  markets,
  onClose,
  onSaved,
}: {
  share: ShareView | null;
  markets: DashboardMarket[];
  onClose: () => void;
  onSaved: (result: { appliedSynchronously: boolean }) => Promise<void>;
}) {
  const { t } = useLocaleText();
  const readOnly = !!share && !share.canManage;
  const form = useShareEditForm({ share, markets, t, onSaved, onClose });
  const editShare = share;
  const shareApp = editShare
    ? shareAccessApps(editShare)[0] ?? resolveShareCoreApp(editShare)
    : undefined;
  const shareAppLabel = shareApp ? SHARE_APP_LABELS[shareApp] : "";

  return (
    <>
      <Modal isOpen={!!share} onOpenChange={(open) => !open && !form?.busy && onClose()}>
        <Modal.Backdrop>
          <Modal.Container placement="center">
            <Modal.Dialog className="share-edit-surface light flex max-h-[min(88vh,calc(100vh-2rem))] w-[min(960px,calc(100vw-2rem))] max-w-none flex-col !bg-white !text-slate-900">
              <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
              <Modal.Header>
                <div className="pr-8">
                  <Modal.Heading>{readOnly ? t("dashboard.shareViewSettings") : t("dashboard.shareEditSettings")}</Modal.Heading>
                  <div className="mt-1 flex flex-wrap items-center gap-2 text-sm text-muted-foreground">
                    <span className="break-all">{share?.subdomain || share?.shareName}</span>
                    {shareAppLabel ? (
                      <span className="rounded bg-primary/10 px-2 py-0.5 text-[11px] font-semibold text-primary">
                        {shareAppLabel}
                      </span>
                    ) : null}
                  </div>
                  {share?.ownerEmail ? (
                    <p className="mt-1 text-xs text-muted-foreground">{share.ownerEmail}</p>
                  ) : null}
                </div>
              </Modal.Header>
              <Modal.Body className="min-h-0 flex-1 overflow-y-auto">
                {readOnly && share ? (
                  <ShareEditReadView share={share} markets={markets} t={t} />
                ) : share && form ? (
                  <div className="grid gap-6">
                    <ShareEditStatusBanner share={share} t={t} />
                    {form.error ? (
                      <Alert status="danger" className="!text-slate-900">
                        {form.error}
                      </Alert>
                    ) : null}
                    {form.notice ? (
                      <Alert status="success" className="!text-slate-900">
                        {form.notice}
                      </Alert>
                    ) : null}
                    <ShareEditFormBody share={share} t={t} form={form} />
                  </div>
                ) : null}
              </Modal.Body>
              <Modal.Footer className="sticky bottom-0 shrink-0 border-t border-slate-200 bg-white/95 backdrop-blur supports-[backdrop-filter]:bg-white/80">
                {readOnly ? (
                  <Button variant="outline" onClick={onClose} isDisabled={form?.busy}>
                    {t("common.close")}
                  </Button>
                ) : form ? (
                  <div className="flex w-full flex-wrap items-center justify-end gap-2">
                    {form.isDirty ? (
                      <Button
                        variant="ghost"
                        className="mr-auto text-muted-foreground"
                        onClick={form.resetDraft}
                        isDisabled={form.busy}
                      >
                        <RotateCcw className="h-4 w-4" />
                        {t("common.reset")}
                      </Button>
                    ) : null}
                    <Button variant="outline" onClick={onClose} isDisabled={form.busy}>
                      {t("common.cancel")}
                    </Button>
                    <Button
                      variant="primary"
                      onClick={form.save}
                      isDisabled={form.busy || form.formInvalid || !form.isDirty}
                    >
                      {form.busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                      {t("common.save")}
                    </Button>
                  </div>
                ) : null}
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>

      {form ? (
        <>
          <ConfirmAlertDialog
            open={form.confirmFreeOpen}
            title={t("dashboard.confirmFreeTitle")}
            description={t("dashboard.confirmFreeDesc")}
            confirmLabel={t("dashboard.confirmFree")}
            cancelLabel={t("common.cancel")}
            tone="danger"
            onConfirm={form.confirmFree}
            onOpenChange={(open) => !open && form.setConfirmFreeOpen(false)}
          />
          <ConfirmAlertDialog
            open={Boolean(form.transferTargetEmail)}
            title={t("dashboard.transferOwnerTitle")}
            description={t("dashboard.transferOwnerDesc", {
              target: form.transferTargetEmail || "-",
              owner: share?.ownerEmail || "-",
            })}
            confirmLabel={t("dashboard.transferOwnerConfirm")}
            cancelLabel={t("common.cancel")}
            tone="danger"
            onConfirm={form.transferOwner}
            onOpenChange={(open) => !open && form.setTransferTargetEmail("")}
          />
        </>
      ) : null}
    </>
  );
}
