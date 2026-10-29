"use client";

/* Hallmark · component: Provider access settings · genre: modern-minimal · theme: existing Router system
 * states: default · hover · focus · active · disabled · loading · error · success
 * pre-emit critique: P4 H5 E4 S5 R5 V4
 */
import * as React from "react";
import { Button } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CompactSelect } from "@/components/common/compact-select";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import {
  approveMarketAccessRequest,
  getMarketAccessDashboard,
  rejectMarketAccessRequest,
  updateMarketAccessPolicy,
  updateMarketCounterparty,
  upsertMarketCounterparty,
} from "@/lib/api";
import type {
  MarketAccessDashboard,
  MarketAccessPolicy,
  MarketAccessRequest,
  MarketCounterparty,
  MarketAccessDecision,
  MarketCounterpartyAccessRule,
} from "@/lib/types";

const SCOPES = (["share", "client_host"] as const).flatMap((productKind) =>
  (["free", "paid"] as const).map((pricingKind) => ({
    productKind,
    pricingKind,
  })),
);

export function AccountMarketAccessPage({ onPendingChange }: { onPendingChange?: (count: number) => void }) {
  const { session } = useAuth();
  const actor = session?.user?.id || session?.user?.email || "anonymous";
  return <AccountMarketAccessEditor key={actor} onPendingChange={onPendingChange} />;
}

function AccountMarketAccessEditor({ onPendingChange }: { onPendingChange?: (count: number) => void }) {
  const { session } = useAuth();
  const { t } = useLocaleText();
  const [data, setData] = React.useState<MarketAccessDashboard | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const busyRef = React.useRef(false);
  const [email, setEmail] = React.useState("");
  const [search, setSearch] = React.useState("");
  const [allowed, setAllowed] = React.useState<string[]>([]);
  const [request, setRequest] = React.useState<{
    item: MarketAccessRequest;
    action: "approve" | "reject";
  } | null>(null);
  const [reason, setReason] = React.useState("");
  const [openPolicy, setOpenPolicy] = React.useState<MarketAccessPolicy | null>(
    null,
  );
  const [ack, setAck] = React.useState(false);
  const [revoke, setRevoke] = React.useState<MarketCounterparty | null>(null);
  const actor = session?.user?.id || session?.user?.email;
  React.useEffect(() => {
    if (data) onPendingChange?.(data.accessRequests.filter((item) => item.status === "requested").length);
  }, [data, onPendingChange]);
  React.useEffect(() => {
    let active = true;
    setData(null);
    setError("");
    if (session?.authenticated)
      getMarketAccessDashboard()
        .then((value) => {
          if (active) setData(value);
        })
        .catch((cause) => {
          if (active)
            setError(cause instanceof Error ? cause.message : String(cause));
        });
    return () => {
      active = false;
    };
  }, [actor, session?.authenticated]);

  const label = (
    rule: Pick<MarketCounterpartyAccessRule, "productKind" | "pricingKind">,
  ) =>
    t(
      rule.productKind === "share"
        ? "marketAccess.product.share"
        : "marketAccess.product.clientHost",
    ) +
    " · " +
    t(
      rule.pricingKind === "free"
        ? "marketAccess.pricing.free"
        : "marketAccess.pricing.paid",
    );
  const key = (
    rule: Pick<MarketCounterpartyAccessRule, "productKind" | "pricingKind">,
  ) => rule.productKind + ":" + rule.pricingKind;
  const run = async (action: () => Promise<unknown>) => {
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError("");
    try {
      await action();
      setData(await getMarketAccessDashboard());
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };
  const savePolicy = (
    policy: MarketAccessPolicy,
    mode: "whitelist" | "blacklist",
  ) => {
    if (mode === policy.mode) return;
    if (mode === "blacklist") {
      setAck(false);
      setOpenPolicy(policy);
    } else
      void run(() =>
        updateMarketAccessPolicy(policy.productKind, policy.pricingKind, {
          mode,
          expectedRevision: policy.revision,
        }),
      );
  };
  const saveRule = (
    buyer: MarketCounterparty,
    rule: (typeof SCOPES)[number],
    decision: MarketAccessDecision,
  ) => {
    const rules = SCOPES.map((scope) => ({
      ...scope,
      decision:
        key(scope) === key(rule)
          ? decision
          : buyer.accessRules.find((item) => key(item) === key(scope))
              ?.decision || ("inherit" as MarketAccessDecision),
    }));
    void run(() =>
      updateMarketCounterparty(buyer.id, {
        accessRules: rules,
        expectedRevision: buyer.revision,
      }),
    );
  };
  if (!session?.authenticated)
    return (
      <p className="text-sm text-muted-foreground">
        {t("account.signInRequired")}
      </p>
    );
  return (
    <section className="grid min-w-0 gap-7">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">
          {t("marketAccess.walletTitle")}
        </h2>
        <Button
          variant="ghost"
          isIconOnly
          aria-label={t("common.reload")}
          isDisabled={busy}
          onPress={() => void run(async () => {})}
        >
          <RefreshCw className="size-4" />
        </Button>
      </div>
      {error ? (
        <p role="alert" className="text-sm text-destructive">
          {error}
        </p>
      ) : null}
      {!data ? (
        <p role="status" className="text-sm text-muted-foreground">
          {t("common.loading")}
        </p>
      ) : (
        <>
          {/* Pending requests are the only items waiting on the Provider: list them first. */}
          {data.accessRequests.some((item) => item.status === "requested") ? (
            <div className="grid min-w-0 gap-3">
              <h3 className="text-sm font-semibold">
                {t("marketAccess.requestCount", {
                  count: data.accessRequests.filter(
                    (item) => item.status === "requested",
                  ).length,
                })}
              </h3>
              {data.accessRequests
                .filter((item) => item.status === "requested")
                .map((item) => (
                  <div
                    key={item.id}
                    className="flex min-w-0 flex-wrap items-center justify-between gap-3 rounded-md bg-muted/50 p-3"
                  >
                    <div className="min-w-0">
                      <p className="break-all text-sm font-medium">
                        {item.buyerEmail}
                      </p>
                      <p className="text-xs text-muted-foreground">
                        {label(item)} · {item.targetLabel}
                      </p>
                    </div>
                    <div className="flex flex-wrap gap-2">
                      <Button
                        variant="outline"
                        className="min-h-11 whitespace-nowrap"
                        isDisabled={busy}
                        onPress={() => {
                          setReason("");
                          setRequest({ item, action: "approve" });
                        }}
                      >
                        {t("marketAccess.approve")}
                      </Button>
                      <Button
                        variant="ghost"
                        className="min-h-11 whitespace-nowrap"
                        isDisabled={busy}
                        onPress={() => {
                          setReason("");
                          setRequest({ item, action: "reject" });
                        }}
                      >
                        {t("marketAccess.reject")}
                      </Button>
                    </div>
                  </div>
                ))}
            </div>
          ) : null}
          <div className="grid gap-3">
            <h3 className="text-sm font-semibold">
              {t("marketAccess.policyTitle")}
            </h3>
            <div className="grid gap-3 sm:grid-cols-2">
              {data.policies.map((policy) => (
                <div key={key(policy)} className="grid min-w-0 gap-1 text-sm">
                  <span>{label(policy)}</span>
                  <CompactSelect
                    ariaLabel={label(policy)} className="w-full" triggerClassName="h-11"
                    value={policy.mode}
                    disabled={busy}
                    onChange={(value) =>
                      savePolicy(
                        policy,
                        value as "whitelist" | "blacklist",
                      )
                    }
                    options={[{ value: "whitelist", label: t("marketAccess.mode.whitelist") }, { value: "blacklist", label: t("marketAccess.mode.blacklist") }]}
                  />
                </div>
              ))}
            </div>
          </div>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void run(async () => {
                if (!allowed.length)
                  throw new Error(t("marketAccess.productRequired"));
                const existing = data.counterparties.find(
                  (buyer) =>
                    buyer.buyerEmail.toLowerCase() ===
                    email.trim().toLowerCase(),
                );
                const rules = SCOPES.map((scope) => ({
                  ...scope,
                  decision: allowed.includes(key(scope))
                    ? ("allow" as const)
                    : existing?.accessRules.find(
                        (item) => key(item) === key(scope),
                      )?.decision || ("inherit" as const),
                }));
                if (existing)
                  await updateMarketCounterparty(existing.id, {
                    accessRules: rules,
                    expectedRevision: existing.revision,
                  });
                else
                  await upsertMarketCounterparty({ email, accessRules: rules });
                setEmail("");
                setAllowed([]);
              });
            }}
            className="grid min-w-0 gap-3"
          >
            <h3 className="text-sm font-semibold">
              {t("marketAccess.addTitle")}
            </h3>
            <label className="grid min-w-0 gap-1 text-sm">
              <span>{t("marketAccess.email")}</span>
              <input
                type="email"
                required
                maxLength={254}
                className={WALLET_INPUT}
                value={email}
                onChange={(event) => setEmail(event.target.value)}
                disabled={busy}
              />
            </label>
            <div className="grid gap-2 sm:grid-cols-2">
              {SCOPES.map((scope) => (
                <label
                  className="flex min-h-11 items-center gap-2 text-sm"
                  key={key(scope)}
                >
                  <input
                    type="checkbox"
                    className="size-4 accent-primary"
                    checked={allowed.includes(key(scope))}
                    disabled={busy}
                    onChange={(event) =>
                      setAllowed((current) =>
                        event.target.checked
                          ? [...current, key(scope)]
                          : current.filter((item) => item !== key(scope)),
                      )
                    }
                  />
                  {label(scope)}
                </label>
              ))}
            </div>
            <Button
              type="submit"
              variant="outline"
              className="min-h-11 justify-self-start whitespace-nowrap"
              isDisabled={busy || !email.trim() || !allowed.length}
            >
              {busy ? (
                <Loader2 className="size-4 animate-spin motion-reduce:animate-none" />
              ) : null}
              {t("common.add")}
            </Button>
          </form>
          <div className="grid min-w-0 gap-3">
            <h3 className="text-sm font-semibold">
              {t("marketAccess.buyersTitle")}
            </h3>
            <input
              type="search"
              aria-label={t("marketAccess.searchBuyers")}
              placeholder={t("marketAccess.searchBuyers")}
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              className={WALLET_INPUT}
            />
            {data.counterparties
              .filter((item) =>
                (item.buyerEmail + " " + (item.buyerUserId || ""))
                  .toLowerCase()
                  .includes(search.toLowerCase()),
              )
              .map((buyer) => (
                <div
                  key={buyer.id}
                  className="grid min-w-0 gap-3 rounded-lg bg-muted/40 p-4"
                >
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <p className="break-all text-sm font-medium">
                      {buyer.buyerEmail}
                    </p>
                    <Button
                      variant="ghost"
                      className="min-h-11 whitespace-nowrap"
                      isDisabled={busy}
                      onPress={() =>
                        buyer.status === "revoked"
                          ? void run(() =>
                              updateMarketCounterparty(buyer.id, {
                                accessRules: buyer.accessRules,
                                status: "active",
                                expectedRevision: buyer.revision,
                              }),
                            )
                          : setRevoke(buyer)
                      }
                    >
                      {t(
                        buyer.status === "revoked"
                          ? "common.enabled"
                          : "marketAccess.revoke",
                      )}
                    </Button>
                  </div>
                  <div className="grid min-w-0 gap-3 sm:grid-cols-2">
                    {SCOPES.map((scope) => (
                      <div
                        key={key(scope)}
                        className="grid min-w-0 gap-1 text-xs"
                      >
                        <span>{label(scope)}</span>
                        <CompactSelect
                          ariaLabel={label(scope)} className="w-full" triggerClassName="h-11"
                          value={
                            buyer.accessRules.find(
                              (item) => key(item) === key(scope),
                            )?.decision || "inherit"
                          }
                          disabled={busy || buyer.status === "revoked"}
                          onChange={(value) =>
                            saveRule(
                              buyer,
                              scope,
                              value as MarketAccessDecision,
                            )
                          }
                          options={(["inherit", "allow", "deny"] as const).map(value => ({ value, label: t(`marketAccess.decision.${value}`) }))}
                        />
                      </div>
                    ))}
                  </div>
                </div>
              ))}
            {!data.counterparties.length ? (
              <p className="text-sm text-muted-foreground">
                {t("marketAccess.empty")}
              </p>
            ) : null}
          </div>
        </>
      )}
      <ConfirmAlertDialog
        open={!!openPolicy}
        title={t("marketAccess.blacklistConfirmTitle")}
        description={t("walletAccess.openHelp")}
        confirmLabel={t("marketAccess.enableBlacklist")}
        cancelLabel={t("common.cancel")}
        busy={busy}
        extra={
          <>
            {error ? (
              <p role="alert" className="text-destructive">
                {error}
              </p>
            ) : null}
            <label className="flex items-start gap-2">
              <input
                type="checkbox"
                checked={ack}
                onChange={(event) => setAck(event.target.checked)}
                className="mt-1 size-4 accent-primary"
              />
              {t("walletAccess.openConfirm")}
            </label>
          </>
        }
        onConfirm={() => {
          if (!openPolicy || !ack) return;
          void run(async () => {
            await updateMarketAccessPolicy(
              openPolicy.productKind,
              openPolicy.pricingKind,
              {
                mode: "blacklist",
                riskAcknowledged: true,
                expectedRevision: openPolicy.revision,
              },
            );
            setOpenPolicy(null);
          });
        }}
        onOpenChange={(open) => {
          if (!open && !busy) setOpenPolicy(null);
        }}
      />
      <ConfirmAlertDialog
        open={!!request}
        title={t(
          request?.action === "reject"
            ? "marketAccess.rejectTitle"
            : "marketAccess.approveTitle",
        )}
        description={request?.item.buyerEmail || ""}
        confirmLabel={t(
          request?.action === "reject"
            ? "marketAccess.rejectConfirm"
            : "marketAccess.approveConfirm",
        )}
        cancelLabel={t("common.cancel")}
        busy={busy}
        extra={
          <>
            {error ? (
              <p role="alert" className="text-destructive">
                {error}
              </p>
            ) : null}
            {request?.action === "reject" ? (
              <label className="grid gap-1">
                <span>{t("marketAccess.rejectionReason")}</span>
                <input
                  className={WALLET_INPUT}
                  value={reason}
                  onChange={(event) => setReason(event.target.value)}
                  maxLength={500}
                />
              </label>
            ) : null}
          </>
        }
        onConfirm={() => {
          if (!request) return;
          void run(async () => {
            if (request.action === "reject") {
              if (!reason.trim())
                throw new Error(t("marketAccess.rejectionReasonRequired"));
              await rejectMarketAccessRequest(
                request.item.id,
                request.item.revision,
                reason.trim(),
              );
            } else
              await approveMarketAccessRequest(request.item.id, {
                expectedRevision: request.item.revision,
              });
            setRequest(null);
          });
        }}
        onOpenChange={(open) => {
          if (!open && !busy) setRequest(null);
        }}
      />
      <ConfirmAlertDialog
        open={!!revoke}
        title={t("marketAccess.revokeConfirmTitle")}
        description={revoke?.buyerEmail || ""}
        confirmLabel={t("marketAccess.revoke")}
        cancelLabel={t("common.cancel")}
        busy={busy}
        extra={
          error ? (
            <p role="alert" className="text-destructive">
              {error}
            </p>
          ) : undefined
        }
        onConfirm={() => {
          if (!revoke) return;
          void run(async () => {
            await updateMarketCounterparty(revoke.id, {
              accessRules: revoke.accessRules,
              status: "revoked",
              expectedRevision: revoke.revision,
            });
            setRevoke(null);
          });
        }}
        onOpenChange={(open) => {
          if (!open && !busy) setRevoke(null);
        }}
      />
    </section>
  );
}
