"use client";

import { Button, toast } from "@heroui/react";
import { ArrowLeft, Copy, ExternalLink, Globe2, Lock, X } from "lucide-react";
import { usePathname, useRouter } from "next/navigation";
import { chatRoomUrl, maskEmail } from "@/components/chat/client-chat-helpers";
import { useDashboardFocus } from "@/components/dashboard/dashboard-focus";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { DASHBOARD_CLIENTS_PATH, isClientsRoute } from "@/lib/dashboard-nav";
import type { ClientChatRoom, DashboardClient } from "@/lib/types";

export function ClientChatRoomHeader({
  room,
  client,
  onBack,
  onClose,
}: {
  room: ClientChatRoom;
  client?: DashboardClient;
  onBack: () => void;
  onClose: () => void;
}) {
  const { t } = useLocaleText();
  const router = useRouter();
  const pathname = usePathname() || "/clients/";
  const focus = useDashboardFocus();

  async function copyLink() {
    try {
      await navigator.clipboard.writeText(chatRoomUrl(room.installationId));
      toast.success(t("chat.linkCopied"));
    } catch {
      toast.danger(t("common.copyFailed"));
    }
  }

  function viewClient() {
    if (!isClientsRoute(pathname)) router.push(DASHBOARD_CLIENTS_PATH);
    focus.setFocus({ kind: "client", id: room.installationId, source: "client-board" });
    focus.openDrawer("client", room.installationId);
  }

  const ownerEmail = client?.installation.ownerEmail || client?.clientTunnel?.ownerEmail;
  const onlineRate = client?.onlineRate24h;

  return (
    <header className="shrink-0 border-b border-slate-200 px-3 py-2.5">
      <div className="flex items-start gap-2">
        <Button isIconOnly variant="ghost" size="sm" className="mt-0.5 rounded-md" onClick={onBack} aria-label={t("chat.back")}>
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <div className="min-w-0 flex-1">
          <h2 id="client-chat-title" className="truncate text-sm font-semibold text-slate-900">
            {room.clientLabel}
          </h2>
          <div className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-[11px] text-slate-500">
            {room.status === "archived" ? (
              <span className="inline-flex items-center gap-1 text-amber-700">
                <Lock className="h-3 w-3" />
                {t("chat.archived")}
              </span>
            ) : (
              <span className="inline-flex items-center gap-1">
                <Globe2 className="h-3 w-3" />
                {t("chat.public")}
              </span>
            )}
            {ownerEmail ? (
              <span>
                {t("chat.owner")}: {maskEmail(ownerEmail)}
              </span>
            ) : null}
            {typeof onlineRate === "number" ? (
              <span>
                {t("chat.online")}: {onlineRate.toFixed(1)}%
              </span>
            ) : null}
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button isIconOnly variant="ghost" size="sm" className="rounded-md" onClick={() => void copyLink()} aria-label={t("chat.copyLink")}>
            <Copy className="h-3.5 w-3.5" />
          </Button>
          <Button isIconOnly variant="ghost" size="sm" className="rounded-md" onClick={viewClient} aria-label={t("chat.viewClient")}>
            <ExternalLink className="h-3.5 w-3.5" />
          </Button>
          <Button isIconOnly variant="ghost" size="sm" className="rounded-md" onClick={onClose} aria-label={t("chat.close")}>
            <X className="h-4 w-4" />
          </Button>
        </div>
      </div>
    </header>
  );
}
