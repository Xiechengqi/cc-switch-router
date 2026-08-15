"use client";

import * as React from "react";
import { Button, Chip, toast } from "@heroui/react";
import { Loader2, MessageCircle, ShieldCheck, ShieldOff } from "lucide-react";
import { useClientChat } from "@/components/chat/client-chat";
import { CompactSelect } from "@/components/common/compact-select";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { ClientMarketRentalBanner } from "@/components/dashboard/client-market-rental-banner";
import { CountryFlag } from "@/components/common/country-flag";
import { ReleaseRentalAction } from "@/components/dashboard/client-market/release-rental-action";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { clientConnectionStateLabelKey } from "@/components/dashboard/client-market/host-utils";
import {
  grantClientMarketProviderTerminalAccess,
  revokeClientMarketProviderTerminalAccess,
} from "@/lib/api";
import type { ClientMarketHost, ClientMarketRental } from "@/lib/types";

function ProviderTerminalAccessControl({
  rental,
  onChanged,
}: {
  rental: ClientMarketRental;
  onChanged: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const [durationMinutes, setDurationMinutes] = React.useState("60");
  const [confirmAction, setConfirmAction] = React.useState<"grant" | "revoke" | null>(null);
  const [busy, setBusy] = React.useState(false);
  const expiresAt = rental.providerTerminalAuthorizedUntil
    ? new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(
        new Date(rental.providerTerminalAuthorizedUntil),
      )
    : null;
  const durationOptions = [
    { value: "30", label: t("clientMarket.terminalAccess.duration30m") },
    { value: "60", label: t("clientMarket.terminalAccess.duration1h") },
    { value: "240", label: t("clientMarket.terminalAccess.duration4h") },
    { value: "1440", label: t("clientMarket.terminalAccess.duration24h") },
  ];

  const submit = async () => {
    if (!confirmAction) return;
    setBusy(true);
    try {
      if (confirmAction === "grant") {
        await grantClientMarketProviderTerminalAccess(
          rental.installationId,
          Number(durationMinutes),
        );
        toast.success(t("clientMarket.terminalAccess.granted"));
      } else {
        await revokeClientMarketProviderTerminalAccess(rental.installationId);
        toast.success(t("clientMarket.terminalAccess.revoked"));
      }
      setConfirmAction(null);
      await onChanged();
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <div className="flex flex-col gap-3 border-t border-border pt-3 sm:flex-row sm:items-center sm:justify-between">
        <div className="flex min-w-0 items-start gap-2.5">
          {rental.providerTerminalAccessActive ? (
            <ShieldCheck className="mt-0.5 h-4 w-4 shrink-0 text-emerald-600" />
          ) : (
            <ShieldOff className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
          )}
          <div className="min-w-0">
            <p className="text-sm font-medium text-foreground">
              {t("clientMarket.terminalAccess.title")}
            </p>
            <p className="mt-0.5 text-xs leading-5 text-muted-foreground">
              {rental.providerTerminalAccessActive && expiresAt
                ? t("clientMarket.terminalAccess.activeUntil", { time: expiresAt })
                : t("clientMarket.terminalAccess.description")}
            </p>
          </div>
        </div>
        {rental.canManageProviderTerminal ? (
          <div className="flex flex-wrap items-center gap-2 sm:justify-end">
            <CompactSelect
              value={durationMinutes}
              options={durationOptions}
              onChange={setDurationMinutes}
              ariaLabel={t("clientMarket.terminalAccess.duration")}
              disabled={busy}
              className="w-32"
            />
            <Button
              size="sm"
              variant="outline"
              isDisabled={busy}
              onClick={() => setConfirmAction("grant")}
            >
              {busy && confirmAction === "grant" ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShieldCheck className="h-4 w-4" />}
              {rental.providerTerminalAccessActive
                ? t("clientMarket.terminalAccess.extend")
                : t("clientMarket.terminalAccess.authorize")}
            </Button>
            {rental.providerTerminalAccessActive ? (
              <Button
                size="sm"
                variant="ghost"
                className="text-rose-700"
                isDisabled={busy}
                onClick={() => setConfirmAction("revoke")}
              >
                <ShieldOff className="h-4 w-4" />
                {t("clientMarket.terminalAccess.revoke")}
              </Button>
            ) : null}
          </div>
        ) : null}
      </div>
      <ConfirmAlertDialog
        open={confirmAction != null}
        title={
          confirmAction === "revoke"
            ? t("clientMarket.terminalAccess.revokeTitle")
            : t("clientMarket.terminalAccess.authorizeTitle")
        }
        description={
          confirmAction === "revoke"
            ? t("clientMarket.terminalAccess.revokeConfirm")
            : t("clientMarket.terminalAccess.authorizeConfirm", {
                duration: durationOptions.find((option) => option.value === durationMinutes)?.label || "",
              })
        }
        confirmLabel={
          confirmAction === "revoke"
            ? t("clientMarket.terminalAccess.revoke")
            : t("clientMarket.terminalAccess.authorize")
        }
        cancelLabel={t("common.cancel")}
        busy={busy}
        tone={confirmAction === "revoke" ? "danger" : "warning"}
        onConfirm={() => void submit()}
        onOpenChange={(open) => {
          if (!open && !busy) setConfirmAction(null);
        }}
      />
    </>
  );
}

/**
 * "My rentals" — the renter's side of the Client Market.
 *
 * This used to have no home. A renter's allocated Clients appeared as rows inside
 * the Provider-oriented host table, distinguished only by an `isClientOwner` flag,
 * with rental state bolted on as a subrow. That forced one table to serve two unrelated
 * jobs, which is why its filters and subrows were reworked repeatedly.
 *
 * Note `/v1/client-market/my-rentals` returns rows where the viewer is *either* the
 * renter or the Provider, so `isClientOwner` must be filtered here.
 */
export function MyRentalsPanel({
  rentals,
  hostsByInstallation,
  onChanged,
}: {
  rentals: ClientMarketRental[];
  hostsByInstallation: Map<string, ClientMarketHost>;
  onChanged: () => Promise<void> | void;
}) {
  const { locale, t } = useLocaleText();
  const { openChat, unreadByInstallation } = useClientChat();

  const activeRentals = React.useMemo(
    () =>
      rentals
        .filter((rental) => rental.isClientOwner && rental.status !== "released")
        .sort((left, right) => left.installationId.localeCompare(right.installationId)),
    [rentals],
  );

  if (!activeRentals.length) {
    return (
      <div className="grid justify-items-center gap-2 rounded-xl border border-dashed border-border bg-card/40 px-4 py-12 text-center text-sm text-muted-foreground">
        <span>{t("clientMarket.rentals.empty")}</span>
      </div>
    );
  }

  return (
    <div className="grid gap-3">
      {activeRentals.map((rental) => {
        const host = hostsByInstallation.get(rental.installationId);
        const subdomain = host?.clientSubdomain;
        const chatUnread = unreadByInstallation.get(rental.installationId) || 0;
        return (
          <section
            key={rental.installationId}
            className="grid gap-3 rounded-xl border border-border bg-card p-4 shadow-sm sm:p-5"
          >
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              {host?.countryCode ? (
                <CountryFlag code={host.countryCode} className="h-4 w-4" />
              ) : null}
              <strong className="truncate text-sm">
                {subdomain || host?.hostname || rental.installationId.slice(0, 12)}
              </strong>
              {host ? (
                <Chip size="sm" variant="tertiary">
                  {host.status}
                </Chip>
              ) : null}
              {host?.clientConnection ? (
                <Chip size="sm" variant="soft">
                  {t(clientConnectionStateLabelKey(host.clientConnection.state))}
                </Chip>
              ) : null}
              <span className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                {t("clientMarket.rentals.provider", { owner: rental.hostOwnerEmail })}
              </span>
              <span className="relative inline-flex" title={t("clientMarket.openGroupChat")}>
                <Button
                  variant="ghost"
                  size="sm"
                  isIconOnly
                  className="h-8 w-8 min-w-8 shrink-0"
                  onClick={() => void openChat(rental.installationId)}
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
              {/* Keep mounted through `releasing` so the same instance can finish polling
                  (and reattach after refresh). Retries for release_failed live in the banner. */}
              {rental.status !== "release_failed" ? (
                <ReleaseRentalAction rental={rental} onChanged={onChanged} />
              ) : null}
            </div>
            <ClientMarketRentalBanner
              rental={rental}
              onChanged={onChanged}
              resumeRelease={false}
            />
            {host?.clientConnection?.state === "offline" ? (
              <div className="rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs leading-5 text-amber-900">
                <p>{t("clientMarket.connection.manualAction")}</p>
                {host.clientConnection.lastHeartbeatAt ? (
                  <p className="mt-1 text-amber-800/80">
                    {t("clientMarket.connection.lastHeartbeat", {
                      time: new Date(host.clientConnection.lastHeartbeatAt).toLocaleString(locale),
                    })}
                  </p>
                ) : null}
              </div>
            ) : null}
            <ProviderTerminalAccessControl rental={rental} onChanged={onChanged} />
          </section>
        );
      })}
    </div>
  );
}
