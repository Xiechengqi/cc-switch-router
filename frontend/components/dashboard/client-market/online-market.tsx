"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { useAuth } from "@/components/auth/auth-provider";
import { useClientConsole } from "@/components/dashboard/client-console";
import {
  onlineRequest,
  type OnlineListing,
  type OnlineLease,
  type OnlineQuote,
} from "@/lib/online-client-market";
import { ApiError, createMarketAccessRequest } from "@/lib/api";
import {
  clientTerminationHref,
  previewClientTermination,
  terminationExplanationValid,
  terminationPreviewExpired,
  type ClientTerminationPreviewRead,
  type ClientTerminationRequest,
  type ClientTerminationHostSummary,
} from "@/lib/client-termination";
import { ClientTerminationHostAction } from "@/components/market/client-termination-host-action";
import { clientMarketMineHref } from "@/lib/dashboard-nav";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { validPaidWalletQuoteFunding } from "@/lib/market-wallet-quote";
import { trialDuration } from "@/lib/client-checkout";
import { ROUTER_OPEN_LOGIN_EVENT } from "./host-utils";

const field =
  "h-12 rounded-lg border bg-background px-3 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
const focusRing =
  "focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";
const button = `min-h-11 ${focusRing}`;
const link = `inline-flex min-h-11 items-center rounded underline outline-none ${focusRing}`;

const statuses = [
  "available",
  "offline",
  "occupied",
  "locking",
  "unlocking",
  "starting",
  "active",
  "ending",
  "cleaning",
  "released",
  "assistance",
  "terminal",
  "upgrade",
  "restart",
  "rollback",
] as const;
export function onlineStatusKey(value: string) {
  const known = statuses.find((s) => s === value);
  return known ? (`online.${known}` as const) : "online.unknown";
}
export function ListOnlineClient({
  id,
  label,
  listed,
}: {
  id: string;
  label: string;
  listed: boolean;
}) {
  const { t } = useLocaleText();
  const [open, setOpen] = React.useState(false);
  const [submitted, setSubmitted] = React.useState(false);
  React.useEffect(() => {
    setSubmitted(false);
  }, [id, listed]);
  const [price, setPrice] = React.useState("0");
  const [days, setDays] = React.useState("");
  const [ack, setAck] = React.useState(false);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const paid = Number(price) > 0;
  const valid =
    /^\d+(\.\d{1,2})?$/.test(price) &&
    Number(price) <= 1_000_000 &&
    (!days ||
      (/^\d+$/.test(days) &&
        Number(days) >= (paid ? 0 : 1) &&
        Number(days) <= (paid ? 30 : 365)));
  async function submit() {
    if (!ack || !valid || busy) return;
    setBusy(true);
    setError("");
    try {
      await onlineRequest(
        `installations/${encodeURIComponent(id)}/listing`,
        "POST",
        {
          cyclePriceMinor: Math.round(Number(price) * 100),
          freeDays: !paid && days ? Number(days) : null,
          trialDays: paid ? Number(days) : 0,
          acknowledged: true,
        },
      );
      toast.success(t("online.listPending"));
      setSubmitted(true);
      setOpen(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  if (listed || submitted)
    return (
      <a
        href={clientMarketMineHref(id)}
        className={link}
      >
        {t("online.manageListing")}
      </a>
    );
  return (
    <>
      <Button
        size="sm"
        variant="outline"
        className={button}
        onPress={() => {
          setAck(false);
          setError("");
          setOpen(true);
        }}
      >
        {t("online.list")}
      </Button>
      <Modal.Backdrop
        isOpen={open}
        onOpenChange={(v) => {
          if (!busy) setOpen(v);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(440px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t("online.offer")} · {label}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-4">
              <label className="grid gap-1">
                {t("online.price")}
                <input
                  className={field}
                  inputMode="decimal"
                  value={price}
                  onChange={(e) => setPrice(e.target.value)}
                />
              </label>
              <label className="grid gap-1">
                {t(paid ? "online.trialDays" : "online.freeDays")}
                <input
                  className={field}
                  inputMode="numeric"
                  value={days}
                  onChange={(e) => setDays(e.target.value)}
                />
              </label>
              <label className="flex min-h-11 items-start gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={ack}
                  onChange={(e) => setAck(e.target.checked)}
                />
                {t("online.sellerAck")}
              </label>
              {error && (
                <p role="alert" className="text-danger">
                  {error}
                </p>
              )}
            </Modal.Body>
            <Modal.Footer>
              <Button
                variant="ghost"
                className={button}
                onPress={() => setOpen(false)}
                isDisabled={busy}
              >
                {t("common.cancel")}
              </Button>
              <Button
                className={button}
                onPress={() => void submit()}
                isDisabled={!valid || !ack || busy}
              >
                {t("online.list")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </>
  );
}

type MarketProps = {
  focusId: string | null;
  providerId?: string;
  terminations: ReadonlyMap<string, ClientTerminationHostSummary>;
  onTerminationOpen: (id: string) => void;
  onChanged: () => void;
};
export function OnlineClientMarket(props: MarketProps) {
  const { session } = useAuth();
  return (
    <OnlineMarketView
      key={session?.user?.id || "guest"}
      authenticated={!!session?.authenticated}
      {...props}
    />
  );
}
function OnlineMarketView({
  authenticated,
  providerId,
  focusId,
  terminations,
  onTerminationOpen,
  onChanged,
}: MarketProps & { authenticated: boolean }) {
  const { locale, t } = useLocaleText();
  const { openConsole } = useClientConsole();
  const [list, setList] = React.useState<OnlineListing[]>([]);
  const [leases, setLeases] = React.useState<OnlineLease[]>([]);
  const [country, setCountry] = React.useState("");
  const [version, setVersion] = React.useState(0);
  const [loaded, setLoaded] = React.useState(false);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [selection, setSelection] = React.useState<OnlineListing | null>(null);
  const [quote, setQuote] = React.useState<OnlineQuote | null>(null);
  const selected = selection
    ? (list.find((listing) =>
        listing.installationId === selection.installationId && listing.owned === selection.owned,
      ) ?? selection)
    : null;
  const [exitRead, setExitRead] =
    React.useState<ClientTerminationPreviewRead | null>(null);
  const exitPreview = exitRead?.preview ?? null;
  const [exitReason, setExitReason] =
    React.useState<ClientTerminationRequest["reason"]>("voluntary");
  const [claim, setClaim] = React.useState("");
  const [accessRequired, setAccessRequired] = React.useState(false);
  const [ending, setEnding] = React.useState<OnlineLease | null>(null);
  const [ack, setAck] = React.useState(false);
  const [auto, setAuto] = React.useState(false);
  const [dialogError, setDialogError] = React.useState("");
  React.useEffect(() => {
    const control = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    const read = async () => {
      try {
        const [s, r] = await Promise.all([
          onlineRequest<{ listings: OnlineListing[] }>(
            "listings",
            "GET",
            undefined,
            control.signal,
          ),
          authenticated
            ? onlineRequest<{ leases: OnlineLease[] }>(
                "leases",
                "GET",
                undefined,
                control.signal,
              )
            : Promise.resolve({ leases: [] }),
        ]);
        if (!control.signal.aborted) {
          setList(
            s.listings.filter(
              (l) => !providerId || l.providerId === providerId,
            ),
          );
          setLeases(r.leases);
          setError("");
        }
      } catch (e) {
        if (!control.signal.aborted)
          setError(e instanceof Error ? e.message : String(e));
      } finally {
        if (!control.signal.aborted) {
          setLoaded(true);
          timer = setTimeout(() => void read(), 15_000);
        }
      }
    };
    void read();
    return () => {
      control.abort();
      clearTimeout(timer);
    };
  }, [authenticated, version, providerId]);
  const focused = React.useRef<string | null>(null);
  React.useEffect(() => {
    if (!focusId) { focused.current = null; return; }
    if (focusId === focused.current) return;
    const card = document.getElementById(`market-client-${focusId}`);
    if (card) {
      focused.current = focusId;
      card.focus();
    }
  }, [focusId, list, leases]);
  const money = (minor: number) =>
    minor
      ? new Intl.NumberFormat(locale, {
          style: "currency",
          currency: "USD",
        }).format(minor / 100) + t("online.perMonth")
      : t("online.free");
  const date = (sec: number) => new Date(sec * 1000).toLocaleString(locale);
  const region = (code: string | null) =>
    code && /^[A-Z]{2}$/.test(code)
      ? new Intl.DisplayNames([locale], { type: "region" }).of(code) || code
      : t("online.unknown");
  async function action(fn: () => Promise<void>) {
    if (busy) return;
    setBusy(true);
    setDialogError("");
    try {
      await fn();
      setVersion((v) => v + 1);
    } catch (e) {
      if (
        e instanceof ApiError && e.status === 409 &&
        selected && !selected.owned && !ending
      ) {
        setQuote(null);
        setVersion((v) => v + 1);
      }
      setAccessRequired(
        e instanceof ApiError && e.code === "MARKET_ACCESS_REQUIRED",
      );
      setDialogError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }
  const close = () => {
    setSelection(null);
    setEnding(null);
    setExitRead(null);
    setExitReason("voluntary");
    setClaim("");
    setAccessRequired(false);
    setQuote(null);
    setAck(false);
    setAuto(false);
    setDialogError("");
  };
  const funding = quote?.walletFunding;
  const paidQuote = !!quote && quote.terms.cyclePriceMinor > 0;
  const freeDays = quote ? quote.terms.freeDays : selected?.freeDays;
  const funded =
    !paidQuote ||
    (validPaidWalletQuoteFunding(
      funding ?? undefined,
      quote.terms.cyclePriceMinor,
    ) &&
      funding?.shortfallUnits === "0");
  return (
    <section className="grid min-w-0 gap-4" aria-label={t("online.supply")}>
      <div className="flex flex-wrap items-end justify-between gap-3">
        <h2 className="text-lg font-semibold">{t("online.supply")}</h2>
        <label className="flex items-center gap-2 text-sm">
          {t("online.region")}
          <select
            className={field}
            value={country}
            onChange={(e) => setCountry(e.target.value)}
          >
            <option value="">{t("online.all")}</option>
            {Array.from(
              new Set(
                list.map((l) => l.country).filter((c): c is string => !!c),
              ),
            )
              .sort()
              .map((c) => (
                <option key={c} value={c}>
                  {region(c)}
                </option>
              ))}
          </select>
        </label>
      </div>
      {error && (
        <p role="alert" className="text-danger">
          {error}{" "}
          <Button
            variant="ghost"
            className={button}
            onPress={() => setVersion((v) => v + 1)}
          >
            {t("common.reload")}
          </Button>
        </p>
      )}
      {!loaded ? (
        <p role="status">{t("common.loading")}</p>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {list
            .filter((l) => !country || country === l.country)
            .map((l) => {
              const hw = l.hardware;
              const status =
                l.state !== "listed"
                  ? l.state
                  : l.occupied
                    ? "occupied"
                    : l.healthy
                      ? "available"
                      : "offline";
              return (
                <article
                  key={l.installationId}
                  id={`market-client-${l.installationId}`} tabIndex={-1}
                  className="grid content-start gap-3 rounded-xl border bg-background p-4"
                >
                  <div className="flex items-center justify-between gap-2">
                    <h3
                      id={`market-client-${l.installationId}-title`}
                      className="truncate font-semibold"
                    >
                      {l.label}
                    </h3>
                    <span
                      className={
                        status === "available"
                          ? "text-sm text-emerald-700"
                          : "text-sm text-muted-foreground"
                      }
                    >
                      {t(onlineStatusKey(status))}
                    </span>
                  </div>
                  <div>
                    <strong>{region(l.country)}</strong>
                    <span className="ml-2 text-sm text-muted-foreground">
                      {l.egressIp}
                    </span>
                    {l.egressDiffers && (
                      <p className="text-xs text-muted-foreground">
                        {t("online.egressDiffers")}
                      </p>
                    )}
                  </div>
                  <p className="text-sm">
                    {[
                      hw?.arch,
                      hw?.cpu && t("online.cpu", { count: hw.cpu }),
                      hw?.memoryBytes &&
                        t("online.memory", {
                          size: (hw.memoryBytes / 2 ** 30).toFixed(1),
                        }),
                      hw?.diskBytes &&
                        t("online.diskSize", {
                          size: (hw.diskBytes / 2 ** 30).toFixed(0),
                        }),
                    ]
                      .filter(Boolean)
                      .join(" · ") || t("online.unknown")}
                  </p>
                  <dl className="grid gap-1 text-xs text-muted-foreground">
                    <div className="flex justify-between gap-2">
                      <dt className="min-w-0">{t("online.hours")}</dt>
                      <dd className="shrink-0 tabular-nums">
                        {t("online.hoursValue", { hours: l.availableHours24 })}
                      </dd>
                    </div>
                    <div className="flex justify-between gap-2">
                      <dt>{t("online.probe")}</dt>
                      <dd>{l.observedAt ? date(l.observedAt) : "—"}</dd>
                    </div>
                  </dl>
                  <ClientTerminationHostAction
                    summary={terminations.get(l.installationId)}
                    onOpen={onTerminationOpen}
                    describedBy={`market-client-${l.installationId}-title`}
                  />
                  <div className="flex items-center justify-between border-t pt-2">
                    <strong>{money(l.cyclePriceMinor)}</strong>
                    <Button
                      className={button}
                      variant="outline"
                      aria-describedby={`market-client-${l.installationId}-title`}
                      isDisabled={
                        busy ||
                        (l.owned
                          ? l.occupied || l.state === "unlocking"
                          : status !== "available")
                      }
                      onPress={() => {
                        if (!authenticated) {
                          window.dispatchEvent(
                            new Event(ROUTER_OPEN_LOGIN_EVENT),
                          );
                          return;
                        }
                        close();
                        setSelection(l);
                      }}
                    >
                      {t(l.owned ? "online.unlist" : "online.rent")}
                    </Button>
                  </div>
                </article>
              );
            })}
          {!list.some((l) => !country || country === l.country) && !error && (
            <p className="text-sm text-muted-foreground">{t("online.empty")}</p>
          )}
        </div>
      )}
      {leases.length > 0 && (
        <section className="grid gap-2">
          <h2 className="text-lg font-semibold">{t("online.myLeases")}</h2>
          <p className="text-sm text-muted-foreground">
            {t("online.rootNotice")}
          </p>
          {leases.map((l) => (
            <article key={l.leaseId} id={`market-client-${l.leaseId}`} tabIndex={-1} className="rounded-xl border p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <div>
                  <strong id={`market-client-${l.leaseId}-title`}>
                    {l.alias}
                  </strong>
                  <p className="text-sm">
                    {t(onlineStatusKey(l.state))} · {t("online.grantUntil")}{" "}
                    {date(l.expiresAt)}
                  </p>
                </div>
                <div className="flex gap-2">
                  {l.state === "active" && (
                    <Button
                      className={button}
                      aria-describedby={`market-client-${l.leaseId}-title`}
                      onPress={() =>
                        openConsole({
                          clientId: l.leaseId,
                          kind: "console",
                          url: l.clientUrl,
                          title: l.alias,
                        })
                      }
                    >
                      {t("online.console")}
                    </Button>
                  )}
                  {["active", "starting"].includes(l.state) && (
                    <Button
                      variant="outline"
                      className={button}
                      aria-describedby={`market-client-${l.leaseId}-title`}
                      onPress={() => {
                        close();
                        setEnding(l);
                      }}
                    >
                      {t("online.end")}
                    </Button>
                  )}
                </div>
              </div>
              {l.terminationId && (
                <a
                  className={link}
                  href={clientTerminationHref(l.terminationId)}
                  aria-describedby={`market-client-${l.leaseId}-title`}
                >
                  {t("online.exitRecord")}
                </a>
              )}
              {l.events.length > 0 && (
                <details className="mt-2 text-sm">
                  <summary className="cursor-pointer">
                    {t("online.ownerActivity")}
                  </summary>
                  <ul>
                    {l.events.map((e, i) => (
                      <li key={i}>
                        {date(e.at)} · {t(onlineStatusKey(e.kind))}
                      </li>
                    ))}
                  </ul>
                </details>
              )}
            </article>
          ))}
        </section>
      )}
      <Modal.Backdrop
        isOpen={!!selected || !!ending}
        onOpenChange={(v) => {
          if (!v && !busy) close();
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="w-[min(460px,calc(100vw-2rem))]">
            <Modal.Header>
              <Modal.Heading>
                {t(
                  ending
                    ? exitPreview
                      ? "online.end"
                      : "online.previewExit"
                    : selected?.owned
                      ? "online.unlist"
                      : "online.rent",
                )}{" "}
                · {ending?.alias || selected?.label}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              {ending ? (
                <>
                  <p>{t("online.endNotice")}</p>
                  {exitPreview && (
                    <>
                      <p>
                        {t("clientTermination.compensation")} ·{" "}
                        {formatWalletUnits(
                          exitPreview.compensationUnits,
                          "$",
                          locale,
                        )}
                      </p>
                      <p>
                        {t("clientTermination.return")} ·{" "}
                        {formatWalletUnits(
                          exitPreview.returnUnits,
                          "$",
                          locale,
                        )}
                      </p>
                      <label className="grid gap-1">
                        {t("clientTermination.reason")}
                        <select
                          className={field}
                          value={exitReason}
                          onChange={(e) =>
                            setExitReason(
                              e.target
                                .value as ClientTerminationRequest["reason"],
                            )
                          }
                        >
                          <option value="voluntary">
                            {t("clientTermination.voluntary")}
                          </option>
                          <option value="provider_issue">
                            {t("clientTermination.providerIssue")}
                          </option>
                          <option value="platform_issue">
                            {t("clientTermination.platformIssue")}
                          </option>
                        </select>
                      </label>
                      {exitReason !== "voluntary" && (
                        <label className="grid gap-1">
                          {t("clientTermination.claim")}
                          <textarea
                            className="min-h-28 resize-y rounded-lg border bg-background px-3 py-2 focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
                            value={claim}
                            onChange={(e) => setClaim(e.target.value)}
                            maxLength={2000}
                          />
                        </label>
                      )}
                    </>
                  )}
                </>
              ) : selected?.owned ? (
                <p>{t("online.unlistNotice")}</p>
              ) : (
                selected && (
                  <>
                    <p className="font-semibold">
                      {money(
                        quote?.terms.cyclePriceMinor ??
                          selected.cyclePriceMinor,
                      )}
                    </p>
                    {quote?.terms.trialSeconds ? (
                      <p>
                        {t("online.trialHours")} ·{" "}
                        {trialDuration(quote.terms.trialSeconds, locale)}
                      </p>
                    ) : null}
                    {quote && quote.terms.cyclePriceMinor > 0 && (
                      <p>
                        {t("clientTermination.quote", {
                          rate: quote.terms.terminationPolicy.rateBps / 100,
                        })}
                      </p>
                    )}
                    {paidQuote &&
                      (validPaidWalletQuoteFunding(
                        funding ?? undefined,
                        quote.terms.cyclePriceMinor,
                      ) && funding ? (
                        <div>
                          <p>
                            {t("marketWallet.available")} ·{" "}
                            {formatWalletUnits(
                              funding.availableUnits,
                              "$",
                              locale,
                            )}
                          </p>
                          {!funded && (
                            <p>
                              {t("marketWallet.shortfall", {
                                amount: formatWalletUnits(
                                  funding.shortfallUnits,
                                  "$",
                                  locale,
                                ),
                              })}{" "}
                              ·{" "}
                              <a
                                className={`rounded underline outline-none ${focusRing}`}
                                href="/account/wallet/"
                                target="_blank"
                                rel="noreferrer"
                              >
                                {t("marketWallet.title")}
                              </a>
                            </p>
                          )}
                        </div>
                      ) : (
                        <p role="alert">
                          {t("marketWallet.shortcutUnavailable")}
                        </p>
                      ))}
                    {freeDays && (
                      <p>
                        {t("online.freeDuration")} · {freeDays}
                      </p>
                    )}
                    {(quote ? paidQuote : selected.cyclePriceMinor > 0) && (
                      <label className="flex min-h-11 items-start gap-2 text-sm">
                        <input
                          type="checkbox"
                          checked={auto}
                          disabled={!!quote}
                          onChange={(e) => setAuto(e.target.checked)}
                        />
                        {t("online.renew")}
                      </label>
                    )}
                    {quote && (
                      <p className="text-sm">
                        {t("online.quoteUntil")} {date(quote.expiresAt)}
                      </p>
                    )}
                    <label className="flex min-h-11 items-start gap-2 text-sm">
                      <input
                        type="checkbox"
                        checked={ack}
                        onChange={(e) => setAck(e.target.checked)}
                      />
                      {t("online.rootNotice")}
                    </label>
                  </>
                )
              )}
              {accessRequired && selected && (
                <Button
                  className={button}
                  isDisabled={busy}
                  onPress={() =>
                    void action(async () => {
                      await createMarketAccessRequest({
                        targetKind: "client_host",
                        targetId: selected.installationId,
                      });
                      toast.success(t("marketApproval.requested"));
                      close();
                    })
                  }
                >
                  {t("marketApproval.apply")}
                </Button>
              )}
              {dialogError && (
                <p role="alert" className="text-danger">
                  {dialogError}
                </p>
              )}
            </Modal.Body>
            <Modal.Footer>
              <Button
                variant="ghost"
                className={button}
                isDisabled={busy}
                onPress={close}
              >
                {t("common.cancel")}
              </Button>
              <Button
                className={button}
                isDisabled={
                  busy ||
                  (!ending && !selected?.owned && !ack) ||
                  (!!ending &&
                    exitReason !== "voluntary" &&
                    !terminationExplanationValid(claim))
                }
                onPress={() =>
                  void action(async () => {
                    if (ending) {
                      if (!exitRead || terminationPreviewExpired(exitRead)) {
                        setExitRead(
                          await previewClientTermination(ending.leaseId),
                        );
                        return;
                      }
                      try {
                        await onlineRequest(
                          `leases/${ending.leaseId}/end`,
                          "POST",
                          {
                            previewId: exitRead.preview.previewId,
                            reason: exitReason,
                            claim: exitReason === "voluntary" ? "" : claim,
                          },
                        );
                      } catch (e) {
                        // An expired preview or a changed period: the next press previews again.
                        if (e instanceof ApiError && e.status === 409)
                          setExitRead(null);
                        throw e;
                      }
                      onChanged();
                      close();
                    } else if (selected?.owned) {
                      await onlineRequest(
                        `installations/${selected.installationId}/listing`,
                        "DELETE",
                      );
                      close();
                    } else if (selected && (!quote || !funded)) {
                      setQuote(
                        await onlineRequest<OnlineQuote>(
                          `installations/${selected.installationId}/quote`,
                          "POST",
                          { revision: selected.revision, autoRenew: auto },
                        ),
                      );
                    } else if (quote) {
                      // A committed quote can still be replayed after expiry if its reply was lost.
                      await onlineRequest(`quotes/${quote.id}/commit`, "POST", {
                        acknowledged: ack,
                      });
                      onChanged();
                      close();
                    }
                  })
                }
              >
                {t(
                  ending
                    ? exitPreview
                      ? "online.end"
                      : "online.previewExit"
                    : selected?.owned
                      ? "online.unlist"
                      : quote && !funded
                        ? "common.reload"
                        : quote
                          ? "online.confirm"
                          : "online.quote",
                )}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
