"use client";

import * as React from "react";
import Link from "next/link";
import { Button, Drawer, Modal } from "@heroui/react";
import {
  CalendarClock,
  Check,
  Clock3,
  Gauge,
  Info,
  Loader2,
  LogIn,
  MessageCircle,
  PanelRightOpen,
  Search,
  Send,
  ShieldCheck,
  ShoppingCart,
  UserRound,
  X,
} from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientChat } from "@/components/chat/client-chat";
import { CompactSelect } from "@/components/common/compact-select";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { ProviderContactsList } from "@/components/common/provider-contacts";
import {
  MarketAccessDialog,
  marketEligibilityFromError,
} from "@/components/common/seller-approval-dialog";
import { ShareAppLogo } from "@/components/dashboard/share-app-logo";
import { drawerDialogClassName } from "@/components/dashboard/share-dashboard-utils";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { rentShareMarketSeat } from "@/lib/api";
import { DASHBOARD_ACCOUNT_SHARE_PATH } from "@/lib/dashboard-nav";
import { SHARE_APP_LABELS } from "@/lib/share-app";
import type {
  MarketEligibility,
  ShareMarketCatalog,
  ShareMarketListing,
  ShareMarketProviderFamily,
  ShareMarketSeat,
  ShareMarketSubscription,
  ShareTokenPeriod,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  PROVIDER_FAMILY_KEYS,
  PROVIDER_FAMILY_ORDER,
  activeSubscriptionForShare,
  capabilityModelLabel,
  formatSeatPrice,
  formatTokenLimit,
  isCoreShareApp,
} from "@/components/dashboard/share-market/market-utils";

type SeatCard = { listing: ShareMarketListing; seat: ShareMarketSeat };
type SeatAction = "rent" | "approval" | "login" | "rented" | "selling" | "unavailable";

function familyRank(family: ShareMarketProviderFamily) {
  const rank = PROVIDER_FAMILY_ORDER.indexOf(family);
  return rank < 0 ? PROVIDER_FAMILY_ORDER.length : rank;
}

function seatAction(
  listing: ShareMarketListing,
  seat: ShareMarketSeat,
  subscriptions: ShareMarketSubscription[],
  authed: boolean,
): SeatAction {
  if (listing.isOwner) return "selling";
  if (activeSubscriptionForShare(subscriptions, listing.shareId)) return "rented";
  if (!listing.shareOnline) return "unavailable";
  if (!authed) return "login";
  if (seat.canRent) return "rent";
  if (seat.rentPrerequisitesMet && !seat.eligibility.allowed) return "approval";
  return "unavailable";
}

function AppLogos({ apps }: { apps: string[] }) {
  return (
    <span className="inline-flex items-center gap-1">
      {apps.filter(isCoreShareApp).map((app) => (
        <ShareAppLogo key={app} app={app} size={17} />
      ))}
    </span>
  );
}

function SeatActionButton({
  action,
  busy,
  onAction,
}: {
  action: SeatAction;
  busy: boolean;
  onAction: () => void;
}) {
  const { t } = useLocaleText();
  const config = {
    rent: { label: t("shareMarket.rent"), icon: ShoppingCart, variant: "primary" as const },
    approval: { label: t("marketApproval.apply"), icon: Send, variant: "primary" as const },
    login: { label: t("nav.login"), icon: LogIn, variant: "primary" as const },
    rented: { label: t("shareMarket.catalog.rented"), icon: Check, variant: "outline" as const },
    selling: { label: t("shareMarket.workspace.selling"), icon: Gauge, variant: "outline" as const },
    unavailable: { label: t("shareMarket.unavailable"), icon: ShieldCheck, variant: "outline" as const },
  }[action];
  const Icon = config.icon;
  return (
    <Button
      size="sm"
      variant={config.variant}
      isDisabled={busy || action === "unavailable"}
      onClick={onAction}
    >
      {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Icon className="h-4 w-4" />}
      {config.label}
    </Button>
  );
}

function AppPolicies({ listing }: { listing: ShareMarketListing }) {
  const { t } = useLocaleText();
  return (
    <div className="grid gap-1.5 text-xs">
      {listing.appCapabilities.map((capability) => (
        <div key={capability.app} className="flex min-w-0 items-center gap-2">
          {isCoreShareApp(capability.app) ? <ShareAppLogo app={capability.app} size={14} /> : null}
          <span className="shrink-0 font-medium text-slate-700">
            {isCoreShareApp(capability.app) ? SHARE_APP_LABELS[capability.app] : capability.app}:
          </span>
          <span className="min-w-0 truncate text-slate-500" title={capabilityModelLabel(
            capability,
            t("shareMarket.modelPassthrough"),
            t("shareMarket.catalog.modelUnknown"),
          )}>
            {capabilityModelLabel(
              capability,
              t("shareMarket.modelPassthrough"),
              t("shareMarket.catalog.modelUnknown"),
            )}
          </span>
        </div>
      ))}
    </div>
  );
}

function Metric({ label, value, title }: { label: string; value: string; title?: string }) {
  return (
    <div className="min-w-0">
      <dt className="truncate text-[11px] text-slate-400">{label}</dt>
      <dd className="mt-0.5 truncate text-sm font-semibold tabular-nums text-slate-800" title={title}>{value}</dd>
    </div>
  );
}

function SeatCardView({
  item,
  action,
  busy,
  trialHours,
  onDetails,
  onAction,
}: {
  item: SeatCard;
  action: SeatAction;
  busy: boolean;
  trialHours: number;
  onDetails: () => void;
  onAction: () => void;
}) {
  const { locale, t } = useLocaleText();
  const { listing, seat } = item;
  const performance = listing.performance;
  const reliability = listing.reliability;
  const subscriptionLevels = [...new Set(
    listing.appCapabilities
      .map((capability) => capability.subscriptionLevel?.trim())
      .filter((value): value is string => !!value),
  )];
  const price = formatSeatPrice(
    seat,
    locale,
    t("shareMarket.free"),
    t("marketBilling.day"),
  );
  const service = seat.serviceDurationDays == null
    ? t("shareMarket.serviceDuration.permanent")
    : t("shareMarket.serviceDuration.daysValue", { count: seat.serviceDurationDays });
  const ttft = performance.averageTtftMs == null ? "-" : `${(performance.averageTtftMs / 1_000).toFixed(2)}s`;
  const tps = performance.averageTps == null ? "-" : performance.averageTps.toFixed(1);
  return (
    <article className="grid min-h-[23rem] min-w-0 grid-rows-[auto_auto_auto_1fr_auto] gap-3 rounded-md border border-slate-200 bg-white p-4 shadow-sm transition-colors hover:border-slate-300">
      <header className="flex min-w-0 items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex min-w-0 items-center gap-2">
            <strong className="truncate text-sm text-slate-900">
              {t(PROVIDER_FAMILY_KEYS[listing.providerFamily])}
            </strong>
            {subscriptionLevels.length ? (
              <span className="truncate text-xs text-slate-500" title={subscriptionLevels.join(" / ")}>
                {subscriptionLevels.join(" / ")}
              </span>
            ) : null}
          </div>
          <p className="mt-1 truncate font-mono text-[11px] text-slate-400" title={listing.subdomain || listing.shareName}>
            {listing.subdomain || listing.shareName}
          </p>
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <span className={cn("text-[11px] font-medium", listing.shareOnline ? "text-emerald-700" : "text-rose-600")}>
            {listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
          </span>
          <AppLogos apps={listing.supportedApps} />
        </div>
      </header>

      <div className="min-w-0">
        <div className="flex items-baseline justify-between gap-3">
          <strong className="text-xl font-semibold text-slate-950">{price}</strong>
          <span className="shrink-0 text-xs text-slate-500">#{seat.position}</span>
        </div>
        <p className="mt-1 text-xs text-slate-500">
          {seat.isFree ? service : `${t("shareMarket.catalog.trial", { hours: trialHours })} · ${service}`}
        </p>
      </div>

      <AppPolicies listing={listing} />

      <dl className="grid content-start grid-cols-2 gap-x-4 gap-y-3 border-y border-slate-100 py-3 sm:grid-cols-3">
        <Metric label={t("shareMarket.parallel")} value={seat.parallelLimit == null ? t("common.unlimited") : String(seat.parallelLimit)} />
        <Metric
          label={t("shareMarket.tokens")}
          value={formatTokenLimit(
            seat,
            locale,
            t("common.unlimited"),
            (period: ShareTokenPeriod) => t(`shareMarket.period.${period}`),
          )}
        />
        <Metric
          label="TTFT / TPS"
          value={`${ttft} / ${tps}`}
          title={t("shareMarket.catalog.performanceSamples", {
            ttft: performance.ttftSampleCount,
            tps: performance.tpsSampleCount,
          })}
        />
        <Metric label={t("shareMarket.catalog.uptime24h")} value={`${reliability.onlineRate24h.toFixed(1)}%`} />
        <Metric label={t("shareMarket.catalog.coverage24h")} value={`${reliability.observationCoverage24h.toFixed(1)}%`} />
        <Metric label={t("shareMarket.owner")} value={listing.ownerEmail} />
      </dl>

      <footer className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center">
        <div className="flex min-w-0 items-center gap-2">
          <PaymentMethodIcons kinds={listing.paymentMethodKinds} />
          <span className="truncate text-[11px] text-slate-500">
            {action === "approval"
              ? t("shareMarket.catalog.approvalRequired")
              : seat.isFree
                ? t("shareMarket.free")
                : t("shareMarket.catalog.postpaid")}
          </span>
        </div>
        <div className="flex flex-wrap justify-end gap-2">
          <Button size="sm" variant="ghost" onClick={onDetails}>
            <PanelRightOpen className="h-4 w-4" />
            {t("shareMarket.catalog.details")}
          </Button>
          <SeatActionButton action={action} busy={busy} onAction={onAction} />
        </div>
      </footer>
    </article>
  );
}

export function ShareMarketBuyerCatalog({
  catalog,
  subscriptions,
  authed,
  onChanged,
  onInteractionChange,
  onSwitchSelling,
}: {
  catalog: ShareMarketCatalog;
  subscriptions: ShareMarketSubscription[];
  authed: boolean;
  onChanged: () => Promise<void> | void;
  onInteractionChange?: (active: boolean) => void;
  onSwitchSelling?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const chat = useClientChat();
  const [query, setQuery] = React.useState("");
  const [family, setFamily] = React.useState<ShareMarketProviderFamily | "all">("all");
  const [selected, setSelected] = React.useState<SeatCard | null>(null);
  const [rentTarget, setRentTarget] = React.useState<SeatCard | null>(null);
  const [accessTarget, setAccessTarget] = React.useState<(SeatCard & { eligibility: MarketEligibility }) | null>(null);
  const [busySeatId, setBusySeatId] = React.useState("");
  const [rentError, setRentError] = React.useState("");
  const interactionActive = !!selected || !!rentTarget || !!accessTarget || !!busySeatId;

  React.useEffect(() => {
    onInteractionChange?.(interactionActive);
    return () => onInteractionChange?.(false);
  }, [interactionActive, onInteractionChange]);

  const cards = React.useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    const rows = catalog.listings.flatMap((listing) =>
      listing.seats.map((seat) => ({ listing, seat })),
    );
    return rows
      .filter(({ listing }) =>
        family === "all"
        || listing.providerFamily === family
        || listing.providerFamilies.includes(family),
      )
      .filter(({ listing, seat }) => {
        if (!needle) return true;
        const text = [
          listing.shareName,
          listing.subdomain,
          listing.ownerEmail,
          listing.providerFamily,
          seat.position,
          ...listing.supportedApps,
          ...listing.appCapabilities.flatMap((capability) => [
            capability.providerName,
            capability.providerType,
            capability.subscriptionLevel,
            capability.upstreamModel,
            ...capability.models,
          ]),
        ].filter(Boolean).join(" ").toLocaleLowerCase();
        return text.includes(needle);
      })
      .sort((left, right) =>
        familyRank(left.listing.providerFamily) - familyRank(right.listing.providerFamily)
        || Number(right.listing.shareOnline) - Number(left.listing.shareOnline)
        || (left.seat.dailyRateMinor ?? 0) - (right.seat.dailyRateMinor ?? 0)
        || left.listing.shareName.localeCompare(right.listing.shareName),
      );
  }, [catalog.listings, family, query]);

  const groups = React.useMemo(() => {
    const map = new Map<ShareMarketProviderFamily, SeatCard[]>();
    for (const card of cards) {
      const groupFamily = family === "all" ? card.listing.providerFamily : family;
      const items = map.get(groupFamily) || [];
      items.push(card);
      map.set(groupFamily, items);
    }
    return [...map.entries()].sort(([left], [right]) => familyRank(left) - familyRank(right));
  }, [cards, family]);

  const triggerAction = (item: SeatCard) => {
    const action = seatAction(item.listing, item.seat, subscriptions, authed);
    if (action === "login") {
      window.dispatchEvent(new Event("router-open-login"));
    } else if (action === "rent") {
      setRentError("");
      setRentTarget(item);
    } else if (action === "approval") {
      setAccessTarget({ ...item, eligibility: item.seat.eligibility });
    } else if (action === "rented") {
      window.location.href = `${DASHBOARD_ACCOUNT_SHARE_PATH}?tab=user`;
    } else if (action === "selling") {
      onSwitchSelling?.();
    }
  };

  const confirmRent = async () => {
    if (!rentTarget || busySeatId) return;
    setBusySeatId(rentTarget.seat.id);
    setRentError("");
    try {
      await rentShareMarketSeat(rentTarget.seat.id, rentTarget.seat.offerRevision);
      setRentTarget(null);
      setSelected(null);
      await onChanged();
    } catch (reason) {
      const eligibility = marketEligibilityFromError(reason);
      if (eligibility) {
        setRentTarget(null);
        setAccessTarget({ ...rentTarget, eligibility });
      } else {
        setRentError(reason instanceof Error ? reason.message : String(reason));
      }
    } finally {
      setBusySeatId("");
    }
  };

  const selectedAction = selected
    ? seatAction(selected.listing, selected.seat, subscriptions, authed)
    : "unavailable";
  const familyOptions = [
    { value: "all", label: t("shareMarket.catalog.allFamilies") },
    ...PROVIDER_FAMILY_ORDER.map((value) => ({ value, label: t(PROVIDER_FAMILY_KEYS[value]) })),
  ];

  return (
    <div className="grid min-w-0 gap-5">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <label className="flex h-10 min-w-[14rem] flex-1 items-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm shadow-sm">
          <Search className="h-4 w-4 shrink-0 text-slate-400" aria-hidden />
          <input
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            className="min-w-0 flex-1 bg-transparent outline-none placeholder:text-slate-400"
            placeholder={t("shareMarket.catalog.search")}
            aria-label={t("shareMarket.searchAria")}
          />
          {query ? (
            <button type="button" className="text-slate-400 hover:text-slate-700" aria-label={t("common.close")} onClick={() => setQuery("")}>
              <X className="h-4 w-4" />
            </button>
          ) : null}
        </label>
        <CompactSelect
          value={family}
          options={familyOptions}
          onChange={(value) => setFamily(value as ShareMarketProviderFamily | "all")}
          ariaLabel={t("shareMarket.catalog.familyFilter")}
          className="w-full sm:w-48"
          triggerClassName="h-10 w-full text-sm"
        />
        {subscriptions.length ? (
          <Link
            href={`${DASHBOARD_ACCOUNT_SHARE_PATH}?tab=user`}
            className="inline-flex h-10 items-center gap-2 rounded-md border border-slate-200 bg-white px-3 text-sm font-medium text-slate-700 hover:bg-slate-50"
          >
            <UserRound className="h-4 w-4" />
            {t("shareMarket.catalog.myRentals", { count: subscriptions.filter((item) => !["released", "grant_failed"].includes(item.status)).length })}
          </Link>
        ) : null}
      </div>

      {groups.map(([providerFamily, items]) => (
        <section key={providerFamily} className="grid min-w-0 gap-3">
          <div className="flex items-baseline justify-between border-b border-slate-200 pb-2">
            <h2 className="text-sm font-semibold text-slate-900">{t(PROVIDER_FAMILY_KEYS[providerFamily])}</h2>
            <span className="text-xs tabular-nums text-slate-400">{items.length}</span>
          </div>
          <div className="grid min-w-0 gap-3 md:grid-cols-2 xl:grid-cols-3">
            {items.map((item) => (
              <SeatCardView
                key={item.seat.id}
                item={item}
                action={seatAction(item.listing, item.seat, subscriptions, authed)}
                busy={busySeatId === item.seat.id}
                trialHours={catalog.trialHours}
                onDetails={() => setSelected(item)}
                onAction={() => triggerAction(item)}
              />
            ))}
          </div>
        </section>
      ))}

      {!cards.length ? (
        <div className="grid min-h-48 place-items-center border-y border-dashed border-slate-200 text-sm text-slate-500">
          {t("shareMarket.catalog.empty")}
        </div>
      ) : null}

      <Drawer.Backdrop isOpen={!!selected} onOpenChange={(open) => !open && setSelected(null)}>
        <Drawer.Content placement="right">
          <Drawer.Dialog className={drawerDialogClassName}>
            <Drawer.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" />
            <Drawer.Header>
              <div className="min-w-0 pr-10">
                <div className="flex min-w-0 items-center gap-2">
                  <Drawer.Heading className="truncate text-base">{selected?.listing.shareName}</Drawer.Heading>
                  {selected ? <AppLogos apps={selected.listing.supportedApps} /> : null}
                </div>
                <p className="mt-1 break-all font-mono text-xs text-slate-500">{selected?.listing.subdomain}</p>
              </div>
            </Drawer.Header>
            <Drawer.Body className="overflow-y-auto pb-28">
              {selected ? (
                <div className="grid gap-5">
                  <section className="grid gap-2 border-b border-slate-200 pb-4">
                    <div className="flex flex-wrap items-center justify-between gap-2">
                      <strong className="text-sm">{t(PROVIDER_FAMILY_KEYS[selected.listing.providerFamily])}</strong>
                      <span className={selected.listing.shareOnline ? "text-sm font-medium text-emerald-700" : "text-sm font-medium text-rose-700"}>
                        {selected.listing.shareOnline ? t("shareMarket.online") : t("shareMarket.offline")}
                      </span>
                    </div>
                    <p className="text-sm text-slate-600">{t("shareMarket.catalog.seatPosition", { position: selected.seat.position })}</p>
                  </section>

                  <section className="grid gap-3 border-b border-slate-200 pb-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.appCapabilities")}</h3>
                    {selected.listing.appCapabilities.map((capability) => (
                      <div key={capability.app} className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
                        {isCoreShareApp(capability.app) ? <ShareAppLogo app={capability.app} size={18} /> : <Info className="h-5 w-5" />}
                        <div className="min-w-0">
                          <strong>{isCoreShareApp(capability.app) ? SHARE_APP_LABELS[capability.app] : capability.app}</strong>
                          <p className="break-words text-slate-600">
                            {[capability.providerName || capability.providerType, capability.subscriptionLevel].filter(Boolean).join(" · ") || t("shareMarket.catalog.providerUnknown")}
                          </p>
                          <p className="break-words text-slate-500">
                            {capabilityModelLabel(capability, t("shareMarket.modelPassthrough"), t("shareMarket.catalog.modelUnknown"))}
                          </p>
                        </div>
                      </div>
                    ))}
                  </section>

                  <section className="grid gap-3 border-b border-slate-200 pb-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.terms")}</h3>
                    <dl className="grid grid-cols-2 gap-3 text-sm">
                      <Metric label={t("shareMarket.parallel")} value={selected.seat.parallelLimit == null ? t("common.unlimited") : String(selected.seat.parallelLimit)} />
                      <Metric label={t("shareMarket.tokens")} value={formatTokenLimit(selected.seat, locale, t("common.unlimited"), (period) => t(`shareMarket.period.${period}`))} />
                      <Metric label={t("shareMarket.catalog.serviceTerm")} value={selected.seat.serviceDurationDays == null ? t("shareMarket.serviceDuration.permanent") : t("shareMarket.serviceDuration.daysValue", { count: selected.seat.serviceDurationDays })} />
                      <Metric label={t("shareMarket.catalog.billingStart")} value={selected.seat.isFree ? t("shareMarket.free") : t("shareMarket.catalog.afterTrial", { hours: catalog.trialHours })} />
                    </dl>
                  </section>

                  <section className="grid gap-3 border-b border-slate-200 pb-4">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.serviceQuality")}</h3>
                    <dl className="grid grid-cols-2 gap-3 text-sm sm:grid-cols-3">
                      <Metric label="TTFT" value={selected.listing.performance.averageTtftMs == null ? "-" : `${(selected.listing.performance.averageTtftMs / 1_000).toFixed(2)}s`} />
                      <Metric label="TPS" value={selected.listing.performance.averageTps == null ? "-" : selected.listing.performance.averageTps.toFixed(1)} />
                      <Metric label={t("shareMarket.catalog.samples")} value={String(selected.listing.performance.recentRequestCount)} />
                      <Metric label={t("shareMarket.catalog.uptime24h")} value={`${selected.listing.reliability.onlineRate24h.toFixed(1)}%`} />
                      <Metric label={t("shareMarket.catalog.coverage24h")} value={`${selected.listing.reliability.observationCoverage24h.toFixed(1)}%`} />
                      <Metric label={t("shareMarket.catalog.observedMinutes")} value={String(selected.listing.reliability.observedMinutes24h)} />
                    </dl>
                  </section>

                  <section className="grid gap-3">
                    <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.seller")}</h3>
                    <div className="flex min-w-0 flex-wrap items-center gap-2 text-sm">
                      <UserRound className="h-4 w-4 text-slate-400" />
                      <span className="min-w-0 break-all font-medium">{selected.listing.ownerEmail}</span>
                      <PaymentMethodIcons kinds={selected.listing.paymentMethodKinds} />
                    </div>
                    <ProviderContactsList contacts={selected.listing.contacts} />
                    <Button variant="outline" onClick={() => void chat.openClientChat(selected.listing.installationId)}>
                      <MessageCircle className="h-4 w-4" />
                      {t("marketApproval.openChat")}
                    </Button>
                  </section>

                  {selected.listing.seats.length > 1 ? (
                    <section className="grid gap-2 border-t border-slate-200 pt-4">
                      <h3 className="text-xs font-semibold uppercase text-slate-500">{t("shareMarket.catalog.otherSeats")}</h3>
                      {selected.listing.seats.filter((seat) => seat.id !== selected.seat.id).map((seat) => (
                        <button key={seat.id} type="button" className="flex items-center justify-between border-b border-slate-100 py-2 text-left text-sm last:border-0" onClick={() => setSelected({ listing: selected.listing, seat })}>
                          <span>#{seat.position}</span>
                          <strong>{formatSeatPrice(seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
                        </button>
                      ))}
                    </section>
                  ) : null}
                </div>
              ) : null}
            </Drawer.Body>
            {selected ? (
              <div className="absolute inset-x-0 bottom-0 flex items-center justify-between gap-3 border-t border-slate-200 bg-white px-5 py-4">
                <div className="min-w-0">
                  <strong className="block truncate text-lg">{formatSeatPrice(selected.seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</strong>
                  <span className="text-xs text-slate-500">#{selected.seat.position}</span>
                </div>
                <SeatActionButton action={selectedAction} busy={busySeatId === selected.seat.id} onAction={() => triggerAction(selected)} />
              </div>
            ) : null}
          </Drawer.Dialog>
        </Drawer.Content>
      </Drawer.Backdrop>

      <Modal.Backdrop isOpen={!!rentTarget} onOpenChange={(open) => !open && !busySeatId && setRentTarget(null)}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("shareMarket.rentConfirm.title")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid gap-3">
              {rentTarget ? (
                <>
                  <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 border-y border-slate-200 py-3 text-sm">
                    <dt className="text-slate-500">{t("shareMarket.col.share")}</dt>
                    <dd className="truncate font-medium">{rentTarget.listing.shareName} · #{rentTarget.seat.position}</dd>
                    <dt className="text-slate-500">{t("shareMarket.owner")}</dt>
                    <dd className="break-all">{rentTarget.listing.ownerEmail}</dd>
                    <dt className="text-slate-500">{t("shareMarket.dialog.amount")}</dt>
                    <dd className="font-medium">{formatSeatPrice(rentTarget.seat, locale, t("shareMarket.free"), t("marketBilling.day"))}</dd>
                  </dl>
                  <div className="flex gap-3 border-l-2 border-sky-400 bg-sky-50 px-3 py-2 text-sm leading-6 text-sky-950">
                    <Clock3 className="mt-1 h-4 w-4 shrink-0" />
                    <p>{rentTarget.seat.isFree ? t("shareMarket.rentConfirm.freeBilling") : t("shareMarket.rentConfirm.postpaid", { hours: catalog.trialHours })}</p>
                  </div>
                  <div className="flex gap-3 border-l-2 border-amber-400 bg-amber-50 px-3 py-2 text-sm leading-6 text-amber-950">
                    <CalendarClock className="mt-1 h-4 w-4 shrink-0" />
                    <p>{rentTarget.seat.serviceDurationDays == null ? t("shareMarket.rentConfirm.servicePermanent") : t("shareMarket.rentConfirm.serviceFixed", { days: rentTarget.seat.serviceDurationDays })}</p>
                  </div>
                  {rentError ? <p className="text-sm text-rose-700">{rentError}</p> : null}
                </>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={!!busySeatId} onClick={() => setRentTarget(null)}>{t("common.cancel")}</Button>
              <Button variant="primary" isDisabled={!!busySeatId} onClick={() => void confirmRent()}>
                {busySeatId ? <Loader2 className="h-4 w-4 animate-spin" /> : <ShoppingCart className="h-4 w-4" />}
                {t("shareMarket.rentConfirm.confirm")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <MarketAccessDialog
        open={!!accessTarget}
        product="share"
        ownerEmail={accessTarget?.listing.ownerEmail || ""}
        buyerEmail={session?.user?.email || ""}
        contacts={accessTarget?.listing.contacts}
        targetKind="share_seat"
        targetId={accessTarget?.seat.id || ""}
        currency={accessTarget?.seat.currency}
        eligibility={accessTarget?.eligibility || { allowed: false, status: "access_required" }}
        onOpenChange={(open) => !open && setAccessTarget(null)}
        onOpenChat={() => accessTarget ? chat.openClientChat(accessTarget.listing.installationId) : undefined}
        onRequestChanged={onChanged}
      />
    </div>
  );
}
