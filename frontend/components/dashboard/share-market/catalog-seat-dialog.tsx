"use client";

import type { ReactNode } from "react";
import { Button, Modal } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { RentalTermLine, rentalPrice } from "@/components/dashboard/share-market/rental-controls";
import { SeatDiscountNote, USD_SEAT_PARALLEL_CAP } from "@/components/dashboard/share-market/seat-value-quota";
import { formatSeatPrice, formatSeatQuota, isSeatIdle, subscriptionStatusKey } from "@/components/dashboard/share-market/market-utils";
import type { ShareMarketListing, ShareMarketSeat, ShareMarketSubscription } from "@/lib/types";

export function CatalogSeatDialog({
  target,
  subscription,
  busy,
  error,
  blockedReason,
  action,
  onClose,
}: {
  target: { listing: ShareMarketListing; seat: ShareMarketSeat } | null;
  subscription?: ShareMarketSubscription;
  busy: boolean;
  error: string;
  blockedReason?: string;
  action: ReactNode;
  onClose: () => void;
}) {
  const { locale, t } = useLocaleText();
  const seat = target?.seat;
  // A renter sees their frozen contract, not a subsequently edited listing's terms.
  const terms = subscription ?? seat;
  const statusKey = subscription ? subscriptionStatusKey(subscription.status) : null;
  const status = subscription
    ? t(statusKey ?? "shareMarket.catalog.rented")
    : seat && isSeatIdle(seat)
      ? t("shareMarket.available")
      : seat && ["occupied", "reserved", "revoking"].includes(seat.status)
        ? t("shareMarket.occupied")
        : t("shareMarket.unavailable");

  return (
    <Modal.Backdrop isOpen={!!target} isDismissable={!busy} onOpenChange={(open) => !open && !busy && onClose()}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light max-h-[calc(100dvh-1rem)] w-[min(500px,calc(100vw-1rem))] max-w-none overflow-hidden !bg-white !text-slate-900">
          <Modal.Header>
            <div className="min-w-0">
              <Modal.Heading>{seat ? t("shareMarket.catalog.seatPosition", { position: seat.position }) : t("shareMarket.col.seat")}</Modal.Heading>
              <p className="mt-1 break-all text-xs text-slate-500">{target?.listing.shareName}</p>
            </div>
          </Modal.Header>
          <Modal.Body className="grid min-h-0 gap-4 overflow-y-auto">
            {target && terms ? (
              <div data-catalog-seat-details={target.seat.id} className="grid gap-4">
                <div className="flex min-w-0 flex-wrap items-center justify-between gap-3">
                  <strong className="break-words text-2xl font-semibold tabular-nums">{subscription
                    ? rentalPrice(subscription, locale, t)
                    : formatSeatPrice(target.seat, locale, t("shareMarket.free"), t("market.day"))}</strong>
                  <span className="text-xs font-medium text-slate-600">{subscription ? `${t("shareMarket.catalog.mineSeat")} · ` : ""}{status}</span>
                </div>
                <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-3 text-sm">
                  <dt className="text-slate-500">{t("shareMarket.col.limits")}</dt>
                  <dd className="min-w-0 break-words">
                    {t("shareMarket.parallelShort", { value: terms.valueQuota
                      ? Math.min(terms.parallelLimit ?? USD_SEAT_PARALLEL_CAP, USD_SEAT_PARALLEL_CAP)
                      : terms.parallelLimit ?? "∞" })}
                    {" · "}
                    {formatSeatQuota(terms, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`), (amount) => t("shareMarket.quota.usdAmount", { amount }))}
                  </dd>
                  <dt className="text-slate-500">{t("shareMarket.catalog.serviceTerm")}</dt>
                  <dd>{terms.pricingModel === "prepaid_calendar_month"
                    ? t("shareMarket.serviceDuration.monthlyManaged")
                    : terms.serviceDurationDays == null
                      ? t("shareMarket.serviceDuration.permanent")
                      : t("shareMarket.serviceDuration.daysValue", { count: terms.serviceDurationDays })}</dd>
                  {!subscription && !target.seat.isFree && (terms.trialHours ?? 0) > 0 ? <>
                    <dt className="text-slate-500">{t("shareMarket.dialog.trialHours")}</dt>
                    <dd>{terms.trialHours}</dd>
                  </> : null}
                  <dt className="text-slate-500">{t("shareMarket.owner")}</dt>
                  <dd className="min-w-0 break-all">{target.listing.ownerEmail}</dd>
                </dl>
                {subscription ? <RentalTermLine subscription={subscription} /> : <SeatDiscountNote seat={target.seat} />}
                {blockedReason ? <p className="text-xs leading-5 text-slate-500">{blockedReason}</p> : null}
                {error ? <p role="alert" className="text-sm text-rose-700">{error}</p> : null}
              </div>
            ) : null}
          </Modal.Body>
          <Modal.Footer className="flex flex-wrap gap-2">
            <Button variant="ghost" isDisabled={busy} onClick={onClose}>{t("common.close")}</Button>
            {action}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
