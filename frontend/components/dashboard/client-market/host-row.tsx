"use client";

import * as React from "react";
import { createPortal } from "react-dom";
import { Button, Checkbox, Chip, Dropdown, Modal, toast } from "@heroui/react";
import { Fingerprint, Loader2, MessageCircle, MoreHorizontal, Pencil, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientChat } from "@/components/chat/client-chat";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CountryFlag } from "@/components/common/country-flag";
import { ProvisionJobLog } from "@/components/dashboard/provision-job-log";
import { WebTerminalGlyph } from "@/components/dashboard/web-terminal/web-terminal-glyph";
import { useWebTerminal } from "@/components/dashboard/web-terminal";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { HostOfferDialog } from "@/components/dashboard/client-market/host-offer-dialog";
import { HostKeyRotationDialog } from "@/components/dashboard/client-market/host-key-rotation-dialog";
import {
  ClientMarketRentalBanner,
  clientMarketRentalHasBanner,
} from "@/components/dashboard/client-market-rental-banner";
import { ProviderContactButton } from "@/components/common/provider-contacts";
import { ReleaseRentalAction } from "@/components/dashboard/client-market/release-rental-action";
import {
  cleanupClientMarketProviderRental,
  deleteClientMarketHost,
  getClientMarketJob,
  reverifyClientMarketHost,
} from "@/lib/api";
import { mergeHosts } from "@/lib/client-market-refresh";
import type { ClientMarketHost, ClientMarketRental, ProvisioningJob } from "@/lib/types";
import {
  cleanupFailureGuidanceKey,
  cleanupPhaseLabelKey,
  cleanupReasonForHost,
  fineStatusHintKey,
  formatHostIpIntelSecondary,
  formatHostIpLocation,
  formatHostOffer,
  hostCanCleanup,
  hostCanClientRelease,
  hostCanDelete,
  hostCanManage,
  hostCanReverify,
  hostDisplayLabel,
  hostStatusGuidanceKey,
  statusLabelKey,
} from "@/components/dashboard/client-market/host-utils";

function HostRowImpl({
  host,
  rental,
  highlighted = false,
  selectionMode,
  selected,
  onSelectedChange,
  selectionDisabled,
  onChanged,
  onCreate,
  onUiBusyChange,
}: {
  host: ClientMarketHost;
  rental?: ClientMarketRental | null;
  highlighted?: boolean;
  selectionMode: boolean;
  selected: boolean;
  /** Takes the host id so the parent can pass one stable callback to every row
   *  instead of allocating a fresh closure per row on every render. */
  onSelectedChange: (hostId: string, selected: boolean) => void;
  selectionDisabled?: boolean;
  onChanged: () => void;
  onCreate: (host: ClientMarketHost) => void;
  onUiBusyChange?: (hostId: string, busy: boolean) => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const { openChat, unreadByInstallation } = useClientChat();
  const viewerEmail = session?.user?.email;
  const { openTerminal } = useWebTerminal();
  const [busy, setBusy] = React.useState(false);
  const [confirmAction, setConfirmAction] = React.useState<"delete" | "cleanup" | null>(null);
  const [denyRenterAccess, setDenyRenterAccess] = React.useState(false);
  const [cleanupJob, setCleanupJob] = React.useState<ProvisioningJob | null>(null);
  const [cleanupOpen, setCleanupOpen] = React.useState(false);
  const [offerOpen, setOfferOpen] = React.useState(false);
  const [hostKeyOpen, setHostKeyOpen] = React.useState(false);

  React.useEffect(() => {
    const locked = offerOpen || hostKeyOpen || cleanupOpen || confirmAction != null || busy;
    onUiBusyChange?.(host.id, locked);
    return () => onUiBusyChange?.(host.id, false);
  }, [busy, cleanupOpen, confirmAction, host.id, hostKeyOpen, offerOpen, onUiBusyChange]);
  const canManageHost = hostCanManage(host, viewerEmail);
  const canDelete = hostCanDelete(host, viewerEmail);
  const isClientOwner = host.isClientOwner === true;
  const canCleanup = hostCanCleanup(host, viewerEmail);
  const canClientRelease = hostCanClientRelease(host, rental);
  const showRenterRental = clientMarketRentalHasBanner(rental);
  const isRetryCleanup =
    canCleanup && (host.status === "unreachable" || host.status === "draining");
  const canReverify = hostCanReverify(host, viewerEmail);
  const canOpenTerminal = host.canWebTerminal === true || canManageHost;
  const hostLabel = hostDisplayLabel(host);
  const terminalTitle = host.ip || hostLabel;
  const countryName = host.countryCode
    ? new Intl.DisplayNames([locale], { type: "region" }).of(host.countryCode) || host.countryCode
    : "";

  const pollCleanupJob = async (jobId: string) => {
    let latest: ProvisioningJob | null = null;
    for (let i = 0; i < 180; i++) {
      await new Promise((r) => setTimeout(r, 1200));
      try {
        latest = await getClientMarketJob(jobId);
      } catch {
        continue;
      }
      setCleanupJob(latest);
      if (latest.status === "succeeded") {
        toast.success(t("clientMarket.cleanupSucceeded"));
        onChanged();
        return;
      }
      if (latest.status === "failed") {
        const detail = latest.failureCode || latest.log.split("\n").filter(Boolean).at(-1) || "";
        toast.danger(
          detail
            ? `${t("clientMarket.cleanupFailed")}: ${detail}`
            : t("clientMarket.cleanupFailed"),
        );
        onChanged();
        return;
      }
    }
    toast.danger(t("clientMarket.cleanupTimedOut"));
    onChanged();
  };

  const onDelete = async () => {
    setConfirmAction(null);
    setBusy(true);
    try {
      await deleteClientMarketHost(host.id);
      onChanged();
    } catch (err) {
      toast.danger(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const onCleanup = async (denyClientAccess = false) => {
    if (!host.installationId) return;
    setConfirmAction(null);
    setBusy(true);
    setCleanupJob(null);
    setCleanupOpen(true);
    try {
      const { jobId } = await cleanupClientMarketProviderRental(host.installationId, {
        reason: cleanupReasonForHost(host),
        denyClientAccess,
      });
      toast.info(t("clientMarket.cleanupStarted"));
      const initial = await getClientMarketJob(jobId).catch(() => null);
      if (initial) setCleanupJob(initial);
      await pollCleanupJob(jobId);
    } catch (err) {
      toast.danger(err instanceof Error ? err.message : String(err));
      setCleanupOpen(false);
    } finally {
      setBusy(false);
    }
  };

  const onReverify = async () => {
    setBusy(true);
    try {
      await reverifyClientMarketHost(host.id);
      toast.success(t("clientMarket.hostReverified"));
      onChanged();
    } catch (err) {
      toast.danger(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const confirmCopy = confirmAction === "cleanup"
    ? {
        title: t(isRetryCleanup ? "clientMarket.retryCleanupConfirmTitle" : "clientMarket.cleanupConfirmTitle"),
        description: t(
          isRetryCleanup ? "clientMarket.retryCleanupConfirmDesc" : "clientMarket.cleanupConfirmDesc",
          { host: hostLabel },
        ),
        confirmLabel: t(isRetryCleanup ? "clientMarket.retryCleanup" : "clientMarket.cleanup"),
      }
    : confirmAction === "delete"
      ? {
          title: t("clientMarket.deleteHostConfirmTitle"),
          description: t("clientMarket.deleteHostConfirmDesc", { host: hostLabel }),
          confirmLabel: t("clientMarket.deleteHost"),
        }
      : null;
  const hasActions = canManageHost || canDelete || canCleanup || canReverify;
  const ipPort = host.ip ? `${host.ip}${host.port ? `:${host.port}` : ""}` : "";
  const intel = host.ipIntel;
  const locationLabel = formatHostIpLocation(intel, countryName, locale);
  const secondaryIntelParts = formatHostIpIntelSecondary(intel, t);
  const subdomain = host.clientSubdomain?.trim() || "";
  const cleanupPhase = cleanupJob?.phase || "";
  const cleanupTone =
    cleanupJob?.status === "failed" ? "failed" : cleanupJob?.status === "succeeded" ? "success" : "running";
  const noteText = host.note?.trim() || "";
  const chatUnread = host.installationId
    ? unreadByInstallation.get(host.installationId) || 0
    : 0;
  /** Notes only — rental lifecycle lives under the offer cell to avoid interstitial table rows. */
  const showSubrow = !!noteText;
  const ipIntelSubtitle = secondaryIntelParts.length ? secondaryIntelParts.join(" · ") : "";
  const statusGuidanceKey = hostStatusGuidanceKey(host.status, host.lastError);
  const statusGuidanceSubtitle = statusGuidanceKey ? t(statusGuidanceKey) : "";
  const statusHint = fineStatusHintKey(host.status) ? t(fineStatusHintKey(host.status)!) : "";
  const colSpan = selectionMode ? 8 : 7;

  return (
    <>
      <tr
        id={host.installationId ? `client-market-host-${host.installationId}` : undefined}
        className={`border-b border-border/80 transition-colors hover:bg-muted/40 ${
          highlighted ? "bg-accent/[0.06] ring-2 ring-inset ring-accent/40" : ""
        }`}
      >
        {selectionMode ? (
          <td className="w-10 px-2 py-2 align-middle">
            <Checkbox
              isSelected={selected}
              onChange={(next) => onSelectedChange(host.id, next)}
              isDisabled={selectionDisabled}
              aria-label={t("clientMarket.selectHost", { host: host.ip ?? host.id })}
              className="shrink-0"
            >
              <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                <Checkbox.Indicator />
              </Checkbox.Control>
            </Checkbox>
          </td>
        ) : null}
        <td className="max-w-[11rem] px-2 py-2 align-middle">
          <div className="min-w-0" title={statusGuidanceSubtitle || statusHint || undefined}>
            <Chip size="sm" variant="soft" className="shrink-0">
              {t(statusLabelKey(host.status))}
            </Chip>
            {statusGuidanceSubtitle ? (
              <span className="mt-0.5 block truncate text-[11px] leading-4 text-muted-foreground">
                {statusGuidanceSubtitle}
              </span>
            ) : null}
          </div>
        </td>
        <td className="max-w-[10rem] px-2 py-2 align-middle">
          {locationLabel || host.countryCode ? (
            <span className="inline-flex min-w-0 max-w-full items-center gap-1.5 text-xs text-muted-foreground">
              <CountryFlag code={host.countryCode} className="h-3.5 w-5 shrink-0 rounded-sm object-cover" />
              {locationLabel ? (
                <span className="truncate" title={locationLabel}>
                  {locationLabel}
                </span>
              ) : null}
            </span>
          ) : (
            <span className="text-xs text-muted-foreground/50">—</span>
          )}
        </td>
        <td className="max-w-[12rem] px-2 py-2 align-middle">
          <div className="flex min-w-0 items-center gap-0.5">
            <span className="min-w-0 truncate text-xs font-medium text-foreground" title={host.hostOwnerEmail}>
              {host.hostOwnerEmail}
            </span>
            <ProviderContactButton contacts={host.contacts} />
          </div>
        </td>
        <td className="max-w-[9rem] px-2 py-2 align-middle">
          <div className="min-w-0">
            <span
              className="block whitespace-nowrap text-xs font-semibold text-foreground"
              title={t("clientMarket.currentOffer")}
            >
              {formatHostOffer(host.dailyRateMinor, locale, host.freeDurationDays)}
            </span>
            {showRenterRental && rental ? (
              <div className="mt-0.5 min-w-0">
                <ClientMarketRentalBanner
                  rental={rental}
                  onChanged={onChanged}
                  compact
                  resumeRelease
                />
              </div>
            ) : null}
          </div>
        </td>
        <td className="max-w-[12rem] px-2 py-2 align-middle">
          <div
            className="min-w-0"
            title={[subdomain || undefined, host.clientOwnerEmail || undefined].filter(Boolean).join(" · ") || undefined}
          >
            <span
              className={`block truncate font-mono text-xs ${subdomain ? "text-muted-foreground" : "text-muted-foreground/50"}`}
            >
              {subdomain || "—"}
            </span>
            {host.clientOwnerEmail ? (
              <span className="mt-0.5 block truncate text-[11px] leading-4 text-muted-foreground">
                {t("clientMarket.rentedBy", { email: host.clientOwnerEmail })}
              </span>
            ) : null}
          </div>
        </td>
        <td className="max-w-[14rem] px-2 py-2 align-middle">
          {ipPort ? (
            <div className="min-w-0" title={[ipPort, host.hostname, ipIntelSubtitle].filter(Boolean).join(" · ")}>
              <span className="block whitespace-nowrap font-mono text-xs text-foreground">{ipPort}</span>
              {ipIntelSubtitle ? (
                <span className="mt-0.5 block truncate text-[11px] leading-4 text-muted-foreground/70">
                  {ipIntelSubtitle}
                </span>
              ) : null}
            </div>
          ) : (
            <span className="text-xs text-muted-foreground/50">—</span>
          )}
        </td>
        <td className="whitespace-nowrap px-2 py-2 align-middle">
          <div className="flex items-center justify-end gap-1">
            {host.installationId ? (
              <span className="relative inline-flex" title={t("clientMarket.openGroupChat")}>
                <Button
                  variant="ghost"
                  size="sm"
                  isIconOnly
                  className="h-8 w-8 min-w-8 shrink-0"
                  onClick={() => void openChat(host.installationId!)}
                  aria-label={t("clientMarket.openGroupChat")}
                >
                  <MessageCircle className="h-4 w-4" />
                </Button>
                {chatUnread > 0 ? (
                  <span className="pointer-events-none absolute -right-1 -top-1 min-w-4 rounded-full bg-red-600 px-1 text-center text-[9px] font-semibold leading-4 text-white">
                    {chatUnread > 99 ? "99+" : chatUnread}
                  </span>
                ) : null}
              </span>
            ) : null}
            {host.status === "idle" ? (
              <Button variant="outline" size="sm" className="h-8 shrink-0" onClick={() => onCreate(host)}>
                <Plus className="h-4 w-4" />
                {t("createClient.newClient")}
              </Button>
            ) : null}
            {canClientRelease && rental ? (
              <ReleaseRentalAction
                rental={rental}
                onChanged={onChanged}
                className="h-8 px-2.5 text-xs text-rose-700"
              />
            ) : null}
            {canOpenTerminal ? (
              <Button
                variant="ghost"
                size="sm"
                isIconOnly
                className="h-8 w-8 min-w-8 shrink-0 border-0 shadow-none"
                onClick={() =>
                  openTerminal({
                    hostId: host.id,
                    title: terminalTitle,
                  })
                }
                aria-label={t("clientMarket.webTerminal")}
              >
                <WebTerminalGlyph className="h-4 w-4 text-muted-foreground" />
              </Button>
            ) : null}
            {hasActions ? (
              <Dropdown>
                <Dropdown.Trigger className="shrink-0 outline-none">
                  <Button
                    variant="ghost"
                    size="sm"
                    isIconOnly
                    className="h-8 w-8 min-w-8"
                    isDisabled={busy}
                    aria-label={t("clientMarket.hostActions")}
                  >
                    {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <MoreHorizontal className="h-4 w-4" />}
                  </Button>
                </Dropdown.Trigger>
                <Dropdown.Popover placement="bottom right">
                  <Dropdown.Menu aria-label={t("clientMarket.hostActions")}>
                    {canManageHost ? (
                      <Dropdown.Item id="offer" onAction={() => setOfferOpen(true)}>
                        <Pencil className="h-4 w-4" />
                        {t("clientMarket.editOfferAction")}
                      </Dropdown.Item>
                    ) : null}
                    {canManageHost ? (
                      <Dropdown.Item id="ssh-host-key" onAction={() => setHostKeyOpen(true)}>
                        <Fingerprint className="h-4 w-4" />
                        {t("clientMarket.sshHostKey.action")}
                      </Dropdown.Item>
                    ) : null}
                    {canReverify ? (
                      <Dropdown.Item id="reverify" onAction={() => void onReverify()}>
                        <RefreshCw className="h-4 w-4" />
                        {t("clientMarket.reverifyHost")}
                      </Dropdown.Item>
                    ) : null}
                    {canCleanup ? (
                      <Dropdown.Item id="cleanup" onAction={() => setConfirmAction("cleanup")}>
                        {t(isRetryCleanup ? "clientMarket.retryCleanup" : "clientMarket.cleanup")}
                      </Dropdown.Item>
                    ) : null}
                    {canDelete ? (
                      <Dropdown.Item
                        id="delete"
                        className="text-destructive"
                        onAction={() => setConfirmAction("delete")}
                      >
                        <Trash2 className="h-4 w-4" />
                        {t("clientMarket.deleteHost")}
                      </Dropdown.Item>
                    ) : null}
                  </Dropdown.Menu>
                </Dropdown.Popover>
              </Dropdown>
            ) : null}
          </div>
        </td>
      </tr>
      {showSubrow ? (
        <tr className="border-b border-border/60 bg-muted/20">
          <td colSpan={colSpan} className="px-2 py-1.5 align-middle">
            <span
              className="min-w-0 whitespace-normal break-words text-[11px] leading-4 text-muted-foreground"
              title={noteText}
            >
              {noteText}
            </span>
          </td>
        </tr>
      ) : null}
      {typeof document !== "undefined"
        ? createPortal(
            <>
              {confirmCopy ? (
                <ConfirmAlertDialog
                  open
                  title={confirmCopy.title}
                  description={confirmCopy.description}
                  confirmLabel={confirmCopy.confirmLabel}
                  cancelLabel={t("common.cancel")}
                  tone="danger"
                  busy={busy}
                  extra={
                    confirmAction === "cleanup" ? (
                      <div className="grid gap-3">
                        <p className="rounded-md border border-sky-200 bg-sky-50 px-3 py-2 text-xs leading-5 text-sky-900">
                          {t("chat.marketPublicNotice")}
                        </p>
                        {host.clientOwnerEmail && !isClientOwner ? (
                          <label className="inline-flex items-start gap-2">
                            <Checkbox
                              isSelected={denyRenterAccess}
                              onChange={setDenyRenterAccess}
                              isDisabled={busy}
                              aria-label={t("clientMarket.denyOnCleanupCheckbox", {
                                email: host.clientOwnerEmail,
                              })}
                              className="mt-0.5 shrink-0"
                            >
                              <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                                <Checkbox.Indicator />
                              </Checkbox.Control>
                            </Checkbox>
                            <span className="leading-5">
                              {t("clientMarket.denyOnCleanupCheckbox", { email: host.clientOwnerEmail })}
                            </span>
                          </label>
                        ) : null}
                      </div>
                    ) : null
                  }
                  onConfirm={() => {
                    if (confirmAction === "cleanup") {
                      void onCleanup(denyRenterAccess);
                    } else void onDelete();
                  }}
                  onOpenChange={(nextOpen) => {
                    if (!nextOpen && !busy) {
                      setConfirmAction(null);
                      setDenyRenterAccess(false);
                    }
                  }}
                />
              ) : null}
              <Modal.Backdrop
                isOpen={cleanupOpen}
                onOpenChange={(next) => {
                  if (!next && !busy) setCleanupOpen(false);
                }}
              >
                <Modal.Container placement="center">
                  <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
                    <Modal.Header>
                      <Modal.Heading className="!text-slate-900">
                        {t("clientMarket.cleanupProgressTitle", { host: hostLabel })}
                      </Modal.Heading>
                    </Modal.Header>
                    <Modal.Body className="grid gap-3 !text-slate-900">
                      <div className="flex flex-wrap items-center gap-2 text-sm">
                        <Chip size="sm" variant="soft">
                          {cleanupJob
                            ? t(cleanupPhaseLabelKey(cleanupPhase))
                            : t("clientMarket.cleanupPhase.starting")}
                        </Chip>
                        {cleanupJob?.status ? (
                          <span className="text-xs text-muted-foreground">{cleanupJob.status}</span>
                        ) : null}
                      </div>
                      <ProvisionJobLog
                        log={cleanupJob?.log || ""}
                        phase={
                          cleanupTone === "failed" ? "failed" : cleanupTone === "success" ? "success" : "running"
                        }
                      />
                      {cleanupJob?.status === "failed" ? (
                        <p className="text-sm text-rose-600">
                          {t(cleanupFailureGuidanceKey(cleanupJob.failureCode || cleanupJob.log))}
                        </p>
                      ) : null}
                      {cleanupJob?.status === "succeeded" ? (
                        <p className="text-sm text-emerald-700">{t("clientMarket.cleanupSucceeded")}</p>
                      ) : null}
                    </Modal.Body>
                    <Modal.Footer>
                      <Button
                        variant="ghost"
                        isDisabled={busy && cleanupJob?.status !== "failed" && cleanupJob?.status !== "succeeded"}
                        onClick={() => setCleanupOpen(false)}
                      >
                        {t("common.close")}
                      </Button>
                    </Modal.Footer>
                  </Modal.Dialog>
                </Modal.Container>
              </Modal.Backdrop>
              <HostOfferDialog host={host} open={offerOpen} onOpenChange={setOfferOpen} onSaved={onChanged} />
              <HostKeyRotationDialog
                hostId={host.id}
                hostLabel={hostLabel}
                open={hostKeyOpen}
                onOpenChange={setHostKeyOpen}
                onUpdated={() => onChanged()}
              />
            </>,
            document.body,
          )
        : null}
    </>
  );
}

/**
 * Memoized: the page polls every 20s and `setHosts` replaces the array each time.
 * `mergeHosts` preserves object identity for unchanged rows, so with stable callback
 * props these rows now bail out instead of all re-rendering on every tick.
 */
export const HostRow = React.memo(HostRowImpl);
HostRow.displayName = "HostRow";
