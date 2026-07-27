"use client";

import * as React from "react";
import { Button, Modal, Tabs, toast } from "@heroui/react";
import { Check, ChevronDown, Circle, Loader2, X } from "lucide-react";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  createClientMarketHost,
  getAccountPaymentProfile,
  getProvisionSshKey,
  lookupClientMarketHostIpInfo,
  testClientMarketHostSsh,
} from "@/lib/api";
import { DASHBOARD_ACCOUNT_PATH } from "@/lib/dashboard-nav";
import type { HostIpIntel, ProvisionSshKey } from "@/lib/types";
import { usePersistentState } from "@/lib/use-persistent-state";
import {
  ADD_HOST_MODE_KEY,
  ADD_HOST_SSH_KEY_OPEN_KEY,
  AddHostMode,
  IDLE_STEP_STATUS,
  StepKey,
  StepStatus,
  StepStatusMap,
  authorizedKeysInstallCommand,
  formatHostIpIntelSecondary,
  formatHostIpLocation,
  isPaymentProfileRequiredError,
  parseHostOffer,
} from "@/components/dashboard/client-market/host-utils";

export function AddHostDialog({
  open,
  onOpenChange,
  onAdded,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onAdded: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [mode, setMode] = usePersistentState<AddHostMode>(ADD_HOST_MODE_KEY, "password");
  const [sshKey, setSshKey] = React.useState<ProvisionSshKey | null>(null);
  const [sshKeyLoading, setSshKeyLoading] = React.useState(false);
  const [sshKeyOpen, setSshKeyOpen] = usePersistentState(ADD_HOST_SSH_KEY_OPEN_KEY, false);
  const [ip, setIp] = React.useState("");
  const [port, setPort] = React.useState("22");
  const [rootPassword, setRootPassword] = React.useState("");
  const [note, setNote] = React.useState("");
  const [priceUsd, setPriceUsd] = React.useState("");
  const [rentalPeriodDays, setRentalPeriodDays] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [testing, setTesting] = React.useState(false);
  const [error, setError] = React.useState("");
  const [phase, setPhase] = React.useState<"form" | "progress" | "success">("form");
  const [stepStatus, setStepStatus] = React.useState<StepStatusMap>(IDLE_STEP_STATUS);
  const [ipIntel, setIpIntel] = React.useState<HostIpIntel | null>(null);
  const [paymentReady, setPaymentReady] = React.useState<boolean | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setError("");
    setBusy(false);
    setTesting(false);
    setPhase("form");
    setStepStatus(IDLE_STEP_STATUS);
    setIpIntel(null);
    setPaymentReady(null);
    let cancelled = false;
    void getAccountPaymentProfile()
      .then((profile) => {
        if (!cancelled) setPaymentReady(profile.methods.length > 0);
      })
      .catch(() => {
        if (!cancelled) setPaymentReady(false);
      });
    setSshKeyLoading(true);
    void getProvisionSshKey()
      .then((key) => {
        if (!cancelled) setSshKey(key);
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
        }
      })
      .finally(() => {
        if (!cancelled) setSshKeyLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  const parsePort = () => {
    const parsedPort = port.trim() ? Number(port) : 22;
    if (!Number.isInteger(parsedPort) || parsedPort < 1 || parsedPort > 65535) {
      setError(t("clientMarket.invalidPort"));
      return null;
    }
    return parsedPort;
  };

  const mapHostError = (message: string) => {
    if (/cc-switch-server process is already running/i.test(message)) {
      return t("clientMarket.hostAlreadyRunning");
    }
    return message;
  };

  const markStepFailed = (prev: StepStatusMap): StepStatusMap => {
    if (prev.installKey === "running") return { ...prev, installKey: "failed" };
    if (prev.connectivity === "running") return { ...prev, connectivity: "failed" };
    if (prev.ipInfo === "running") return { ...prev, ipInfo: "failed" };
    if (prev.register === "running") return { ...prev, register: "failed" };
    return prev;
  };

  const testSsh = async () => {
    if (!ip.trim()) {
      setError(t("clientMarket.testSshNeedIp"));
      return;
    }
    if (mode === "password" && !rootPassword) {
      setError(t("clientMarket.rootPasswordRequired"));
      return;
    }
    const parsedPort = parsePort();
    if (parsedPort == null) return;
    setTesting(true);
    setError("");
    try {
      await testClientMarketHostSsh({
        ip: ip.trim(),
        port: parsedPort,
        rootPassword: mode === "password" ? rootPassword : undefined,
      });
      toast.success(t("clientMarket.testSshOk"));
    } catch (err) {
      setError(mapHostError(err instanceof Error ? err.message : String(err)));
    } finally {
      setTesting(false);
    }
  };

  const submit = async () => {
    const parsedPort = parsePort();
    if (parsedPort == null) return;
    let offer: ReturnType<typeof parseHostOffer>;
    try {
      offer = parseHostOffer(priceUsd, rentalPeriodDays, t);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    }
    if (offer.priceCents && paymentReady === false) {
      setError(t("clientMarket.offerRequiresPayment"));
      return;
    }
    if (note.length > 500) {
      setError(t("clientMarket.noteTooLong"));
      return;
    }
    if (mode === "password" && !rootPassword) {
      setError(t("clientMarket.rootPasswordRequired"));
      return;
    }
    const hostIp = ip.trim();
    setBusy(true);
    setError("");
    setPhase("progress");
    setIpIntel(null);
    try {
      if (mode === "password") {
        setStepStatus({
          installKey: "running",
          connectivity: "pending",
          ipInfo: "pending",
          register: "pending",
        });
        const host = await createClientMarketHost({
          ip: hostIp,
          port: parsedPort,
          note: note.trim() || undefined,
          rootPassword,
          ...offer,
        });
        setIpIntel(host.ipIntel || null);
        setStepStatus({
          installKey: "done",
          connectivity: "done",
          ipInfo: "done",
          register: "done",
        });
      } else {
        setStepStatus({
          installKey: "pending",
          connectivity: "running",
          ipInfo: "pending",
          register: "pending",
        });
        await testClientMarketHostSsh({ ip: hostIp, port: parsedPort });
        setStepStatus({
          installKey: "pending",
          connectivity: "done",
          ipInfo: "running",
          register: "pending",
        });

        const intel = await lookupClientMarketHostIpInfo({ ip: hostIp });
        setIpIntel(intel);
        setStepStatus({
          installKey: "pending",
          connectivity: "done",
          ipInfo: "done",
          register: "running",
        });

        await createClientMarketHost({
          ip: hostIp,
          port: parsedPort,
          note: note.trim() || undefined,
          ...offer,
        });
        setStepStatus({
          installKey: "pending",
          connectivity: "done",
          ipInfo: "done",
          register: "done",
        });
      }
      setPhase("success");
      onAdded();
    } catch (err) {
      const raw = err instanceof Error ? err.message : String(err);
      const message = isPaymentProfileRequiredError(raw)
        ? t("clientMarket.offerRequiresPayment")
        : mapHostError(raw);
      setError(message);
      setStepStatus(markStepFailed);
    } finally {
      setBusy(false);
    }
  };

  const closeDialog = (nextOpen: boolean) => {
    if (busy) return;
    onOpenChange(nextOpen);
    if (!nextOpen) {
      setIp("");
      setPort("22");
      setRootPassword("");
      setNote("");
      setPriceUsd("");
      setRentalPeriodDays("");
      setPhase("form");
      setError("");
      setIpIntel(null);
      setStepStatus(IDLE_STEP_STATUS);
    }
  };

  const installCommand = sshKey
    ? authorizedKeysInstallCommand(sshKey.authorizedKeysLine)
    : "";

  const stepMeta = (
    status: StepStatus,
  ): { label: string; icon: React.ReactNode; className: string } => {
    if (status === "running") {
      return {
        label: t("clientMarket.stepRunning"),
        icon: <Loader2 className="h-4 w-4 animate-spin text-primary" />,
        className: "border-primary/30 bg-primary/5",
      };
    }
    if (status === "done") {
      return {
        label: t("clientMarket.stepDone"),
        icon: <Check className="h-4 w-4 text-emerald-600" />,
        className: "border-emerald-200 bg-emerald-50",
      };
    }
    if (status === "failed") {
      return {
        label: t("clientMarket.stepFailed"),
        icon: <X className="h-4 w-4 text-rose-600" />,
        className: "border-rose-200 bg-rose-50",
      };
    }
    return {
      label: t("clientMarket.stepPending"),
      icon: <Circle className="h-4 w-4 text-slate-300" />,
      className: "border-border bg-white",
    };
  };

  const renderStep = (key: StepKey, title: string, detail?: React.ReactNode) => {
    const meta = stepMeta(stepStatus[key]);
    return (
      <div key={key} className={`rounded-xl border px-3 py-3 ${meta.className}`}>
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-center gap-2 text-sm font-medium text-slate-900">
            {meta.icon}
            <span>{title}</span>
          </div>
          <span className="text-xs text-muted-foreground">{meta.label}</span>
        </div>
        {detail ? <div className="mt-2 text-xs leading-5 text-slate-600">{detail}</div> : null}
      </div>
    );
  };

  const canSubmit =
    !!ip.trim() && (mode === "manual" || !!rootPassword) && !busy && !testing;

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={closeDialog}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header>
            <Modal.Heading>
              {phase === "form"
                ? t("clientMarket.addHostTitle")
                : phase === "success"
                  ? t("clientMarket.registerSuccess")
                  : t("clientMarket.registerProgressTitle")}
            </Modal.Heading>
          </Modal.Header>
          {phase === "form" ? (
            <>
              <Modal.Body className="grid gap-3 text-slate-900">
                <Tabs
                  selectedKey={mode}
                  onSelectionChange={(key: React.Key) => setMode(String(key) as AddHostMode)}
                  variant="secondary"
                  className="text-foreground"
                >
                  <Tabs.List className="grid w-full grid-cols-2 text-foreground">
                    <Tabs.Tab
                      id="password"
                      className="rounded-md border border-transparent px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors data-[selected=true]:border-primary/30 data-[selected=true]:bg-primary/10 data-[selected=true]:text-primary"
                    >
                      {t("clientMarket.tabPassword")}
                    </Tabs.Tab>
                    <Tabs.Tab
                      id="manual"
                      className="rounded-md border border-transparent px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors data-[selected=true]:border-primary/30 data-[selected=true]:bg-primary/10 data-[selected=true]:text-primary"
                    >
                      {t("clientMarket.tabManual")}
                    </Tabs.Tab>
                  </Tabs.List>
                </Tabs>

                {mode === "manual" ? (
                  <div className="overflow-hidden rounded-xl border border-border">
                    <button
                      type="button"
                      className="flex w-full items-center justify-between gap-3 px-3 py-2.5 text-left text-sm font-medium text-slate-900 transition-colors hover:bg-muted/60"
                      aria-expanded={sshKeyOpen}
                      onClick={() => setSshKeyOpen((value) => !value)}
                    >
                      <span>{t("clientMarket.addSshKeyTitle")}</span>
                      <ChevronDown
                        className={`h-4 w-4 shrink-0 text-muted-foreground transition-transform duration-200 ${
                          sshKeyOpen ? "rotate-180" : ""
                        }`}
                      />
                    </button>
                    {sshKeyOpen ? (
                      <div className="grid gap-3 border-t border-border px-3 py-3">
                        <p className="text-sm text-muted-foreground">{t("clientMarket.addSshKeyHint")}</p>
                        {sshKeyLoading ? (
                          <div className="flex items-center gap-2 text-sm text-muted-foreground">
                            <Loader2 className="h-4 w-4 animate-spin" />
                            …
                          </div>
                        ) : installCommand ? (
                          <CopyableCodeField
                            label={t("clientMarket.authorizedKeysCommand")}
                            value={installCommand}
                            copyLabel={t("clientMarket.copy")}
                            copiedLabel={t("clientMarket.copied")}
                          />
                        ) : null}
                      </div>
                    ) : null}
                  </div>
                ) : null}

                <div className="grid grid-cols-[minmax(0,1fr)_9rem] gap-3">
                  <label className="grid min-w-0 gap-1 text-sm">
                    <span className="text-muted-foreground">{t("clientMarket.hostIp")}</span>
                    <input
                      value={ip}
                      onChange={(e) => setIp(e.target.value)}
                      className="h-11 w-full rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                      autoComplete="off"
                    />
                  </label>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("clientMarket.hostPort")}</span>
                    <input
                      value={port}
                      onChange={(e) => setPort(e.target.value)}
                      className="h-11 w-full rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                      inputMode="numeric"
                      min={1}
                      max={65535}
                    />
                  </label>
                </div>
                {mode === "password" ? (
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("clientMarket.rootPassword")}</span>
                    <input
                      type="password"
                      value={rootPassword}
                      onChange={(e) => setRootPassword(e.target.value)}
                      className="h-11 rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                      autoComplete="new-password"
                    />
                    <span className="text-xs text-muted-foreground">{t("clientMarket.rootPasswordHint")}</span>
                  </label>
                ) : null}
                <div className="grid gap-3 sm:grid-cols-2">
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("clientMarket.rentalPrice")}</span>
                    <input
                      value={priceUsd}
                      onChange={(event) => setPriceUsd(event.target.value)}
                      placeholder={t("clientMarket.free")}
                      inputMode="decimal"
                      className="h-11 rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                    />
                  </label>
                  <label className="grid gap-1 text-sm">
                    <span className="text-muted-foreground">{t("clientMarket.rentalPeriod")}</span>
                    <input
                      value={rentalPeriodDays}
                      onChange={(event) => setRentalPeriodDays(event.target.value)}
                      placeholder={t("clientMarket.forever")}
                      inputMode="numeric"
                      className="h-11 rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                    />
                  </label>
                </div>
                <p className="text-xs text-muted-foreground">{t("clientMarket.offerHint")}</p>
                {paymentReady === false ? (
                  <div className="grid gap-1.5 rounded-lg border border-amber-200 bg-amber-50 px-3 py-2 text-xs text-amber-950">
                    <span>{t("clientMarket.offerRequiresPayment")}</span>
                    <a
                      href={DASHBOARD_ACCOUNT_PATH}
                      className="font-medium text-foreground underline underline-offset-2"
                    >
                      {t("clientMarket.goToAccountPayment")}
                    </a>
                  </div>
                ) : null}
                <label className="grid gap-1 text-sm">
                  <span className="text-muted-foreground">{t("clientMarket.hostNote")}</span>
                  <input
                    value={note}
                    onChange={(e) => setNote(e.target.value)}
                    className="h-11 rounded-lg border border-border bg-white px-3 text-slate-900 outline-none focus:ring-2 focus:ring-primary/30"
                    maxLength={500}
                  />
                </label>
                {error ? <p className="text-sm text-rose-600">{error}</p> : null}
              </Modal.Body>
              <Modal.Footer className="flex-wrap">
                <Button variant="ghost" isDisabled={busy || testing} onClick={() => closeDialog(false)}>
                  {t("common.close")}
                </Button>
                <Button
                  variant="outline"
                  isDisabled={busy || testing || !ip.trim() || (mode === "password" && !rootPassword)}
                  onClick={() => void testSsh()}
                >
                  {testing ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("clientMarket.testSsh")}
                </Button>
                <Button
                  variant="primary"
                  isDisabled={!canSubmit}
                  onClick={() => void submit()}
                >
                  {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                  {t("clientMarket.addHost")}
                </Button>
              </Modal.Footer>
            </>
          ) : (
            <>
              <Modal.Body className="grid gap-3 text-slate-900">
                {mode === "password" ? renderStep("installKey", t("clientMarket.stepInstallKey")) : null}
                {renderStep("connectivity", t("clientMarket.stepConnectivity"))}
                {renderStep(
                  "ipInfo",
                  t("clientMarket.stepIpInfo"),
                  ipIntel ? (
                    <div className="grid gap-1">
                      <div>
                        {t("clientMarket.ipInfoSummary", {
                          location:
                            formatHostIpLocation(
                              ipIntel,
                              ipIntel.countryCode
                                ? new Intl.DisplayNames([locale], { type: "region" }).of(ipIntel.countryCode) ||
                                    ipIntel.countryCode
                                : ipIntel.query,
                              locale,
                            ) || ipIntel.query,
                          countryCode: ipIntel.countryCode,
                        })}
                      </div>
                      {formatHostIpIntelSecondary(ipIntel, t).map((line) => (
                        <div key={line}>{line}</div>
                      ))}
                    </div>
                  ) : null,
                )}
                {renderStep("register", t("clientMarket.stepRegister"))}
                {error ? <p className="text-sm text-rose-600">{error}</p> : null}
              </Modal.Body>
              <Modal.Footer>
                {phase === "success" || error ? (
                  <Button
                    variant="primary"
                    onClick={() => {
                      if (phase === "success") {
                        closeDialog(false);
                      } else {
                        setPhase("form");
                        setError("");
                        setStepStatus(IDLE_STEP_STATUS);
                      }
                    }}
                  >
                    {phase === "success" ? t("common.close") : t("clientMarket.back")}
                  </Button>
                ) : (
                  <Button variant="ghost" isDisabled>
                    <Loader2 className="h-4 w-4 animate-spin" />
                    {t("clientMarket.stepRunning")}
                  </Button>
                )}
              </Modal.Footer>
            </>
          )}
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
