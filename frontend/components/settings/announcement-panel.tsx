"use client";

import { Eye, Loader2, RotateCcw, Save } from "lucide-react";
import { Alert, Button, Card, Chip, Modal, ScrollShadow, Switch, TextArea } from "@heroui/react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAnnouncement, updateAnnouncement } from "@/lib/api";
import { sanitizeAnnouncementHtml } from "@/lib/announcement-html";
import type { AnnouncementResponse, AnnouncementSettingsUpdate } from "@/lib/types";

const DIALOG_CLASS =
  "light !bg-white !text-slate-900 " +
  "[--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)] [--overlay:#fff] [--overlay-foreground:rgb(15,23,42)] " +
  "[--surface:#fff] [--surface-foreground:rgb(15,23,42)]";

type AnnouncementDraft = {
  enabled: boolean;
  contentEn: string;
  contentZhCn: string;
  revision: string;
};

const EMPTY_DRAFT: AnnouncementDraft = {
  enabled: false,
  contentEn: "",
  contentZhCn: "",
  revision: "",
};

function toDraft(data: AnnouncementResponse): AnnouncementDraft {
  return {
    enabled: data.enabled,
    contentEn: data.contentEn,
    contentZhCn: data.contentZhCn,
    revision: data.revision,
  };
}

function sameDraft(a: AnnouncementDraft, b: AnnouncementDraft) {
  return (
    a.enabled === b.enabled
    && a.contentEn === b.contentEn
    && a.contentZhCn === b.contentZhCn
  );
}

export function AnnouncementPanel() {
  const { locale, t } = useLocaleText();
  const [saved, setSaved] = React.useState<AnnouncementDraft>(EMPTY_DRAFT);
  const [draft, setDraft] = React.useState<AnnouncementDraft>(EMPTY_DRAFT);
  const [loading, setLoading] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [success, setSuccess] = React.useState("");
  const [previewOpen, setPreviewOpen] = React.useState(false);

  const dirty = !sameDraft(draft, saved);
  const previewHtml = sanitizeAnnouncementHtml(
    locale === "zh-CN" ? draft.contentZhCn : draft.contentEn,
  );

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const next = toDraft(await getAnnouncement());
      setSaved(next);
      setDraft(next);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    load().catch(console.error);
  }, [load]);

  const save = async () => {
    setBusy(true);
    setError("");
    setSuccess("");
    try {
      const hasContent = Boolean(draft.contentEn.trim() || draft.contentZhCn.trim());
      const update: AnnouncementSettingsUpdate = {};
      if (draft.enabled !== saved.enabled) update.enabled = draft.enabled;
      if (draft.contentEn !== saved.contentEn) update.contentEn = draft.contentEn;
      if (draft.contentZhCn !== saved.contentZhCn) update.contentZhCn = draft.contentZhCn;
      if (hasContent && !draft.enabled && (update.contentEn !== undefined || update.contentZhCn !== undefined)) {
        update.enabled = true;
      }
      if (!Object.keys(update).length) return;
      const savedSettings = await updateAnnouncement(update);
      const next: AnnouncementDraft = {
        enabled: savedSettings.enabled,
        contentEn: savedSettings.contentEn,
        contentZhCn: savedSettings.contentZhCn,
        revision: savedSettings.updatedAt,
      };
      setSaved(next);
      setDraft(next);
      setSuccess(t("settings.announcementSaved"));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card className="rounded-lg">
      <Card.Header>
        <div className="flex flex-wrap items-center gap-2">
          <Card.Title>{t("settings.announcementTitle")}</Card.Title>
          {dirty ? <Chip color="accent" size="sm" variant="soft">{t("common.changed")}</Chip> : null}
        </div>
        <Card.Description>{t("settings.announcementDescription")}</Card.Description>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger">{error}</Alert> : null}
        {success ? <Alert status="success">{success}</Alert> : null}
        {loading ? (
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
            {t("settings.loading")}
          </div>
        ) : (
          <>
            <div className="flex items-center justify-between gap-3 rounded-md border bg-background p-3">
              <div>
                <div className="font-medium">{t("settings.announcementEnabled")}</div>
                <p className="mt-1 text-sm text-muted-foreground">{t("settings.announcementEnabledDescription")}</p>
              </div>
              <Switch
                isSelected={draft.enabled}
                onChange={(value) => setDraft((prev) => ({ ...prev, enabled: value }))}
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium" htmlFor="announcement-content-zh">
                {t("settings.announcementContentZh")}
              </label>
              <TextArea
                id="announcement-content-zh"
                className="min-h-40"
                value={draft.contentZhCn}
                onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) =>
                  setDraft((prev) => ({ ...prev, contentZhCn: event.target.value }))}
                placeholder={t("settings.announcementHtmlPlaceholder")}
              />
            </div>
            <div className="grid gap-2">
              <label className="text-sm font-medium" htmlFor="announcement-content-en">
                {t("settings.announcementContentEn")}
              </label>
              <TextArea
                id="announcement-content-en"
                className="min-h-40"
                value={draft.contentEn}
                onChange={(event: React.ChangeEvent<HTMLTextAreaElement>) =>
                  setDraft((prev) => ({ ...prev, contentEn: event.target.value }))}
                placeholder={t("settings.announcementHtmlPlaceholder")}
              />
            </div>
            <div className="flex flex-wrap justify-end gap-2">
              <Button variant="outline" onClick={() => load()} isDisabled={busy || loading}>
                <RotateCcw className="h-4 w-4" />
                {t("common.reload")}
              </Button>
              <Button variant="outline" onClick={() => setPreviewOpen(true)} isDisabled={!previewHtml.trim()}>
                <Eye className="h-4 w-4" />
                {t("settings.announcementPreview")}
              </Button>
              <Button variant="primary" onClick={save} isDisabled={busy || !dirty}>
                {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Save className="h-4 w-4" />}
                {dirty ? t("common.saveWithCount", { count: 1 }) : t("common.save")}
              </Button>
            </div>
          </>
        )}
      </Card.Content>

      <Modal isOpen={previewOpen} onOpenChange={setPreviewOpen}>
        <Modal.Backdrop>
          <Modal.Container placement="center">
            <Modal.Dialog className={`${DIALOG_CLASS} w-[min(640px,calc(100vw-2rem))] max-w-none`}>
              <Modal.Header>
                <Modal.Heading>{t("settings.announcementPreview")}</Modal.Heading>
              </Modal.Header>
              <Modal.Body>
                <ScrollShadow className="max-h-[min(60vh,480px)] pr-1">
                  <div
                    className="announcement-content text-sm leading-7 text-slate-800 [&_a]:text-primary [&_a]:underline [&_blockquote]:border-l-2 [&_blockquote]:border-slate-200 [&_blockquote]:pl-3 [&_h1]:text-xl [&_h1]:font-semibold [&_h2]:text-lg [&_h2]:font-semibold [&_h3]:text-base [&_h3]:font-semibold [&_li]:ml-4 [&_ol]:list-decimal [&_p+_p]:mt-3 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-slate-100 [&_pre]:p-3 [&_ul]:list-disc"
                    dangerouslySetInnerHTML={{ __html: previewHtml }}
                  />
                </ScrollShadow>
              </Modal.Body>
              <Modal.Footer>
                <Button variant="outline" onClick={() => setPreviewOpen(false)}>
                  {t("common.close")}
                </Button>
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>
    </Card>
  );
}
