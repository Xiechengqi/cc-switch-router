"use client";

import * as React from "react";
import { Button, Chip, Dropdown, Modal, Tabs, toast } from "@heroui/react";
import { Check, ChevronDown, Circle, Loader2, MoreHorizontal, Plus, RefreshCw, TerminalSquare, Trash2, X } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CountryFlag } from "@/components/common/country-flag";
import { ClientMarketTerminalDialog } from "@/components/dashboard/client-market-terminal-dialog";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cleanupClientMarketClient,
  createClientMarketHost,
  deleteClientMarketHost,
  getClientMarketHosts,
  getClientMarketJob,
  getProvisionSshKey,
  lookupClientMarketHostIpInfo,
  reverifyClientMarketHost,
  testClientMarketHostSsh,
} from "@/lib/api";
import type { ClientMarketHost, HostIpIntel, ProvisionSshKey } from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";
import { usePersistentState } from "@/lib/use-persistent-state";

const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";
const ADD_HOST_SSH_KEY_OPEN_KEY = "cc-switch.client-market.add-host.ssh-key-open";
const ADD_HOST_MODE_KEY = "cc-switch.client-market.add-host.mode";

type AddHostMode = "password" | "manual";
type StepKey = "installKey" | "connectivity" | "ipInfo" | "register";
type StepStatus = "pending" | "running" | "done" | "failed";
type StepStatusMap = Record<StepKey, StepStatus>;

const IDLE_STEP_STATUS: StepStatusMap = {
  installKey: "pending",
  connectivity: "pending",
  ipInfo: "pending",
  register: "pending",
};

const IP_RISK_LABEL_KEYS: Record<string, MessageKey> = {
  中性: "clientMarket.ipRisk.neutral",
  轻微风险: "clientMarket.ipRisk.low",
  低风险: "clientMarket.ipRisk.low",
  稍高风险: "clientMarket.ipRisk.elevated",
  中风险: "clientMarket.ipRisk.medium",
  高风险: "clientMarket.ipRisk.high",
  极高风险: "clientMarket.ipRisk.critical",
  风险: "clientMarket.ipRisk.risky",
  neutral: "clientMarket.ipRisk.neutral",
  low: "clientMarket.ipRisk.low",
  "low risk": "clientMarket.ipRisk.low",
  elevated: "clientMarket.ipRisk.elevated",
  medium: "clientMarket.ipRisk.medium",
  high: "clientMarket.ipRisk.high",
  critical: "clientMarket.ipRisk.critical",
  risky: "clientMarket.ipRisk.risky",
};

const IP_CLASS_LABEL_KEYS: Record<string, MessageKey> = {
  "IDC 机房 IP": "clientMarket.ipClass.idc",
  "IDC机房IP": "clientMarket.ipClass.idc",
  数据中心: "clientMarket.ipClass.datacenter",
  "住宅 IP": "clientMarket.ipClass.residential",
  住宅IP: "clientMarket.ipClass.residential",
  "VPN 出口节点": "clientMarket.ipClass.vpnExit",
  VPN出口节点: "clientMarket.ipClass.vpnExit",
  代理: "clientMarket.ipClass.proxy",
  VPN: "clientMarket.ipClass.vpn",
  托管: "clientMarket.ipClass.hosting",
  Tor: "clientMarket.ipClass.tor",
  business: "clientMarket.ipClass.business",
  hosting: "clientMarket.ipClass.hosting",
  datacenter: "clientMarket.ipClass.datacenter",
  residential: "clientMarket.ipClass.residential",
  proxy: "clientMarket.ipClass.proxy",
  vpn: "clientMarket.ipClass.vpn",
  tor: "clientMarket.ipClass.tor",
  idc: "clientMarket.ipClass.idc",
};

function containsCjk(value: string) {
  return /[\u3400-\u9fff]/.test(value);
}

function translateMappedLabel(
  raw: string | undefined,
  map: Record<string, MessageKey>,
  t: (key: MessageKey) => string,
): string | null {
  const value = raw?.trim();
  if (!value) return null;
  const key = map[value] || map[value.toLowerCase()];
  return key ? t(key) : null;
}

function formatHostIpIntelSecondary(
  intel: HostIpIntel | undefined,
  t: (key: MessageKey) => string,
): string[] {
  if (!intel) return [];
  const parts: string[] = [];
  const ispAsn = [intel.isp || intel.asName, intel.asn].filter(Boolean).join(" · ");
  if (ispAsn) parts.push(ispAsn);

  const risk = translateMappedLabel(intel.riskLevel, IP_RISK_LABEL_KEYS, t);
  if (risk) parts.push(risk);

  const classification =
    translateMappedLabel(intel.classificationType, IP_CLASS_LABEL_KEYS, t) ||
    translateMappedLabel(intel.networkType, IP_CLASS_LABEL_KEYS, t) ||
    (intel.vpn ? t("clientMarket.ipClass.vpn") : null) ||
    (intel.hosting ? t("clientMarket.ipClass.hosting") : null) ||
    (intel.proxy ? t("clientMarket.ipClass.proxy") : null) ||
    (intel.tor ? t("clientMarket.ipClass.tor") : null);
  if (classification) parts.push(classification);

  return parts;
}

function formatHostIpLocation(
  intel: HostIpIntel | undefined,
  countryName: string,
  locale: string,
): string {
  if (!intel) return countryName;
  const preferLatin = locale.toLowerCase().startsWith("en");
  if (intel.location && !(preferLatin && containsCjk(intel.location))) {
    return intel.location;
  }
  const parts = [intel.city, intel.region, intel.country || countryName]
    .filter((part): part is string => !!part && !(preferLatin && containsCjk(part)));
  if (parts.length) return parts.join(" · ");
  return countryName;
}

function statusLabelKey(status: string): MessageKey {
  const known = {
    idle: "clientMarket.status.idle",
    allocated: "clientMarket.status.allocated",
    locked: "clientMarket.status.locked",
    draining: "clientMarket.status.draining",
    disabled: "clientMarket.status.disabled",
    unreachable: "clientMarket.status.unreachable",
    abnormal: "clientMarket.status.abnormal",
  } as const;
  return (known[status as keyof typeof known] || "clientMarket.status.idle") as MessageKey;
}

function authorizedKeysInstallCommand(line: string): string {
  const escaped = line.replace(/'/g, `'\\''`);
  return `echo '${escaped}' >> $HOME/.ssh/authorized_keys`;
}

function AddHostDialog({
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
  const [busy, setBusy] = React.useState(false);
  const [testing, setTesting] = React.useState(false);
  const [error, setError] = React.useState("");
  const [phase, setPhase] = React.useState<"form" | "progress" | "success">("form");
  const [stepStatus, setStepStatus] = React.useState<StepStatusMap>(IDLE_STEP_STATUS);
  const [ipIntel, setIpIntel] = React.useState<HostIpIntel | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setError("");
    setBusy(false);
    setTesting(false);
    setPhase("form");
    setStepStatus(IDLE_STEP_STATUS);
    setIpIntel(null);
    let cancelled = false;
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
      const message = mapHostError(err instanceof Error ? err.message : String(err));
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

function HostRow({
  host,
  viewerEmail,
  isAdmin,
  onChanged,
}: {
  host: ClientMarketHost;
  viewerEmail?: string;
  isAdmin: boolean;
  onChanged: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [busy, setBusy] = React.useState(false);
  const [confirmAction, setConfirmAction] = React.useState<"delete" | "cleanup" | null>(null);
  const [terminalOpen, setTerminalOpen] = React.useState(false);
  const canManageHost =
    !!viewerEmail &&
    (isAdmin || viewerEmail.toLowerCase() === host.hostOwnerEmail.toLowerCase());
  const canDelete =
    canManageHost &&
    (host.status === "idle" || host.status === "disabled" || host.status === "abnormal");
  const isClientOwner =
    !!viewerEmail &&
    !!host.clientOwnerEmail &&
    viewerEmail.toLowerCase() === host.clientOwnerEmail.toLowerCase();
  const canCleanup =
    !!host.installationId &&
    (host.status === "allocated" || host.status === "unreachable") &&
    (isAdmin || isClientOwner);
  const canReverify =
    canManageHost &&
    (isAdmin || !host.installationId) &&
    (host.status === "unreachable" || host.status === "disabled" || host.status === "abnormal");
  const canOpenTerminal = host.canWebTerminal === true || canManageHost;
  const countryName = host.countryCode
    ? new Intl.DisplayNames([locale], { type: "region" }).of(host.countryCode) || host.countryCode
    : "";

  const pollJob = async (jobId: string) => {
    for (let i = 0; i < 120; i++) {
      await new Promise((r) => setTimeout(r, 1500));
      let job;
      try {
        job = await getClientMarketJob(jobId);
      } catch {
        continue;
      }
      if (job.status === "succeeded") break;
      if (job.status === "failed") {
        toast.danger(t("clientMarket.cleanupFailed"));
        break;
      }
    }
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

  const onCleanup = async () => {
    if (!host.installationId) return;
    setConfirmAction(null);
    setBusy(true);
    try {
      const { jobId } = await cleanupClientMarketClient(host.installationId);
      toast.info(t("clientMarket.cleanupStarted"));
      await pollJob(jobId);
    } catch (err) {
      toast.danger(err instanceof Error ? err.message : String(err));
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

  const hostLabel = host.hostname || host.ip || host.id.slice(0, 8);
  const confirmCopy = confirmAction === "cleanup"
    ? {
        title: t("clientMarket.cleanupConfirmTitle"),
        description: t("clientMarket.cleanupConfirmDesc", { host: hostLabel }),
        confirmLabel: t("clientMarket.cleanup"),
      }
    : confirmAction === "delete"
      ? {
          title: t("clientMarket.deleteHostConfirmTitle"),
          description: t("clientMarket.deleteHostConfirmDesc", { host: hostLabel }),
          confirmLabel: t("clientMarket.deleteHost"),
        }
      : null;
  const hasActions = canDelete || canCleanup || canReverify;
  const ipPort = host.ip ? `${host.ip}${host.port ? `:${host.port}` : ""}` : "";
  const intel = host.ipIntel;
  const locationLabel = formatHostIpLocation(intel, countryName, locale);
  const secondaryIntelParts = formatHostIpIntelSecondary(intel, t);
  const subdomain = host.clientSubdomain?.trim() || "";

  return (
    <>
      <div className="grid gap-1.5 rounded-lg border border-border bg-white px-3 py-2.5 text-sm">
        <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
          <Chip size="sm" variant="soft" className="shrink-0">
            {t(statusLabelKey(host.status))}
          </Chip>
          {canOpenTerminal ? (
            <Button
              variant="outline"
              size="sm"
              className="shrink-0"
              onClick={() => setTerminalOpen(true)}
            >
              <TerminalSquare className="h-3.5 w-3.5" />
              {t("clientMarket.webTerminal")}
            </Button>
          ) : null}
          {locationLabel || host.countryCode ? (
            <span className="inline-flex min-w-0 max-w-[14rem] items-center gap-1.5 text-xs text-muted-foreground">
              <CountryFlag code={host.countryCode} className="h-3.5 w-5 shrink-0 rounded-sm object-cover" />
              {locationLabel ? (
                <span className="truncate" title={locationLabel}>
                  {locationLabel}
                </span>
              ) : null}
            </span>
          ) : null}
          <span
            className="min-w-0 max-w-[16rem] truncate text-xs font-medium text-foreground"
            title={host.hostOwnerEmail}
          >
            {host.hostOwnerEmail}
          </span>
          {subdomain ? (
            <span
              className="min-w-0 flex-1 truncate font-mono text-xs text-muted-foreground"
              title={host.installationId || host.hostname || undefined}
            >
              {subdomain}
            </span>
          ) : (
            <span className="min-w-0 flex-1" aria-hidden />
          )}
          {ipPort ? (
            <span className="shrink-0 font-mono text-xs text-foreground" title={host.hostname || undefined}>
              {ipPort}
            </span>
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
                  {canReverify ? (
                    <Dropdown.Item id="reverify" onAction={() => void onReverify()}>
                      <RefreshCw className="h-4 w-4" />
                      {t("clientMarket.reverifyHost")}
                    </Dropdown.Item>
                  ) : null}
                  {canCleanup ? (
                    <Dropdown.Item id="cleanup" onAction={() => setConfirmAction("cleanup")}>
                      {t("clientMarket.cleanup")}
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
          ) : (
            <span className="h-8 w-8 shrink-0" aria-hidden />
          )}
        </div>
        {secondaryIntelParts.length || host.note ? (
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 pl-0.5 text-[11px] leading-4 text-muted-foreground">
            {secondaryIntelParts.length ? (
              <span className="whitespace-normal break-words">{secondaryIntelParts.join(" · ")}</span>
            ) : null}
            {host.note ? (
              <span className="min-w-0 whitespace-normal break-words" title={host.note}>
                {host.note}
              </span>
            ) : null}
          </div>
        ) : null}
      </div>
      {confirmCopy ? (
        <ConfirmAlertDialog
          open
          title={confirmCopy.title}
          description={confirmCopy.description}
          confirmLabel={confirmCopy.confirmLabel}
          cancelLabel={t("common.cancel")}
          tone="danger"
          busy={busy}
          onConfirm={() => {
            if (confirmAction === "cleanup") void onCleanup();
            else void onDelete();
          }}
          onOpenChange={(nextOpen) => {
            if (!nextOpen && !busy) setConfirmAction(null);
          }}
        />
      ) : null}
      <ClientMarketTerminalDialog
        open={terminalOpen}
        hostId={terminalOpen ? host.id : null}
        hostLabel={hostLabel}
        onOpenChange={setTerminalOpen}
      />
    </>
  );
}

const OWNER_FILTER_KEY = "cc_switch_router_client_market_owner_filter_v1";
const REGION_FILTER_KEY = "cc_switch_router_client_market_region_filter_v1";

export function ClientMarketPage() {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const viewerEmail = session?.user?.email;
  const isAdmin = !!session?.isAdmin;

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [addOpen, setAddOpen] = React.useState(false);
  const [pendingAddAfterLogin, setPendingAddAfterLogin] = React.useState(false);
  const [mineOnly, setMineOnly] = React.useState(false);
  const [ownerFilters, setOwnerFilters] = usePersistentState<string[]>(OWNER_FILTER_KEY, []);
  const [regionFilters, setRegionFilters] = usePersistentState<string[]>(REGION_FILTER_KEY, []);
  const [error, setError] = React.useState("");

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      setHosts(await getClientMarketHosts());
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [isAdmin, load, viewerEmail]);

  const ownerOptions = React.useMemo(() => {
    const emails = Array.from(new Set(hosts.map((host) => host.hostOwnerEmail))).sort((a, b) =>
      a.localeCompare(b),
    );
    return emails.map((email) => ({ value: email, label: email }));
  }, [hosts]);

  const regionOptions = React.useMemo(() => {
    const regionNames = new Intl.DisplayNames([locale], { type: "region" });
    const codes = Array.from(
      new Set(
        hosts
          .map((host) => (host.countryCode || "").trim().toUpperCase())
          .filter(Boolean),
      ),
    ).sort((a, b) => a.localeCompare(b));
    return codes.map((code) => ({
      value: code,
      label: regionNames.of(code) || code,
    }));
  }, [hosts, locale]);

  const visibleHosts = React.useMemo(() => {
    const ownerSet = new Set(ownerFilters.map((email) => email.toLowerCase()));
    const regionSet = new Set(regionFilters.map((code) => code.toUpperCase()));
    return hosts
      .filter((host) => {
        if (mineOnly && viewerEmail) {
          if (host.hostOwnerEmail.toLowerCase() !== viewerEmail.toLowerCase()) return false;
        }
        if (ownerSet.size > 0 && !ownerSet.has(host.hostOwnerEmail.toLowerCase())) return false;
        if (regionSet.size > 0) {
          const code = (host.countryCode || "").trim().toUpperCase();
          if (!code || !regionSet.has(code)) return false;
        }
        return true;
      })
      .sort((a, b) => {
        const ownerCmp = a.hostOwnerEmail.localeCompare(b.hostOwnerEmail);
        if (ownerCmp !== 0) return ownerCmp;
        const ipCmp = (a.ip || "").localeCompare(b.ip || "");
        if (ipCmp !== 0) return ipCmp;
        return a.id.localeCompare(b.id);
      });
  }, [hosts, mineOnly, ownerFilters, regionFilters, viewerEmail]);

  React.useEffect(() => {
    if (!pendingAddAfterLogin || !authed) return;
    setPendingAddAfterLogin(false);
    setAddOpen(true);
  }, [authed, pendingAddAfterLogin]);

  const openAddHost = () => {
    if (!authed) {
      setPendingAddAfterLogin(true);
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    setAddOpen(true);
  };

  return (
    <div className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-5 pb-10">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <CompactRegionMultiSelect
            values={ownerFilters}
            onChange={setOwnerFilters}
            options={ownerOptions}
            allLabel={t("clientMarket.allOwners")}
            moreLabel={(count) => t("clientMarket.ownersMore", { count })}
            clearLabel={t("clientMarket.clearOwnerSelection")}
            ariaLabel={t("clientMarket.filterOwners")}
            className="w-full sm:w-56"
          />
          <CompactRegionMultiSelect
            values={regionFilters}
            onChange={setRegionFilters}
            options={regionOptions}
            allLabel={t("clientMarket.allRegions")}
            moreLabel={(count) => t("clientMarket.regionsMore", { count })}
            clearLabel={t("clientMarket.clearRegionSelection")}
            ariaLabel={t("clientMarket.filterRegions")}
            className="w-full sm:w-44"
          />
        </div>
        <div className="flex items-center gap-2">
          {authed ? (
            <Button
              variant={mineOnly ? "primary" : "outline"}
              size="sm"
              onClick={() => setMineOnly((value) => !value)}
            >
              {mineOnly ? t("clientMarket.allHosts") : t("clientMarket.myHosts")}
            </Button>
          ) : null}
          <Button variant="outline" size="sm" onClick={openAddHost}>
            <Plus className="h-4 w-4" />
            {t("clientMarket.addHost")}
          </Button>
        </div>
      </div>

      {!authed ? (
        <p className="text-sm text-muted-foreground">{t("clientMarket.loginToAddHost")}</p>
      ) : null}

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          …
        </div>
      ) : error ? (
        <p className="text-sm text-rose-600">{error}</p>
      ) : visibleHosts.length === 0 ? (
        <p className="rounded-lg border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
          {t("clientMarket.noHosts")}
        </p>
      ) : (
        <div className="grid gap-2">
          {visibleHosts.map((host) => (
            <HostRow
              key={host.id}
              host={host}
              viewerEmail={viewerEmail}
              isAdmin={isAdmin}
              onChanged={() => void load()}
            />
          ))}
        </div>
      )}

      <AddHostDialog open={addOpen} onOpenChange={setAddOpen} onAdded={() => void load()} />
    </div>
  );
}
