"use client";

import { Button, Modal, ScrollShadow } from "@heroui/react";
import * as React from "react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAnnouncement } from "@/lib/api";
import {
  dismissAnnouncementPermanent,
  dismissAnnouncementToday,
  shouldShowAnnouncement,
} from "@/lib/announcement-dismiss";
import { sanitizeAnnouncementHtml } from "@/lib/announcement-html";
import type { AppLocale } from "@/lib/i18n";

const DIALOG_CLASS =
  "light z-[70] !bg-white !text-slate-900 " +
  "[--foreground:rgb(15,23,42)] [--muted:rgb(100,116,139)] [--overlay:#fff] [--overlay-foreground:rgb(15,23,42)] " +
  "[--surface:#fff] [--surface-foreground:rgb(15,23,42)]";

function sameRouterDomainClientRedirect(raw: string | null) {
  if (!raw || typeof window === "undefined") return null;
  try {
    const target = new URL(raw);
    const current = window.location;
    if (!["http:", "https:"].includes(target.protocol)) return null;
    if (target.hostname === current.hostname) return null;
    if (!target.hostname.endsWith(`.${current.hostname}`)) return null;
    return target.toString();
  } catch {
    return null;
  }
}

function pickAnnouncementContent(locale: AppLocale, contentEn: string, contentZhCn: string) {
  const primary = locale === "zh-CN" ? contentZhCn : contentEn;
  if (primary.trim()) return primary;
  return locale === "zh-CN" ? contentEn : contentZhCn;
}

export function AnnouncementDialog() {
  const { loading, session } = useAuth();
  const { locale, t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  const [revision, setRevision] = React.useState("");
  const [html, setHtml] = React.useState("");
  const evaluatedRevisionRef = React.useRef<string | null>(null);

  React.useEffect(() => {
    if (loading) return;
    const clientRedirect = sameRouterDomainClientRedirect(
      new URLSearchParams(window.location.search).get("clientRedirect"),
    );
    if (clientRedirect && !session?.authenticated) return;

    let cancelled = false;
    getAnnouncement()
      .then((data) => {
        if (cancelled) return;
        if (evaluatedRevisionRef.current === data.revision) return;
        evaluatedRevisionRef.current = data.revision;

        const content = pickAnnouncementContent(locale, data.contentEn, data.contentZhCn);
        if (!shouldShowAnnouncement(data.enabled, data.revision, content)) return;

        const sanitized = sanitizeAnnouncementHtml(content);
        if (!sanitized.trim()) return;

        setRevision(data.revision);
        setHtml(sanitized);
        setOpen(true);
      })
      .catch((error) => {
        console.error("announcement load failed", error);
      });

    return () => {
      cancelled = true;
    };
  }, [loading, locale, session?.authenticated]);

  const dismissToday = () => {
    if (revision) dismissAnnouncementToday(revision);
    setOpen(false);
  };

  const dismissPermanent = () => {
    if (revision) dismissAnnouncementPermanent(revision);
    setOpen(false);
  };

  return (
    <Modal isOpen={open} onOpenChange={setOpen}>
      <Modal.Backdrop className="z-[70]">
        <Modal.Container placement="center" className="z-[70]">
          <Modal.Dialog className={`${DIALOG_CLASS} w-[min(640px,calc(100vw-2rem))] max-w-none`}>
            <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200 hover:!text-slate-950" />
            <Modal.Body className="pt-2">
              <ScrollShadow className="max-h-[min(60vh,480px)] pr-1">
                <div
                  className="announcement-content text-sm leading-7 text-slate-800 [&_a]:text-primary [&_a]:underline [&_blockquote]:border-l-2 [&_blockquote]:border-slate-200 [&_blockquote]:pl-3 [&_h1]:text-xl [&_h1]:font-semibold [&_h2]:text-lg [&_h2]:font-semibold [&_h3]:text-base [&_h3]:font-semibold [&_li]:ml-4 [&_ol]:list-decimal [&_p+_p]:mt-3 [&_pre]:overflow-x-auto [&_pre]:rounded-md [&_pre]:bg-slate-100 [&_pre]:p-3 [&_ul]:list-disc"
                  dangerouslySetInnerHTML={{ __html: html }}
                />
              </ScrollShadow>
            </Modal.Body>
            <Modal.Footer className="justify-end gap-2">
              <Button variant="outline" onClick={dismissToday}>
                {t("announcement.dismissToday")}
              </Button>
              <Button variant="outline" onClick={dismissPermanent}>
                {t("announcement.dismissPermanent")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}
