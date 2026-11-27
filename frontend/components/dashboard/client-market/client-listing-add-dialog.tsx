"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { ArrowUpRight, Loader2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError, getDashboard } from "@/lib/api";
import {
  buildDashboardHref,
  clientMarketMineHref,
  DASHBOARD_CLIENTS_PATH,
} from "@/lib/dashboard-nav";
import {
  listingBlocker,
  onlineRequest,
  type ListingBlocker,
  type ListingPreflight,
} from "@/lib/online-client-market";
import type { DashboardClient } from "@/lib/types";
import { ROUTER_OPEN_LOGIN_EVENT } from "./host-utils";
import { ClientReliability, ClientSpecs } from "./online-shop";
import { EMPTY_LISTING_TERMS, ListingTermsFields, listingTermsFrom, type ListingTermsDraft } from "./client-listing-terms";

type Issue =
  | ListingBlocker
  | "client_missing"
  | "session_expired"
  | "check_unavailable"
  | "submit_uncertain"
  | "verify_uncertain";
// `wait`: nothing to do from here (the seller finishes it elsewhere, or it clears by itself).
type Remedy = "listing" | "client" | "verifyProvider" | "recheck" | "signIn" | "wait";
const remedies: Record<Issue, Remedy> = {
  already_listed: "listing",
  listing_changed: "recheck",
  owner_unverified: "client",
  inactive: "client",
  upgrade_required: "client",
  offline: "client",
  share_in_use: "wait",
  workspace_busy: "wait",
  control_unavailable: "recheck",
  egress_unverified: "recheck",
  hardware_unavailable: "client",
  provider_verification: "verifyProvider",
  email_verification: "signIn",
  provider_paused: "recheck",
  provider_conflict: "recheck",
  client_missing: "client",
  session_expired: "signIn",
  check_unavailable: "recheck",
  submit_uncertain: "recheck",
  verify_uncertain: "recheck",
};
const actionLabels = {
  listing: "clientMarket.listing.seeListings",
  client: "clientMarket.listing.manageClient",
  verifyProvider: "clientMarket.listing.verifyProvider",
  recheck: "clientMarket.listing.recheck",
  signIn: "clientMarket.listing.signIn",
} as const;
const focus = "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";

function paidIssue(issue: Issue | null) {
  return (
    issue === "provider_verification" ||
    issue === "email_verification" ||
    issue === "provider_paused" ||
    issue === "verify_uncertain"
  );
}

function errorIssue(error: unknown, fallback: Issue): Issue {
  if (error instanceof ApiError) {
    const reason = error.details?.reason;
    if (
      error.code === "CLIENT_LISTING_BLOCKED" &&
      typeof reason === "string" &&
      Object.hasOwn(remedies, reason)
    ) {
      return reason as Issue;
    }
    if (error.status === 401) return "session_expired";
    if (error.status === 404) return "client_missing";
  }
  return fallback;
}

function deviceName(client: DashboardClient) {
  return client.clientTunnel?.subdomain || client.installation.id;
}

type Probe = { id: string; revision: number; data?: ListingPreflight; failure?: Issue };

export function ClientListingAddDialog({
  initialId,
  onClose,
  onListed,
}: {
  initialId: string | null;
  onClose: () => void;
  onListed: () => void;
}) {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const owner = session?.user?.email?.trim().toLowerCase();
  const formId = React.useId();
  const [clients, setClients] = React.useState<DashboardClient[] | null>(null);
  const [loadError, setLoadError] = React.useState(false);
  const [loadRevision, reloadClients] = React.useReducer((n) => n + 1, 0);
  const [id, setId] = React.useState(initialId);
  const [probe, setProbe] = React.useState<Probe | null>(null);
  const [revision, recheck] = React.useReducer((n) => n + 1, 0);
  const [draft, setDraft] = React.useState<ListingTermsDraft>(EMPTY_LISTING_TERMS);
  const [pendingAction, setPendingAction] = React.useState<"verify" | "publish" | null>(null);
  const mutation = React.useRef<AbortController | null>(null);
  const [signingIn, setSigningIn] = React.useState(false);
  const [submitIssue, setSubmitIssue] = React.useState<Issue | null>(null);
  const [termsRejected, setTermsRejected] = React.useState(false);

  React.useEffect(() => {
    const ctl = new AbortController();
    let current = true;
    const timeout = window.setTimeout(() => ctl.abort(), 20_000);
    setLoadError(false);
    getDashboard(ctl.signal)
      .then((dashboard) => {
        if (ctl.signal.aborted) return;
        const mine = dashboard.clients.filter(
          (c) => owner && c.installation.ownerEmail?.trim().toLowerCase() === owner,
        );
        setClients(mine);
        setId((current) => {
          if (mine.some((c) => c.installation.id === current)) return current;
          // A deep link to a missing device must not silently offer a different one.
          if (current !== null) return "";
          const eligible = mine.find(
            (c) => !c.installation.marketListed && c.installation.marketLeaseV1 && c.clientTunnel?.online,
          );
          return (eligible || mine[0])?.installation.id ?? null;
        });
      })
      .catch(() => {
        if (current) setLoadError(true);
      })
      .finally(() => window.clearTimeout(timeout));
    return () => {
      current = false;
      window.clearTimeout(timeout);
      ctl.abort();
    };
  }, [owner, loadRevision]);
  const selected = clients?.find((client) => client.installation.id === id);
  const selectedId = selected?.installation.id;

  React.useEffect(() => {
    if (!selectedId) return;
    const ctl = new AbortController();
    let current = true;
    const timeout = window.setTimeout(() => ctl.abort(), 20_000);
    // The same probe returns both device readiness and the seller's paid eligibility.
    void onlineRequest<ListingPreflight>(
      `installations/${encodeURIComponent(selectedId)}/preflight`,
      "POST",
      { paid: false },
      ctl.signal,
    )
      .then((data) => {
        if (current) setProbe({ id: selectedId, revision, data });
      })
      .catch((err) => {
        if (current) setProbe({ id: selectedId, revision, failure: errorIssue(err, "check_unavailable") });
      })
      .finally(() => window.clearTimeout(timeout));
    return () => {
      current = false;
      window.clearTimeout(timeout);
      ctl.abort();
    };
  }, [selectedId, revision]);

  // A re-check keeps the last answer on screen, so the dialog never flashes
  // back to "checking"; only a device's first check shows the spinner.
  const lastProbe = probe?.id === selectedId ? probe : null;
  const checking = !!selected && !lastProbe;
  const rechecking = !!lastProbe && lastProbe.revision !== revision;
  const check = lastProbe?.data;
  const terms = listingTermsFrom(draft);
  const paid = terms.paid;
  const deviceIssue =
    lastProbe?.failure ?? (lastProbe && !check?.checks ? "check_unavailable" : check && listingBlocker(check));
  // Earlier Routers can claim a first-time Provider during publication itself.
  const paidBlocker =
    paid && check?.paidReady === false && check.paidBlocker !== undefined
      ? (check.paidBlocker ?? "provider_verification")
      : null;
  const reported = deviceIssue ?? (paid || !paidIssue(submitIssue) ? submitIssue : null) ?? paidBlocker;
  const issue = reported && !Object.hasOwn(remedies, reported) ? "check_unavailable" : reported;
  const paidOnly = paidIssue(issue);
  const issueHeading = paidOnly
    ? "clientMarket.listing.paidSetup"
    : issue === "submit_uncertain" || issue === "check_unavailable"
      ? "clientMarket.listing.unknownStatus"
      : "clientMarket.listing.blocked";
  const showPricing = !!check?.ready && (!issue || paidOnly);
  const remedy = issue ? remedies[issue] : null;

  const refresh = React.useCallback(() => {
    if (mutation.current) return;
    setSubmitIssue(null);
    setTermsRejected(false);
    recheck();
  }, []);
  React.useEffect(
    () => () => {
      mutation.current?.abort();
      mutation.current = null;
    },
    [],
  );
  React.useEffect(() => {
    if (!signingIn) return;
    const onLoginDialogChange = (event: Event) => {
      if ((event as CustomEvent<{ open: boolean }>).detail?.open === false) {
        setSigningIn(false);
        refresh();
      }
    };
    window.addEventListener("router-login-dialog-changed", onLoginDialogChange);
    return () => window.removeEventListener("router-login-dialog-changed", onLoginDialogChange);
  }, [signingIn, refresh]);
  React.useEffect(() => {
    if (!issue && !loadError && clients?.length) return;
    let timer: ReturnType<typeof setTimeout>;
    const onReturn = () => {
      if (document.visibilityState !== "visible" || mutation.current) return;
      clearTimeout(timer);
      timer = setTimeout(() => {
        if (loadError || !clients?.length || issue === "client_missing") reloadClients();
        refresh();
      }, 100);
    };
    window.addEventListener("focus", onReturn);
    document.addEventListener("visibilitychange", onReturn);
    return () => {
      clearTimeout(timer);
      window.removeEventListener("focus", onReturn);
      document.removeEventListener("visibilitychange", onReturn);
    };
  }, [issue, loadError, clients, refresh]);
  React.useEffect(() => {
    if (issue !== "workspace_busy" || checking) return;
    const timer = window.setTimeout(refresh, 15_000);
    return () => window.clearTimeout(timer);
  }, [issue, checking, refresh]);

  function select(next: string) {
    if (next === id || mutation.current) return;
    setId(next);
    setProbe(null);
    recheck();
    setDraft(EMPTY_LISTING_TERMS);
    setSubmitIssue(null);
    setTermsRejected(false);
  }
  const busy = pendingAction !== null;
  const canSubmit = !!selected && !!check?.ready && !issue && !!terms.payload && !busy && !rechecking && !loadError;

  async function perform(action: "publish" | "verify") {
    if (!selectedId || mutation.current) return;
    if (action === "publish" ? !canSubmit || !terms.payload : remedy !== "verifyProvider" || checking || rechecking) return;
    const ctl = new AbortController();
    mutation.current = ctl;
    setPendingAction(action);
    setTermsRejected(false);
    const timeout = window.setTimeout(() => ctl.abort(), 35_000);
    const endpoint = `installations/${encodeURIComponent(selectedId)}`;
    try {
      if (action === "verify") {
        const result = await onlineRequest<{ paidReady: boolean }>(
          `${endpoint}/verify-provider`,
          "POST",
          undefined,
          ctl.signal,
        );
        if (mutation.current !== ctl) return;
        if (result?.paidReady === true) {
          setSubmitIssue(null);
          recheck();
        } else setSubmitIssue("verify_uncertain");
        return;
      }
      const result = await onlineRequest<{ state: string }>(
        `${endpoint}/listing`,
        "POST",
        // The dialog states these lease rules before publishing.
        { ...terms.payload, acknowledged: true },
        ctl.signal,
      );
      if (mutation.current !== ctl) return;
      if (result?.state !== "locking") {
        setSubmitIssue("submit_uncertain");
        return;
      }
      toast.success(t("online.listPending"));
      onListed();
      onClose();
    } catch (err) {
      if (mutation.current !== ctl) return;
      if (action === "publish" && err instanceof ApiError && err.status === 400) setTermsRejected(true);
      else setSubmitIssue(errorIssue(err, action === "publish" ? "submit_uncertain" : "verify_uncertain"));
    } finally {
      window.clearTimeout(timeout);
      if (mutation.current === ctl) {
        mutation.current = null;
        setPendingAction(null);
      }
    }
  }

  const clientHref =
    selectedId && issue !== "client_missing"
      ? buildDashboardHref(
          DASHBOARD_CLIENTS_PATH,
          new URLSearchParams({ drawerKind: "client", drawerId: selectedId }),
        )
      : DASHBOARD_CLIENTS_PATH;

  function primaryAction() {
    if (loadError)
      return (
        <Button variant="primary" onPress={reloadClients}>
          {t("common.retry")}
        </Button>
      );
    if (!clients || checking) return null;
    const action = clients.length ? remedy : "client";
    if (action === "client" || action === "listing") {
      const links = {
        client: clientHref,
        listing: clientMarketMineHref(selectedId),
      };
      return (
        <a
          className={
            "inline-flex min-h-11 items-center gap-1.5 rounded-lg bg-slate-900 px-4 text-sm font-semibold text-white hover:bg-slate-800 " +
            focus
          }
          href={links[action]}
          target="_blank"
          rel="noopener noreferrer"
        >
          {t(actionLabels[action])}
          <ArrowUpRight aria-hidden="true" className="size-4" />
        </a>
      );
    }
    if (action === "verifyProvider")
      return (
        <Button variant="primary" isDisabled={busy || rechecking} onPress={() => void perform("verify")}>
          {t(pendingAction === "verify" ? "clientMarket.listing.verifying" : actionLabels.verifyProvider)}
        </Button>
      );
    if (action === "recheck")
      return (
        <Button variant="primary" onPress={refresh}>
          {t(actionLabels.recheck)}
        </Button>
      );
    if (action === "signIn")
      return (
        <Button
          variant="primary"
          onPress={() => {
            setSigningIn(true);
            window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
          }}
        >
          {t(actionLabels.signIn)}
        </Button>
      );
    if (action || !showPricing) return null;
    return (
      <Button type="submit" form={formId} variant="primary" isDisabled={!canSubmit}>
        {t(
          pendingAction === "publish"
            ? "clientMarket.listing.publishing"
            : paid
              ? "clientMarket.shop.dialog.publish"
              : "clientMarket.listing.publishFree",
        )}
      </Button>
    );
  }

  return (
    <Modal.Backdrop
      isOpen={!signingIn}
      isDismissable={!busy}
      isKeyboardDismissDisabled={busy}
      onOpenChange={(open) => {
        if (!open && !mutation.current && !signingIn) onClose();
      }}
    >
      <Modal.Container placement="center" scroll="inside" className="w-full max-w-[34rem] p-3 sm:w-full sm:p-3">
        <Modal.Dialog className="flex max-h-full min-h-0 w-full max-w-full flex-col gap-0 overflow-hidden rounded-2xl! border border-border bg-card! p-0 text-card-foreground">
          <Modal.Header className="shrink-0 px-5 pt-5 pb-3 sm:px-6 sm:pt-6">
            <Modal.Heading className="text-xl font-semibold tracking-tight">
              {t("clientMarket.shop.dialog.title")}
            </Modal.Heading>
          </Modal.Header>
          <Modal.Body className="min-h-0 flex-1 overflow-y-auto overscroll-contain px-5 pt-1 pb-5 sm:px-6">
            {loadError ? (
              <p role="alert" className="text-sm">
                {t("clientMarket.listing.loadFailed")}
              </p>
            ) : !clients ? (
              <p role="status" className="text-sm text-muted-foreground">
                {t("common.loading")}
              </p>
            ) : !clients.length ? (
              <p className="text-sm text-muted-foreground">{t("clientMarket.shop.dialog.none")}</p>
            ) : (
              <>
                <div className="grid min-w-0 gap-2">
                  {clients.length === 1 && selected ? (
                    <strong className="break-all font-mono text-base">{deviceName(selected)}</strong>
                  ) : (
                    <CompactSelect
                      ariaLabel={t("clientMarket.shop.dialog.select")}
                      value={id ?? ""}
                      disabled={busy}
                      className="w-full"
                      triggerClassName="h-9 w-full"
                      onChange={select}
                      options={[
                        ...(selected ? [] : [{ value: "", label: t("clientMarket.listing.choose") }]),
                        ...clients.map((c) => ({
                          value: c.installation.id,
                          label: deviceName(c),
                          content: <span className="block truncate font-mono text-xs font-semibold">{deviceName(c)}</span>,
                        })),
                      ]}
                    />
                  )}
                  {!selected && id === "" && (
                    <p className="text-sm text-muted-foreground">
                      {t("clientMarket.listing.unavailableSelection")}
                    </p>
                  )}
                  {selected && showPricing && (
                    <>
                      {check?.resource && (
                        <ClientSpecs
                          listing={{ country: check.countryCode ?? null, hardware: check.resource }}
                        />
                      )}
                      <ClientReliability
                        inline
                        listing={{
                          registeredAt: selected.installation.createdAt,
                          onlineRate24h: selected.onlineRate24h ?? null,
                          observedMinutes24h: selected.observedMinutes24h ?? 0,
                          observationCoverage24h: selected.observationCoverage24h ?? 0,
                        }}
                      />
                    </>
                  )}
                </div>
                {showPricing && (
                  <form
                    id={formId}
                    onSubmit={(event) => {
                      event.preventDefault();
                      void perform("publish");
                    }}
                    onChange={() => setTermsRejected(false)}
                    className="mt-4 grid gap-4"
                  >
                    <ListingTermsFields
                      draft={draft}
                      onChange={(next) => {
                        setDraft(next);
                        setTermsRejected(false);
                      }}
                      disabled={busy}
                      idPrefix={formId}
                      showPricing={!paidOnly}
                    />
                    {!paidOnly && (
                      <p className="text-xs leading-5 text-muted-foreground">{t("clientMarket.listing.notice")}</p>
                    )}
                  </form>
                )}
                {checking ? (
                  <p role="status" className="mt-3 flex items-center gap-2 text-sm text-muted-foreground">
                    <Loader2 aria-hidden="true" className="size-4 motion-safe:animate-spin" />
                    {t("clientMarket.listing.checking")}
                  </p>
                ) : (
                  issue && (
                    <div
                      role="status"
                      className="mt-4 grid gap-1 rounded-lg bg-slate-100 px-4 py-3"
                    >
                      <strong className="text-sm font-semibold">{t(issueHeading)}</strong>
                      <p className="text-sm leading-5 text-muted-foreground">
                        {t(`clientMarket.listing.blocker.${issue}`)}
                      </p>
                    </div>
                  )
                )}
                {termsRejected && (
                  <p role="alert" className="mt-2 text-sm text-danger">
                    {t("clientMarket.listing.invalidTerms")}
                  </p>
                )}
              </>
            )}
          </Modal.Body>
          <Modal.Footer className="mt-0! flex shrink-0 flex-wrap items-center justify-end gap-2 px-5 pt-1 pb-5 sm:px-6">
            <Button variant="ghost" isDisabled={busy} onPress={onClose}>
              {t("common.cancel")}
            </Button>
            {primaryAction()}
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
