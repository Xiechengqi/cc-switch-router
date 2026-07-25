"use client";

import * as React from "react";
import { createPortal } from "react-dom";
import { Button, Checkbox, Chip, Dropdown, Modal, Tabs, toast, Tooltip } from "@heroui/react";
import { ArrowDown, ArrowUp, Check, CheckSquare, ChevronDown, ChevronLeft, ChevronRight, Circle, Clock3, Download, Filter, Loader2, MoreHorizontal, Pencil, Plus, RefreshCw, Trash2, Upload, X } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CountryFlag } from "@/components/common/country-flag";
import { PaymentMethodIcons } from "@/components/common/payment-method-icons";
import { CreateClientDialog } from "@/components/dashboard/create-client-dialog";
import { ProvisionJobLog } from "@/components/dashboard/provision-job-log";
import { WebTerminalGlyph } from "@/components/dashboard/web-terminal/web-terminal-glyph";
import { useWebTerminal } from "@/components/dashboard/web-terminal";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cleanupClientMarketClientWithReason,
  createClientMarketHost,
  deleteClientMarketHost,
  exportMyClientMarketHosts,
  getAccountPaymentProfile,
  getClientMarketProviderSupply,
  getClientMarketHosts,
  getClientMarketJob,
  getMyClientMarketBilling,
  getProvisionSshKey,
  importMyClientMarketHosts,
  lookupClientMarketHostIpInfo,
  reverifyClientMarketHost,
  testClientMarketHostSsh,
  updateClientMarketHostOffer,
} from "@/lib/api";
import { DASHBOARD_ACCOUNT_PATH } from "@/lib/dashboard-nav";
import type { ClientMarketBilling, ClientMarketHost, ClientMarketHostImportResponse, ClientMarketProvider, HostIpIntel, ProvisionSshKey, ProvisioningJob } from "@/lib/types";
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

function formatObservationRate(value: number | undefined, locale: string) {
  if (value == null) return "-";
  return new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 1 }).format(value);
}

function formatObservationDate(value: string, locale: string) {
  return new Intl.DateTimeFormat(locale, { dateStyle: "medium" }).format(new Date(value));
}

function formatProviderPriceRange(provider: ClientMarketProvider, locale: string) {
  if (provider.minPriceCents == null || provider.maxPriceCents == null) return "-";
  const format = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" });
  const min = format.format(provider.minPriceCents / 100);
  const max = format.format(provider.maxPriceCents / 100);
  return min === max ? min : `${min}-${max}`;
}

function formatProviderPeriodRange(provider: ClientMarketProvider) {
  if (provider.minRentalPeriodDays == null || provider.maxRentalPeriodDays == null) return "-";
  return provider.minRentalPeriodDays === provider.maxRentalPeriodDays
    ? String(provider.minRentalPeriodDays)
    : `${provider.minRentalPeriodDays}-${provider.maxRentalPeriodDays}`;
}

function hostDisplayLabel(host: ClientMarketHost) {
  return host.hostname || host.ip || host.id.slice(0, 8);
}

function hostCanManage(host: ClientMarketHost, isAdmin: boolean) {
  return isAdmin || host.isHostOwner === true;
}

function hostCanCleanup(host: ClientMarketHost, isAdmin: boolean) {
  return (
    !!host.installationId &&
    (host.status === "allocated" || host.status === "unreachable" || host.status === "draining") &&
    (hostCanManage(host, isAdmin) || host.isClientOwner === true)
  );
}

function hostCanReverify(host: ClientMarketHost, isAdmin: boolean) {
  return (
    hostCanManage(host, isAdmin) &&
    (host.status === "unreachable" || host.status === "disabled" || host.status === "abnormal")
  );
}

function hostCanDelete(host: ClientMarketHost, isAdmin: boolean) {
  return (
    hostCanManage(host, isAdmin) &&
    !host.installationId &&
    (host.status === "idle" || host.status === "disabled" || host.status === "abnormal")
  );
}

function hostCanExport(host: ClientMarketHost) {
  return host.isHostOwner === true && !!host.ip && host.port != null;
}

function hostExportKey(host: { ip?: string | null; port?: number | null }) {
  if (!host.ip || host.port == null) return "";
  return `${host.ip}:${host.port}`;
}

function cleanupReasonForHost(host: ClientMarketHost, isAdmin: boolean) {
  const isClientOwner = host.isClientOwner === true;
  if (host.isHostOwner === true && !isClientOwner) return "provider_release" as const;
  if (isAdmin && !isClientOwner) return "operator_release" as const;
  return "client_release" as const;
}

type BatchItemStatus = "queued" | "running" | "succeeded" | "failed" | "skipped";
type BatchProgressItem = {
  hostId: string;
  label: string;
  status: BatchItemStatus;
  detail?: string;
};

async function mapPool<T, R>(items: T[], concurrency: number, fn: (item: T, index: number) => Promise<R>): Promise<R[]> {
  const results = new Array<R>(items.length);
  let cursor = 0;
  const workers = Array.from({ length: Math.min(Math.max(concurrency, 1), Math.max(items.length, 1)) }, async () => {
    while (true) {
      const index = cursor;
      cursor += 1;
      if (index >= items.length) return;
      results[index] = await fn(items[index], index);
    }
  });
  await Promise.all(workers);
  return results;
}

function countBatchStatuses(items: BatchProgressItem[]) {
  let succeeded = 0;
  let skipped = 0;
  let failed = 0;
  for (const item of items) {
    if (item.status === "succeeded") succeeded += 1;
    else if (item.status === "skipped") skipped += 1;
    else if (item.status === "failed") failed += 1;
  }
  return { succeeded, skipped, failed };
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
    reserved: "clientMarket.status.reserved",
    allocated: "clientMarket.status.allocated",
    locked: "clientMarket.status.locked",
    draining: "clientMarket.status.draining",
    disabled: "clientMarket.status.disabled",
    unreachable: "clientMarket.status.unreachable",
    abnormal: "clientMarket.status.abnormal",
  } as const;
  return (known[status as keyof typeof known] || "clientMarket.status.idle") as MessageKey;
}

const HOST_STATUS_GROUPS = ["all", "idle", "in_use", "needs_attention"] as const;
type HostStatusFilter = (typeof HOST_STATUS_GROUPS)[number];

const STATUS_GROUP_MEMBERS: Record<Exclude<HostStatusFilter, "all">, readonly string[]> = {
  idle: ["idle"],
  in_use: ["allocated", "locked", "reserved"],
  needs_attention: ["draining", "unreachable", "abnormal", "disabled"],
};

function statusGroupForHost(status: string): Exclude<HostStatusFilter, "all"> | null {
  const normalized = status.trim().toLowerCase();
  for (const group of ["idle", "in_use", "needs_attention"] as const) {
    if (STATUS_GROUP_MEMBERS[group].includes(normalized)) return group;
  }
  return null;
}

function hostMatchesStatusFilter(status: string, filter: HostStatusFilter) {
  if (filter === "all") return true;
  return statusGroupForHost(status) === filter;
}

function statusGroupLabelKey(group: HostStatusFilter): MessageKey {
  return `clientMarket.statusGroup.${group}` as MessageKey;
}

function statusGroupHintKey(group: HostStatusFilter): MessageKey {
  return `clientMarket.statusGroupHint.${group}` as MessageKey;
}

function fineStatusHintKey(status: string): MessageKey | null {
  const known = {
    idle: "clientMarket.statusHint.idle",
    reserved: "clientMarket.statusHint.reserved",
    allocated: "clientMarket.statusHint.allocated",
    locked: "clientMarket.statusHint.locked",
    draining: "clientMarket.statusHint.draining",
    disabled: "clientMarket.statusHint.disabled",
    unreachable: "clientMarket.statusHint.unreachable",
    abnormal: "clientMarket.statusHint.abnormal",
  } as const;
  return known[status as keyof typeof known] ?? null;
}

function authorizedKeysInstallCommand(line: string): string {
  const escaped = line.replace(/'/g, `'\\''`);
  return `echo '${escaped}' >> $HOME/.ssh/authorized_keys`;
}

type Translate = (key: MessageKey, values?: Record<string, string | number>) => string;

function formatHostOffer(priceCents: number | undefined, rentalPeriodDays: number | undefined, locale: string) {
  if (!priceCents || !rentalPeriodDays) return locale.startsWith("zh") ? "免费 · 永久" : "Free · forever";
  const amount = new Intl.NumberFormat(locale, { style: "currency", currency: "USD" }).format(priceCents / 100);
  return locale.startsWith("zh") ? `${amount} · ${rentalPeriodDays} 天` : `${amount} · ${rentalPeriodDays}d`;
}

function compactCountdown(value: string, locale: string) {
  const remainingMinutes = Math.max(0, Math.ceil((Date.parse(value) - Date.now()) / 60_000));
  const days = Math.floor(remainingMinutes / 1_440);
  const hours = Math.floor((remainingMinutes % 1_440) / 60);
  const minutes = remainingMinutes % 60;
  if (locale.startsWith("zh")) {
    if (days) return `${days}天 ${hours}小时`;
    if (hours) return `${hours}小时 ${minutes}分钟`;
    return `${minutes}分钟`;
  }
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}

function HostBillingCountdown({ billing }: { billing?: ClientMarketBilling }) {
  const { locale, t } = useLocaleText();
  const [, tick] = React.useState(0);
  const target = billing?.status === "payment_due" ? billing.paymentDeadline : billing?.currentPeriodEnd;

  React.useEffect(() => {
    if (!target) return;
    const timer = window.setInterval(() => tick((value) => value + 1), 30_000);
    return () => window.clearInterval(timer);
  }, [target]);

  if (!billing || !target || !billing.priceCents) return null;
  const key = billing.status === "payment_due" ? "clientMarket.paymentDueCountdown" : "clientMarket.nextBillCountdown";
  return (
    <span className={`inline-flex shrink-0 items-center gap-1 text-xs ${billing.status === "payment_due" ? "text-amber-700" : "text-muted-foreground"}`} title={new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(target))}>
      <Clock3 className="h-3.5 w-3.5" />
      {t(key, { countdown: compactCountdown(target, locale) })}
    </span>
  );
}

function parseHostOffer(priceUsd: string, periodDays: string, t: Translate) {
  const price = priceUsd.trim();
  const period = periodDays.trim();
  if (!price && !period) return { priceCents: undefined, rentalPeriodDays: undefined };
  if (!price || !period || !/^\d{1,7}(?:\.\d{1,2})?$/.test(price) || !/^\d+$/.test(period)) {
    throw new Error(t("clientMarket.offerInvalid"));
  }
  const [whole, fraction = ""] = price.split(".");
  const priceCents = Number(whole) * 100 + Number(fraction.padEnd(2, "0"));
  const rentalPeriodDays = Number(period);
  if (priceCents < 1 || priceCents > 100_000_000 || rentalPeriodDays < 4 || rentalPeriodDays > 3_650) {
    throw new Error(t("clientMarket.offerRange"));
  }
  return { priceCents, rentalPeriodDays };
}

function isPaymentProfileRequiredError(message: string) {
  return message.toLowerCase().includes("configure payment details on the account page");
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

function cleanupPhaseLabelKey(phase: string): MessageKey {
  switch (phase) {
    case "cleanup_stop":
      return "clientMarket.cleanupPhase.stop";
    case "cleanup_wipe":
      return "clientMarket.cleanupPhase.wipe";
    case "cleanup_purge":
      return "clientMarket.cleanupPhase.purge";
    case "complete":
      return "clientMarket.cleanupPhase.complete";
    case "cleanup_remote":
    default:
      return "clientMarket.cleanupPhase.remote";
  }
}

function cleanupFailureGuidanceKey(failureCode?: string): MessageKey {
  if (!failureCode) return "clientMarket.cleanupFailedGuidance";
  if (failureCode.startsWith("cleanup_purge_failed")) return "clientMarket.cleanupFailedGuidance.purge";
  if (
    failureCode.startsWith("cleanup_ssh_timeout") ||
    failureCode.startsWith("cleanup_stop_failed") ||
    failureCode.startsWith("cleanup_wipe_failed")
  ) {
    return "clientMarket.cleanupFailedGuidance.remote";
  }
  if (
    failureCode.startsWith("cleanup_fingerprint_mismatch") ||
    failureCode.startsWith("cleanup_host_binding_mismatch")
  ) {
    return "clientMarket.cleanupFailedGuidance.safety";
  }
  return "clientMarket.cleanupFailedGuidance";
}

/** Human next-step copy for host list — never surface raw failure codes. */
function hostStatusGuidanceKey(status: string, lastError?: string): MessageKey | null {
  const group = statusGroupForHost(status);
  if (group !== "needs_attention") return null;

  const code = (lastError || "").trim().toLowerCase();
  if (
    code.startsWith("provisioning_failed") ||
    code.startsWith("installer_failed") ||
    code.includes("provisioning failed")
  ) {
    return "clientMarket.hostErrorGuidance.provisioningFailed";
  }
  if (code.startsWith("rollback_failed") || code.includes("operator verification")) {
    return "clientMarket.hostErrorGuidance.rollbackFailed";
  }
  if (
    code.includes("already running") ||
    code.includes("cc-switch-server process")
  ) {
    return "clientMarket.hostErrorGuidance.abnormalProcess";
  }
  if (code.startsWith("cleanup_") || code.startsWith("cleanup ")) {
    return cleanupFailureGuidanceKey(lastError);
  }
  if (status === "draining") return "clientMarket.statusHint.draining";
  if (status === "disabled") return "clientMarket.statusHint.disabled";
  if (status === "abnormal") return "clientMarket.statusHint.abnormal";
  if (status === "unreachable") {
    return lastError
      ? "clientMarket.hostErrorGuidance.generic"
      : "clientMarket.statusHint.unreachable";
  }
  return lastError ? "clientMarket.hostErrorGuidance.generic" : null;
}

function HostOfferDialog({
  host,
  open,
  onOpenChange,
  onSaved,
}: {
  host: ClientMarketHost;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const [price, setPrice] = React.useState(host.priceCents ? (host.priceCents / 100).toFixed(2) : "");
  const [period, setPeriod] = React.useState(host.rentalPeriodDays ? String(host.rentalPeriodDays) : "");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [paymentReady, setPaymentReady] = React.useState<boolean | null>(null);

  React.useEffect(() => {
    if (!open) return;
    setPrice(host.priceCents ? (host.priceCents / 100).toFixed(2) : "");
    setPeriod(host.rentalPeriodDays ? String(host.rentalPeriodDays) : "");
    setError("");
    setPaymentReady(null);
    let cancelled = false;
    void getAccountPaymentProfile()
      .then((profile) => {
        if (!cancelled) setPaymentReady(profile.methods.length > 0);
      })
      .catch(() => {
        if (!cancelled) setPaymentReady(false);
      });
    return () => {
      cancelled = true;
    };
  }, [host.priceCents, host.rentalPeriodDays, open]);

  const save = async () => {
    let offer: ReturnType<typeof parseHostOffer>;
    try {
      offer = parseHostOffer(price, period, t);
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return;
    }
    if (offer.priceCents && paymentReady === false) {
      setError(t("clientMarket.offerRequiresPayment"));
      return;
    }
    setBusy(true);
    setError("");
    try {
      await updateClientMarketHostOffer(host.id, offer);
      toast.success(t("clientMarket.offerUpdated"));
      onSaved();
      onOpenChange(false);
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason);
      setError(isPaymentProfileRequiredError(message) ? t("clientMarket.offerRequiresPayment") : message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={(next) => !busy && onOpenChange(next)}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light w-[min(460px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
          <Modal.Header><Modal.Heading>{t("clientMarket.editOffer")}</Modal.Heading></Modal.Header>
          <Modal.Body className="grid gap-4">
            <p className="text-sm text-muted-foreground">{t("clientMarket.editOfferHint")}</p>
            {paymentReady === false ? (
              <div className="grid gap-2 rounded-lg border border-amber-200 bg-amber-50 px-3 py-3 text-sm text-amber-950">
                <p>{t("clientMarket.offerRequiresPayment")}</p>
                <a
                  href={DASHBOARD_ACCOUNT_PATH}
                  className="inline-flex w-fit items-center font-medium text-foreground underline underline-offset-2"
                >
                  {t("clientMarket.goToAccountPayment")}
                </a>
              </div>
            ) : null}
            <div className="grid gap-3 sm:grid-cols-2">
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.priceUsd")}</span>
                <input
                  value={price}
                  onChange={(event) => setPrice(event.target.value)}
                  placeholder={t("clientMarket.free")}
                  inputMode="decimal"
                  className="h-10 rounded-md border px-3"
                />
              </label>
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.periodDays")}</span>
                <input
                  value={period}
                  onChange={(event) => setPeriod(event.target.value)}
                  placeholder={t("clientMarket.forever")}
                  inputMode="numeric"
                  className="h-10 rounded-md border px-3"
                />
              </label>
            </div>
            <p className="text-xs text-muted-foreground">{t("clientMarket.makeFreeHint")}</p>
            {error ? <p className="text-sm text-rose-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer>
            <Button variant="ghost" isDisabled={busy} onClick={() => onOpenChange(false)}>{t("common.cancel")}</Button>
            <Button variant="primary" isDisabled={busy || paymentReady === null} onClick={() => void save()}>
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}

function HostRow({
  host,
  billing,
  isAdmin,
  selectionMode,
  selected,
  onSelectedChange,
  selectionDisabled,
  onChanged,
  onCreate,
}: {
  host: ClientMarketHost;
  billing?: ClientMarketBilling;
  isAdmin: boolean;
  selectionMode: boolean;
  selected: boolean;
  onSelectedChange: (selected: boolean) => void;
  selectionDisabled?: boolean;
  onChanged: () => void;
  onCreate: (host: ClientMarketHost) => void;
}) {
  const { locale, t } = useLocaleText();
  const { openTerminal } = useWebTerminal();
  const [busy, setBusy] = React.useState(false);
  const [confirmAction, setConfirmAction] = React.useState<"delete" | "cleanup" | "unpaid" | null>(null);
  const [cleanupJob, setCleanupJob] = React.useState<ProvisioningJob | null>(null);
  const [cleanupOpen, setCleanupOpen] = React.useState(false);
  const [offerOpen, setOfferOpen] = React.useState(false);
  const canManageHost = hostCanManage(host, isAdmin);
  const canDelete = hostCanDelete(host, isAdmin);
  const isClientOwner = host.isClientOwner === true;
  const canCleanup = hostCanCleanup(host, isAdmin);
  const canMarkUnpaid =
    !!host.installationId &&
    host.status === "allocated" &&
    host.isHostOwner === true &&
    !isClientOwner;
  const isRetryCleanup =
    canCleanup && (host.status === "unreachable" || host.status === "draining");
  const canReverify = hostCanReverify(host, isAdmin);
  const canOpenTerminal = host.canWebTerminal === true;
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

  const onCleanup = async (markUnpaid: boolean) => {
    if (!host.installationId) return;
    setConfirmAction(null);
    setBusy(true);
    setCleanupJob(null);
    setCleanupOpen(true);
    try {
      const { jobId } = markUnpaid
        ? await cleanupClientMarketClientWithReason(host.installationId, {
            reason: "payment_not_received",
            blockClientForProvider: true,
          })
        : await cleanupClientMarketClientWithReason(host.installationId, {
            reason: cleanupReasonForHost(host, isAdmin),
            blockClientForProvider: false,
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

  const confirmCopy = confirmAction === "unpaid"
    ? {
        title: t("clientMarket.unpaidCleanupConfirmTitle"),
        description: t("clientMarket.unpaidCleanupConfirmDesc", { host: hostLabel }),
        confirmLabel: t("clientMarket.unpaidCleanup"),
      }
    : confirmAction === "cleanup"
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
  const paymentKinds = host.paymentMethodKinds || [];
  const hasBillingCountdown = !!(
    billing &&
    billing.priceCents &&
    (billing.status === "payment_due" ? billing.paymentDeadline : billing.currentPeriodEnd)
  );
  const showSubrow =
    !!host.clientOwnerEmail ||
    hasBillingCountdown ||
    !!host.note;
  const ipIntelSubtitle = secondaryIntelParts.length ? secondaryIntelParts.join(" · ") : "";
  const statusGuidanceKey = hostStatusGuidanceKey(host.status, host.lastError);
  const statusGuidanceSubtitle = statusGuidanceKey ? t(statusGuidanceKey) : "";
  const statusHint = fineStatusHintKey(host.status) ? t(fineStatusHintKey(host.status)!) : "";
  const colSpan = selectionMode ? 8 : 7;

  return (
    <>
      <tr className="border-b border-border/80 transition-colors hover:bg-muted/40">
        {selectionMode ? (
          <td className="w-10 px-2 py-2 align-middle">
            <Checkbox
              isSelected={selected}
              onChange={onSelectedChange}
              isDisabled={selectionDisabled}
              aria-label={t("clientMarket.batchSelected", { count: 1 })}
              className="shrink-0"
            >
              <Checkbox.Control>
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
          <span className="block truncate text-xs font-medium text-foreground" title={host.hostOwnerEmail}>
            {host.hostOwnerEmail}
          </span>
        </td>
        <td className="max-w-[9rem] whitespace-nowrap px-2 py-2 align-middle">
          <div className="min-w-0">
            <span className="block text-xs font-semibold text-foreground" title={t("clientMarket.currentOffer")}>
              {formatHostOffer(host.priceCents, host.rentalPeriodDays, locale)}
            </span>
            {paymentKinds.length ? (
              <PaymentMethodIcons kinds={paymentKinds} className="mt-0.5" />
            ) : null}
          </div>
        </td>
        <td className="max-w-[10rem] px-2 py-2 align-middle">
          {subdomain ? (
            <span
              className="block truncate font-mono text-xs text-muted-foreground"
              title={host.installationId || host.hostname || undefined}
            >
              {subdomain}
            </span>
          ) : (
            <span className="text-xs text-muted-foreground/50">—</span>
          )}
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
            {host.status === "idle" ? (
              <Button variant="outline" size="sm" className="h-8 shrink-0" onClick={() => onCreate(host)}>
                <Plus className="h-4 w-4" />
                {t("createClient.newClient")}
              </Button>
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
                    {canMarkUnpaid ? (
                      <Dropdown.Item id="unpaid" className="text-destructive" onAction={() => setConfirmAction("unpaid")}>
                        <X className="h-4 w-4" />
                        {t("clientMarket.unpaidCleanup")}
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
            <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-[11px] leading-4 text-muted-foreground">
              {host.clientOwnerEmail ? (
                <span className="min-w-0 whitespace-normal break-words" title={host.clientOwnerEmail}>
                  {t("clientMarket.rentedBy", { email: host.clientOwnerEmail })}
                </span>
              ) : null}
              <HostBillingCountdown billing={billing} />
              {host.note ? (
                <span className="min-w-0 whitespace-normal break-words" title={host.note}>
                  {host.note}
                </span>
              ) : null}
            </div>
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
                  onConfirm={() => {
                    if (confirmAction === "cleanup" || confirmAction === "unpaid") {
                      void onCleanup(confirmAction === "unpaid");
                    } else void onDelete();
                  }}
                  onOpenChange={(nextOpen) => {
                    if (!nextOpen && !busy) setConfirmAction(null);
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
            </>,
            document.body,
          )
        : null}
    </>
  );
}

const OWNER_FILTER_KEY = "cc_switch_router_client_market_owner_filter_v1";
const REGION_FILTER_KEY = "cc_switch_router_client_market_region_filter_v1";
const STATUS_FILTER_KEY = "cc_switch_router_client_market_status_filter_v2";
const SORT_PREFS_KEY = "cc_switch_router_client_market_sort_v1";
const HOST_PAGE_SIZE = 10;

/** Compact page list: 1 … 4 5 6 … 12 */
function buildHostPageItems(current: number, total: number): Array<number | "ellipsis"> {
  if (total <= 7) return Array.from({ length: total }, (_, i) => i + 1);
  const pages = new Set<number>([1, total, current, current - 1, current + 1]);
  if (current <= 3) {
    pages.add(2);
    pages.add(3);
    pages.add(4);
  }
  if (current >= total - 2) {
    pages.add(total - 1);
    pages.add(total - 2);
    pages.add(total - 3);
  }
  const sorted = [...pages].filter((page) => page >= 1 && page <= total).sort((a, b) => a - b);
  const items: Array<number | "ellipsis"> = [];
  for (const page of sorted) {
    const prev = items[items.length - 1];
    if (typeof prev === "number" && page - prev > 1) items.push("ellipsis");
    items.push(page);
  }
  return items;
}

const HOST_SORT_KEYS = ["status", "region", "owner", "offer", "subdomain", "ip"] as const;
type HostSortKey = (typeof HOST_SORT_KEYS)[number];
type HostSortDir = "asc" | "desc";
type HostSortPrefs = { key: HostSortKey | null; dir: HostSortDir };

const DEFAULT_HOST_SORT: HostSortPrefs = { key: "owner", dir: "asc" };
const CLEARED_HOST_SORT: HostSortPrefs = { key: null, dir: "asc" };

function normalizeHostSortPrefs(value: unknown): HostSortPrefs {
  if (!value || typeof value !== "object") return DEFAULT_HOST_SORT;
  const record = value as { key?: unknown; dir?: unknown };
  if (record.key === null) return CLEARED_HOST_SORT;
  const key =
    typeof record.key === "string" && (HOST_SORT_KEYS as readonly string[]).includes(record.key)
      ? (record.key as HostSortKey)
      : DEFAULT_HOST_SORT.key;
  const dir = record.dir === "desc" ? "desc" : "asc";
  return { key, dir };
}

function compareHostOffer(left: ClientMarketHost, right: ClientMarketHost) {
  const leftFree = !left.priceCents || !left.rentalPeriodDays;
  const rightFree = !right.priceCents || !right.rentalPeriodDays;
  if (leftFree !== rightFree) return leftFree ? -1 : 1;
  const priceCmp = (left.priceCents || 0) - (right.priceCents || 0);
  if (priceCmp !== 0) return priceCmp;
  return (left.rentalPeriodDays || 0) - (right.rentalPeriodDays || 0);
}

function compareHostsBySortKey(left: ClientMarketHost, right: ClientMarketHost, key: HostSortKey) {
  switch (key) {
    case "status":
      return left.status.localeCompare(right.status);
    case "region":
      return (left.countryCode || "").localeCompare(right.countryCode || "");
    case "owner":
      return left.hostOwnerEmail.localeCompare(right.hostOwnerEmail);
    case "offer":
      return compareHostOffer(left, right);
    case "subdomain":
      return (left.clientSubdomain || "").localeCompare(right.clientSubdomain || "");
    case "ip":
      return `${left.ip || ""}:${left.port || 0}`.localeCompare(`${right.ip || ""}:${right.port || 0}`);
    default:
      return 0;
  }
}

function compareHostsDefault(left: ClientMarketHost, right: ClientMarketHost) {
  const ownerCmp = left.hostOwnerEmail.localeCompare(right.hostOwnerEmail);
  if (ownerCmp !== 0) return ownerCmp;
  const ipCmp = `${left.ip || ""}:${left.port || 0}`.localeCompare(`${right.ip || ""}:${right.port || 0}`);
  if (ipCmp !== 0) return ipCmp;
  return left.id.localeCompare(right.id);
}

function sortHosts(hosts: ClientMarketHost[], prefs: HostSortPrefs) {
  if (!prefs.key) {
    return [...hosts].sort(compareHostsDefault);
  }
  const dir = prefs.dir === "desc" ? -1 : 1;
  const key = prefs.key;
  return [...hosts].sort((left, right) => {
    const primary = compareHostsBySortKey(left, right, key);
    if (primary !== 0) return primary * dir;
    return compareHostsDefault(left, right);
  });
}

function normalizeHostStatusFilter(value: unknown): HostStatusFilter {
  if (typeof value !== "string") return "all";
  if ((HOST_STATUS_GROUPS as readonly string[]).includes(value)) {
    return value as HostStatusFilter;
  }
  // Migrate legacy fine-grained tabs.
  const mapped = statusGroupForHost(value);
  return mapped ?? "all";
}

function hostStatusTabTone(status: HostStatusFilter, active: boolean) {
  if (active) return "bg-white font-medium text-foreground shadow-sm";
  switch (status) {
    case "needs_attention":
      return "text-amber-700";
    case "idle":
      return "text-emerald-700";
    case "in_use":
      return "text-slate-700";
    default:
      return "text-muted-foreground";
  }
}

const HOST_SORT_COLUMN_LABELS: Record<HostSortKey, MessageKey> = {
  status: "clientMarket.col.status",
  region: "clientMarket.col.region",
  owner: "clientMarket.col.owner",
  offer: "clientMarket.col.offer",
  subdomain: "clientMarket.col.subdomain",
  ip: "clientMarket.col.ip",
};

function HostSortHeader({
  columnKey,
  sortPrefs,
  onSort,
}: {
  columnKey: HostSortKey;
  sortPrefs: HostSortPrefs;
  onSort: (key: HostSortKey) => void;
}) {
  const { t } = useLocaleText();
  const active = sortPrefs.key === columnKey;
  const label = t(HOST_SORT_COLUMN_LABELS[columnKey]);
  const ariaSort = active ? (sortPrefs.dir === "asc" ? "ascending" : "descending") : "none";
  const sortStateLabel = active
    ? t(sortPrefs.dir === "asc" ? "clientMarket.sortAsc" : "clientMarket.sortDesc")
    : undefined;

  return (
    <th
      scope="col"
      aria-sort={ariaSort}
      className="sticky top-0 z-10 whitespace-nowrap border-b border-border bg-card px-2 py-2 text-left text-xs font-medium text-muted-foreground"
    >
      <button
        type="button"
        className="inline-flex items-center gap-1 rounded-md px-1 py-0.5 text-left transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
        onClick={() => onSort(columnKey)}
        aria-label={t("clientMarket.sortBy", { column: label })}
      >
        <span>{label}</span>
        {active ? (
          sortPrefs.dir === "asc" ? (
            <ArrowUp className="h-3.5 w-3.5 text-accent" aria-hidden />
          ) : (
            <ArrowDown className="h-3.5 w-3.5 text-accent" aria-hidden />
          )
        ) : (
          <span className="inline-flex h-3.5 w-3.5 flex-col justify-center opacity-30" aria-hidden>
            <ArrowUp className="h-2.5 w-2.5 -mb-0.5" />
            <ArrowDown className="h-2.5 w-2.5" />
          </span>
        )}
        {sortStateLabel ? <span className="sr-only">{sortStateLabel}</span> : null}
      </button>
    </th>
  );
}

export function ClientMarketPage() {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const viewerUserId = session?.user?.id;
  const isAdmin = !!session?.isAdmin;

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [providers, setProviders] = React.useState<ClientMarketProvider[]>([]);
  const [billingByInstallation, setBillingByInstallation] = React.useState<Map<string, ClientMarketBilling>>(new Map());
  const [loading, setLoading] = React.useState(true);
  const [addOpen, setAddOpen] = React.useState(false);
  const [pendingAddAfterLogin, setPendingAddAfterLogin] = React.useState(false);
  const [mineOnly, setMineOnly] = React.useState(false);
  const [ownerFilters, setOwnerFilters] = usePersistentState<string[]>(OWNER_FILTER_KEY, []);
  const [regionFilters, setRegionFilters] = usePersistentState<string[]>(REGION_FILTER_KEY, []);
  const [statusFilterRaw, setStatusFilter] = usePersistentState<HostStatusFilter>(STATUS_FILTER_KEY, "all");
  const [sortPrefsRaw, setSortPrefs] = usePersistentState<HostSortPrefs>(SORT_PREFS_KEY, DEFAULT_HOST_SORT);
  const sortPrefs = React.useMemo(() => normalizeHostSortPrefs(sortPrefsRaw), [sortPrefsRaw]);
  const statusFilter = normalizeHostStatusFilter(statusFilterRaw);
  const [page, setPage] = React.useState(1);
  const [error, setError] = React.useState("");
  const [fixedHost, setFixedHost] = React.useState<ClientMarketHost | null>(null);
  const [transferBusy, setTransferBusy] = React.useState(false);
  const [importResult, setImportResult] = React.useState<ClientMarketHostImportResponse | null>(null);
  const importInputRef = React.useRef<HTMLInputElement | null>(null);
  const [selectionMode, setSelectionMode] = React.useState(false);
  const [selectedIds, setSelectedIds] = React.useState<Set<string>>(new Set());
  const [filterOpen, setFilterOpen] = React.useState(false);
  const filterRootRef = React.useRef<HTMLDivElement | null>(null);
  const [batchBusy, setBatchBusy] = React.useState(false);
  const [batchConfirm, setBatchConfirm] = React.useState<"cleanup" | "delete" | "reverify" | null>(null);
  const [batchProgressOpen, setBatchProgressOpen] = React.useState(false);
  const [batchProgressAction, setBatchProgressAction] = React.useState<"cleanup" | "delete" | "reverify">("cleanup");
  const [batchProgressItems, setBatchProgressItems] = React.useState<BatchProgressItem[]>([]);

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const [nextHosts, supply, billing] = await Promise.all([
        getClientMarketHosts(),
        getClientMarketProviderSupply(),
        authed ? getMyClientMarketBilling() : Promise.resolve([]),
      ]);
      setHosts(nextHosts);
      setProviders(supply.providers);
      setBillingByInstallation(new Map(billing.map((record) => [record.installationId, record])));
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, [authed]);

  React.useEffect(() => {
    void load();
  }, [isAdmin, load, viewerUserId]);

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

  const scopedHosts = React.useMemo(() => {
    const ownerSet = new Set(ownerFilters.map((email) => email.toLowerCase()));
    const regionSet = new Set(regionFilters.map((code) => code.toUpperCase()));
    return hosts.filter((host) => {
      if (mineOnly && host.isHostOwner !== true) return false;
      if (ownerSet.size > 0 && !ownerSet.has(host.hostOwnerEmail.toLowerCase())) return false;
      if (regionSet.size > 0) {
        const code = (host.countryCode || "").trim().toUpperCase();
        if (!code || !regionSet.has(code)) return false;
      }
      return true;
    });
  }, [hosts, mineOnly, ownerFilters, regionFilters]);

  const statusCounts = React.useMemo(() => {
    const counts: Record<HostStatusFilter, number> = {
      all: scopedHosts.length,
      idle: 0,
      in_use: 0,
      needs_attention: 0,
    };
    for (const host of scopedHosts) {
      const group = statusGroupForHost(host.status);
      if (group) counts[group] += 1;
    }
    return counts;
  }, [scopedHosts]);

  const visibleHosts = React.useMemo(() => {
    const filtered = scopedHosts.filter((host) => hostMatchesStatusFilter(host.status, statusFilter));
    return sortHosts(filtered, sortPrefs);
  }, [scopedHosts, sortPrefs, statusFilter]);

  const toggleHostSort = React.useCallback((key: HostSortKey) => {
    setSortPrefs((prev) => {
      const current = normalizeHostSortPrefs(prev);
      if (current.key !== key) return { key, dir: "asc" };
      if (current.dir === "asc") return { key, dir: "desc" };
      return CLEARED_HOST_SORT;
    });
  }, [setSortPrefs]);

  const totalPages = Math.max(1, Math.ceil(visibleHosts.length / HOST_PAGE_SIZE));
  const safePage = Math.min(page, totalPages);
  const pagedHosts = React.useMemo(() => {
    const start = (safePage - 1) * HOST_PAGE_SIZE;
    return visibleHosts.slice(start, start + HOST_PAGE_SIZE);
  }, [safePage, visibleHosts]);

  React.useEffect(() => {
    setPage(1);
  }, [mineOnly, ownerFilters, regionFilters, sortPrefs.key, sortPrefs.dir, statusFilter]);

  React.useEffect(() => {
    if (page > totalPages) setPage(totalPages);
  }, [page, totalPages]);

  React.useEffect(() => {
    if (!pendingAddAfterLogin || !authed) return;
    setPendingAddAfterLogin(false);
    setAddOpen(true);
  }, [authed, pendingAddAfterLogin]);

  // Drop selections that left the current filter set (or disappeared from hosts).
  React.useEffect(() => {
    if (!selectionMode) return;
    const visibleIds = new Set(visibleHosts.map((host) => host.id));
    setSelectedIds((prev) => {
      let changed = false;
      const next = new Set<string>();
      for (const id of prev) {
        if (visibleIds.has(id)) next.add(id);
        else changed = true;
      }
      return changed || next.size !== prev.size ? next : prev;
    });
  }, [selectionMode, visibleHosts]);

  React.useEffect(() => {
    if (!authed && selectionMode) {
      setSelectionMode(false);
      setSelectedIds(new Set());
    }
  }, [authed, selectionMode]);

  const selectedHosts = React.useMemo(
    () => hosts.filter((host) => selectedIds.has(host.id)),
    [hosts, selectedIds],
  );
  const selectedCount = selectedIds.size;
  const cleanupEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanCleanup(host, isAdmin)),
    [isAdmin, selectedHosts],
  );
  const reverifyEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanReverify(host, isAdmin)),
    [isAdmin, selectedHosts],
  );
  const deleteEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanDelete(host, isAdmin)),
    [isAdmin, selectedHosts],
  );
  const exportEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanExport(host)),
    [selectedHosts],
  );

  const setHostSelected = React.useCallback((hostId: string, selected: boolean) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (selected) next.add(hostId);
      else next.delete(hostId);
      return next;
    });
  }, []);

  const enterSelectionMode = () => setSelectionMode(true);

  const exitSelectionMode = React.useCallback(() => {
    setSelectionMode(false);
    setSelectedIds(new Set());
  }, []);

  const selectPage = () => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      for (const host of pagedHosts) next.add(host.id);
      return next;
    });
  };

  const selectAllFiltered = () => {
    setSelectedIds(new Set(visibleHosts.map((host) => host.id)));
  };

  const clearSelection = () => setSelectedIds(new Set());

  const finishBatch = React.useCallback(
    (items: BatchProgressItem[]) => {
      const summary = countBatchStatuses(items);
      toast[summary.failed > 0 ? "danger" : summary.succeeded > 0 ? "success" : "info"](
        t("clientMarket.batchSummary", summary),
      );
      if (summary.failed > 0) {
        setSelectionMode(true);
        setSelectedIds(
          new Set(items.filter((item) => item.status === "failed").map((item) => item.hostId)),
        );
      } else {
        setSelectionMode(false);
        setSelectedIds(new Set());
      }
      void load();
    },
    [load, t],
  );

  const beginBatchProgress = (
    action: "cleanup" | "delete" | "reverify",
    targets: ClientMarketHost[],
    skippedHosts: ClientMarketHost[],
  ) => {
    const items: BatchProgressItem[] = [
      ...targets.map((host) => ({
        hostId: host.id,
        label: hostDisplayLabel(host),
        status: "queued" as const,
      })),
      ...skippedHosts.map((host) => ({
        hostId: host.id,
        label: hostDisplayLabel(host),
        status: "skipped" as const,
      })),
    ];
    const byId = new Map(items.map((item) => [item.hostId, item]));
    setBatchProgressAction(action);
    setBatchProgressItems(items);
    setBatchProgressOpen(true);
    const patch = (hostId: string, next: Partial<BatchProgressItem>) => {
      const current = byId.get(hostId);
      if (!current) return;
      const updated = { ...current, ...next };
      byId.set(hostId, updated);
      setBatchProgressItems(Array.from(byId.values()));
    };
    return { byId, patch };
  };

  const pollCleanupJobQuiet = async (jobId: string) => {
    for (let i = 0; i < 180; i++) {
      await new Promise((r) => setTimeout(r, 1200));
      try {
        const latest = await getClientMarketJob(jobId);
        if (latest.status === "succeeded") return { ok: true as const };
        if (latest.status === "failed") {
          const detail = latest.failureCode || latest.log.split("\n").filter(Boolean).at(-1) || "";
          return { ok: false as const, detail };
        }
      } catch {
        continue;
      }
    }
    return { ok: false as const, detail: t("clientMarket.cleanupTimedOut") };
  };

  const runBatchCleanup = async () => {
    const targets = cleanupEligible;
    const skippedHosts = selectedHosts.filter((host) => !hostCanCleanup(host, isAdmin));
    const { byId, patch } = beginBatchProgress("cleanup", targets, skippedHosts);
    setBatchBusy(true);
    try {
      await mapPool(targets, 2, async (host) => {
        patch(host.id, { status: "running" });
        if (!host.installationId) {
          patch(host.id, { status: "skipped" });
          return;
        }
        try {
          const { jobId } = await cleanupClientMarketClientWithReason(host.installationId, {
            reason: cleanupReasonForHost(host, isAdmin),
            blockClientForProvider: false,
          });
          const result = await pollCleanupJobQuiet(jobId);
          if (result.ok) patch(host.id, { status: "succeeded" });
          else patch(host.id, { status: "failed", detail: result.detail });
        } catch (err) {
          patch(host.id, {
            status: "failed",
            detail: err instanceof Error ? err.message : String(err),
          });
        }
      });
      finishBatch(Array.from(byId.values()));
    } finally {
      setBatchBusy(false);
    }
  };

  const runBatchReverify = async () => {
    const targets = reverifyEligible;
    const skippedHosts = selectedHosts.filter((host) => !hostCanReverify(host, isAdmin));
    const { byId, patch } = beginBatchProgress("reverify", targets, skippedHosts);
    setBatchBusy(true);
    try {
      await mapPool(targets, 5, async (host) => {
        patch(host.id, { status: "running" });
        try {
          await reverifyClientMarketHost(host.id);
          patch(host.id, { status: "succeeded" });
        } catch (err) {
          patch(host.id, {
            status: "failed",
            detail: err instanceof Error ? err.message : String(err),
          });
        }
      });
      finishBatch(Array.from(byId.values()));
    } finally {
      setBatchBusy(false);
    }
  };

  const runBatchDelete = async () => {
    const targets = deleteEligible;
    const skippedHosts = selectedHosts.filter((host) => !hostCanDelete(host, isAdmin));
    const { byId, patch } = beginBatchProgress("delete", targets, skippedHosts);
    setBatchBusy(true);
    try {
      await mapPool(targets, 5, async (host) => {
        patch(host.id, { status: "running" });
        try {
          await deleteClientMarketHost(host.id);
          patch(host.id, { status: "succeeded" });
        } catch (err) {
          patch(host.id, {
            status: "failed",
            detail: err instanceof Error ? err.message : String(err),
          });
        }
      });
      finishBatch(Array.from(byId.values()));
    } finally {
      setBatchBusy(false);
    }
  };

  const openAddHost = () => {
    if (!authed) {
      setPendingAddAfterLogin(true);
      window.dispatchEvent(new Event(ROUTER_OPEN_LOGIN_EVENT));
      return;
    }
    setAddOpen(true);
  };

  const downloadHostExport = (document: Awaited<ReturnType<typeof exportMyClientMarketHosts>>) => {
    const blob = new Blob([`${JSON.stringify(document, null, 2)}\n`], { type: "application/json" });
    const url = URL.createObjectURL(blob);
    const link = window.document.createElement("a");
    link.href = url;
    link.download = `cc-switch-client-market-hosts-${new Date().toISOString().slice(0, 10)}.json`;
    link.click();
    URL.revokeObjectURL(url);
    toast.success(t("clientMarket.exportedHosts", { count: document.hosts.length }));
  };

  const exportHosts = async (selectedOnly: boolean) => {
    setTransferBusy(true);
    try {
      const document = await exportMyClientMarketHosts();
      if (selectedOnly) {
        const keys = new Set(exportEligible.map((host) => hostExportKey(host)).filter(Boolean));
        if (!keys.size) {
          toast.danger(t("clientMarket.batchExportEmpty"));
          return;
        }
        document.hosts = document.hosts.filter((entry) => keys.has(hostExportKey(entry)));
        if (!document.hosts.length) {
          toast.danger(t("clientMarket.batchExportEmpty"));
          return;
        }
      }
      downloadHostExport(document);
      if (selectedOnly) clearSelection();
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setTransferBusy(false);
    }
  };

  const batchConfirmCopy =
    batchConfirm === "cleanup"
      ? {
          title: t("clientMarket.batchConfirmCleanupTitle"),
          description: t("clientMarket.batchConfirmCleanupDesc", {
            run: cleanupEligible.length,
            skip: selectedCount - cleanupEligible.length,
          }),
          confirmLabel: t("clientMarket.cleanup"),
          run: () => void runBatchCleanup(),
        }
      : batchConfirm === "reverify"
        ? {
            title: t("clientMarket.batchConfirmReverifyTitle"),
            description: t("clientMarket.batchConfirmReverifyDesc", {
              run: reverifyEligible.length,
              skip: selectedCount - reverifyEligible.length,
            }),
            confirmLabel: t("clientMarket.reverifyHost"),
            run: () => void runBatchReverify(),
          }
        : batchConfirm === "delete"
          ? {
              title: t("clientMarket.batchConfirmDeleteTitle"),
              description: t("clientMarket.batchConfirmDeleteDesc", {
                run: deleteEligible.length,
                skip: selectedCount - deleteEligible.length,
              }),
              confirmLabel: t("clientMarket.deleteHost"),
              run: () => void runBatchDelete(),
            }
          : null;

  const batchActionLabel =
    batchProgressAction === "cleanup"
      ? t("clientMarket.batchProgressCleanup")
      : batchProgressAction === "reverify"
        ? t("clientMarket.batchProgressReverify")
        : t("clientMarket.batchProgressDelete");

  const importHosts = async (file?: File) => {
    if (!file) return;
    if (file.size > 1024 * 1024) {
      toast.danger(t("clientMarket.importSizeLimit"));
      return;
    }
    setTransferBusy(true);
    try {
      const document = JSON.parse(await file.text());
      if (!document || document.version !== 1 || !Array.isArray(document.hosts)) {
        throw new Error(t("clientMarket.importVersionRequired"));
      }
      const result = await importMyClientMarketHosts(document);
      setImportResult(result);
      await load();
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setTransferBusy(false);
      if (importInputRef.current) importInputRef.current.value = "";
    }
  };

  const statusTabs = React.useMemo(
    () =>
      HOST_STATUS_GROUPS.map((value) => ({
        value,
        label: t(statusGroupLabelKey(value)),
        hint: t(statusGroupHintKey(value)),
        count: statusCounts[value],
      })),
    [statusCounts, t],
  );

  const activeFilterCount =
    (mineOnly ? 1 : 0) + (ownerFilters.length > 0 ? 1 : 0) + (regionFilters.length > 0 ? 1 : 0);
  const hasActiveFilters = activeFilterCount > 0;

  React.useEffect(() => {
    if (!filterOpen) return;
    const onPointerDown = (event: PointerEvent) => {
      if (filterRootRef.current?.contains(event.target as Node)) return;
      setFilterOpen(false);
    };
    window.addEventListener("pointerdown", onPointerDown);
    return () => window.removeEventListener("pointerdown", onPointerDown);
  }, [filterOpen]);

  const clearScopedFilters = () => {
    setOwnerFilters([]);
    setRegionFilters([]);
    setMineOnly(false);
  };

  return (
    <div className="mx-auto grid min-w-0 w-[calc(100%-2rem)] max-w-7xl grid-cols-[minmax(0,1fr)] gap-5 pb-10">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
          <div className="inline-flex max-w-full overflow-x-auto rounded-lg bg-slate-100 p-1 text-[11px]">
            {statusTabs.map((tab) => (
              <button
                key={tab.value}
                type="button"
                title={tab.hint}
                aria-label={`${tab.label}. ${tab.hint}`}
                onClick={() => setStatusFilter(tab.value)}
                className={`rounded-md px-2.5 py-1.5 transition-colors ${hostStatusTabTone(tab.value, statusFilter === tab.value)}`}
              >
                {tab.label} · {tab.count}
              </button>
            ))}
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <div ref={filterRootRef} className="relative">
            <Button
              variant="outline"
              size="sm"
              className="h-8"
              aria-expanded={filterOpen}
              aria-label={t("clientMarket.filter")}
              onClick={() => setFilterOpen((open) => !open)}
            >
              <Filter className="h-4 w-4" />
              {hasActiveFilters
                ? t("clientMarket.filterActive", { count: activeFilterCount })
                : t("clientMarket.filter")}
              <ChevronDown className={`h-3.5 w-3.5 transition-transform ${filterOpen ? "rotate-180" : ""}`} />
            </Button>
            {filterOpen ? (
              <div className="absolute right-0 z-30 mt-1.5 w-[min(20rem,calc(100vw-2rem))] rounded-lg border border-border bg-white p-3 shadow-lg">
                <div className="grid gap-3">
                  <CompactRegionMultiSelect
                    values={ownerFilters}
                    onChange={setOwnerFilters}
                    options={ownerOptions}
                    allLabel={t("clientMarket.allOwners")}
                    moreLabel={(count) => t("clientMarket.ownersMore", { count })}
                    clearLabel={t("clientMarket.clearOwnerSelection")}
                    ariaLabel={t("clientMarket.filterOwners")}
                    className="w-full"
                  />
                  <CompactRegionMultiSelect
                    values={regionFilters}
                    onChange={setRegionFilters}
                    options={regionOptions}
                    allLabel={t("clientMarket.allRegions")}
                    moreLabel={(count) => t("clientMarket.regionsMore", { count })}
                    clearLabel={t("clientMarket.clearRegionSelection")}
                    ariaLabel={t("clientMarket.filterRegions")}
                    className="w-full"
                  />
                  {authed ? (
                    <label className="flex cursor-pointer items-center gap-2 rounded-md border border-border px-3 py-2 text-sm">
                      <input
                        type="checkbox"
                        className="h-4 w-4 accent-[var(--accent,#0052FF)]"
                        checked={mineOnly}
                        onChange={(event) => setMineOnly(event.target.checked)}
                      />
                      <span>{t("clientMarket.filterMineOnly")}</span>
                    </label>
                  ) : null}
                  <Button
                    variant="ghost"
                    size="sm"
                    className="justify-start"
                    isDisabled={!hasActiveFilters}
                    onClick={clearScopedFilters}
                  >
                    {t("clientMarket.filterClear")}
                  </Button>
                </div>
              </div>
            ) : null}
          </div>
          {authed ? (
            <>
              <input ref={importInputRef} type="file" accept="application/json,.json" className="hidden" onChange={(event) => void importHosts(event.target.files?.[0])} />
              {selectionMode ? (
                <Button variant="outline" size="sm" className="h-8" isDisabled={batchBusy} onClick={exitSelectionMode}>
                  <CheckSquare className="h-4 w-4" />
                  {t("clientMarket.batchDoneSelection")}
                </Button>
              ) : (
                <Button
                  variant="outline"
                  size="sm"
                  className="h-8"
                  isDisabled={batchBusy || !visibleHosts.length}
                  onClick={enterSelectionMode}
                >
                  <CheckSquare className="h-4 w-4" />
                  {t("clientMarket.batchEnterSelection")}
                </Button>
              )}
              <Tooltip>
                <Tooltip.Trigger>
                  <Button
                    variant="outline"
                    size="sm"
                    isIconOnly
                    className="h-8 w-8 min-w-8"
                    aria-label={t("clientMarket.importMyHosts")}
                    isDisabled={transferBusy}
                    onClick={() => importInputRef.current?.click()}
                  >
                    <Upload className="h-4 w-4" />
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>{t("clientMarket.importMyHosts")}</Tooltip.Content>
              </Tooltip>
              <Tooltip>
                <Tooltip.Trigger>
                  <Button
                    variant="outline"
                    size="sm"
                    isIconOnly
                    className="h-8 w-8 min-w-8"
                    aria-label={t("clientMarket.exportMyHosts")}
                    isDisabled={transferBusy || batchBusy}
                    onClick={() => void exportHosts(false)}
                  >
                    {transferBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : <Download className="h-4 w-4" />}
                  </Button>
                </Tooltip.Trigger>
                <Tooltip.Content>{t("clientMarket.exportMyHosts")}</Tooltip.Content>
              </Tooltip>
            </>
          ) : null}
          <Button variant="primary" size="sm" className="h-8" onClick={openAddHost}>
            <Plus className="h-4 w-4" />
            {t("clientMarket.addHost")}
          </Button>
        </div>
      </div>

      {selectionMode ? (
        <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-white px-3 py-2 text-sm">
          <span className="font-medium text-foreground">{t("clientMarket.batchSelected", { count: selectedCount })}</span>
          <Button variant="outline" size="sm" isDisabled={batchBusy || !visibleHosts.length} onClick={selectAllFiltered}>
            {t("clientMarket.batchSelectAll")}
          </Button>
          <Button variant="ghost" size="sm" isDisabled={batchBusy || !pagedHosts.length} onClick={selectPage}>
            {t("clientMarket.batchSelectPage")}
          </Button>
          <Button variant="ghost" size="sm" isDisabled={batchBusy || selectedCount === 0} onClick={clearSelection}>
            {t("clientMarket.batchClear")}
          </Button>
          <span className="mx-1 h-4 w-px bg-border" aria-hidden />
          <Button
            variant="outline"
            size="sm"
            isDisabled={batchBusy || cleanupEligible.length === 0}
            aria-label={t("clientMarket.batchEligible", { run: cleanupEligible.length, selected: selectedCount })}
            onClick={() => {
              if (!cleanupEligible.length) {
                toast.info(t("clientMarket.batchNothingEligible"));
                return;
              }
              setBatchConfirm("cleanup");
            }}
          >
            {t("clientMarket.cleanup")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: cleanupEligible.length, selected: selectedCount })}
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            isDisabled={batchBusy || reverifyEligible.length === 0}
            aria-label={t("clientMarket.batchEligible", { run: reverifyEligible.length, selected: selectedCount })}
            onClick={() => {
              if (!reverifyEligible.length) {
                toast.info(t("clientMarket.batchNothingEligible"));
                return;
              }
              setBatchConfirm("reverify");
            }}
          >
            <RefreshCw className="h-4 w-4" />
            {t("clientMarket.reverifyHost")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: reverifyEligible.length, selected: selectedCount })}
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            className="text-destructive"
            isDisabled={batchBusy || deleteEligible.length === 0}
            aria-label={t("clientMarket.batchEligible", { run: deleteEligible.length, selected: selectedCount })}
            onClick={() => {
              if (!deleteEligible.length) {
                toast.info(t("clientMarket.batchNothingEligible"));
                return;
              }
              setBatchConfirm("delete");
            }}
          >
            <Trash2 className="h-4 w-4" />
            {t("clientMarket.deleteHost")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: deleteEligible.length, selected: selectedCount })}
            </span>
          </Button>
          <Button
            variant="outline"
            size="sm"
            isDisabled={transferBusy || batchBusy || exportEligible.length === 0}
            aria-label={t("clientMarket.batchEligible", { run: exportEligible.length, selected: selectedCount })}
            onClick={() => void exportHosts(true)}
          >
            <Download className="h-4 w-4" />
            {t("clientMarket.batchExportSelected")}
            <span className="ml-1 text-xs text-muted-foreground">
              {t("clientMarket.batchEligible", { run: exportEligible.length, selected: selectedCount })}
            </span>
          </Button>
        </div>
      ) : null}

      {!authed ? (
        <p className="text-sm text-muted-foreground">{t("clientMarket.loginToAddHost")}</p>
      ) : null}

      {providers.length ? (
        <section className="overflow-x-auto border-y border-border bg-white/70 py-2" aria-label={t("clientMarket.providerObservations")}>
          <div className="mb-1 px-1 text-[11px] text-muted-foreground">{t("clientMarket.providerObservationNotice")}</div>
          <div className="flex min-w-max items-stretch gap-5 px-1 text-xs">
            {providers.map((provider) => (
              <div key={provider.providerId} className="grid content-center gap-1 border-r border-border pr-5 last:border-r-0">
                <div className="flex items-center gap-2">
                  <span className="max-w-48 truncate font-medium text-foreground" title={provider.ownerEmail}>{provider.ownerEmail}</span>
                  {provider.official ? <Chip size="sm" variant="soft">{t("createClient.official")}</Chip> : null}
                  <PaymentMethodIcons kinds={provider.paymentMethodKinds} />
                </div>
                <div className="flex items-center gap-3 text-muted-foreground">
                  <span>{t("clientMarket.observedHosts", { total: provider.hostTotal, idle: provider.idleTotal, allocated: provider.allocatedTotal })}</span>
                  <span>{t("clientMarket.observedAllocationRate", { rate: formatObservationRate(provider.allocationRate, locale) })}</span>
                  <span>{t("clientMarket.observedFreeSupply", { total: provider.freeHostTotal, allocated: provider.freeAllocatedTotal })}</span>
                  <span>{t("clientMarket.observedPaidSupply", { total: provider.paidHostTotal, allocated: provider.paidAllocatedTotal })}</span>
                  <span>{t("clientMarket.observedExternalOwners", { count: provider.externalClientOwnerTotal })}</span>
                  <span>{t("clientMarket.observedLongRentals", { over3: provider.externalClientsOver3Days, over30: provider.externalClientsOver30Days })}</span>
                </div>
                <div className="flex items-center gap-3 text-muted-foreground">
                  <span>{t("clientMarket.observedUptime", { rate: formatObservationRate(provider.onlineRate30d, locale) })}</span>
                  <span>{t("clientMarket.observedAnomaly", { rate: formatObservationRate(provider.anomalousHostRate, locale) })}</span>
                  <span>{t("clientMarket.observedJoined", { date: formatObservationDate(provider.joinedAt, locale) })}</span>
                  <span>{t("clientMarket.observedOfferStable", { date: formatObservationDate(provider.offerStableSince, locale) })}</span>
                  <span>{t("clientMarket.observedPriceRange", { range: formatProviderPriceRange(provider, locale) })}</span>
                  <span>{t("clientMarket.observedPeriodRange", { range: formatProviderPeriodRange(provider) })}</span>
                </div>
              </div>
            ))}
          </div>
        </section>
      ) : null}

      {loading ? (
        <div className="flex items-center gap-2 text-sm text-muted-foreground">
          <Loader2 className="h-4 w-4 animate-spin" />
          …
        </div>
      ) : error ? (
        <p className="text-sm text-rose-600">{error}</p>
      ) : visibleHosts.length === 0 ? (
        <div className="grid justify-items-center gap-2 rounded-lg border border-dashed border-border px-4 py-10 text-center text-sm text-muted-foreground">
          <span>{scopedHosts.length ? t("dashboard.noFilterResults") : t("clientMarket.noHosts")}</span>
          {scopedHosts.length || ownerFilters.length || regionFilters.length || statusFilter !== "all" || mineOnly ? (
            <button
              type="button"
              className="text-xs font-medium text-primary hover:underline"
              onClick={() => {
                setStatusFilter("all");
                setOwnerFilters([]);
                setRegionFilters([]);
                setMineOnly(false);
              }}
            >
              {t("dashboard.clearFilters")}
            </button>
          ) : null}
        </div>
      ) : (
        <div className="overflow-hidden rounded-xl border border-border bg-card shadow-sm">
          <div className="max-h-[min(70vh,40rem)] overflow-auto">
            <table className="w-full min-w-[56rem] border-collapse text-sm">
              <thead>
                <tr>
                  {selectionMode ? (
                    <th
                      scope="col"
                      className="sticky top-0 z-10 w-10 border-b border-border bg-card px-2 py-2 text-left"
                    >
                      <Checkbox
                        isSelected={
                          pagedHosts.length > 0 && pagedHosts.every((host) => selectedIds.has(host.id))
                        }
                        isIndeterminate={
                          pagedHosts.some((host) => selectedIds.has(host.id)) &&
                          !pagedHosts.every((host) => selectedIds.has(host.id))
                        }
                        onChange={(checked) => {
                          if (checked) selectPage();
                          else {
                            setSelectedIds((prev) => {
                              const next = new Set(prev);
                              for (const host of pagedHosts) next.delete(host.id);
                              return next;
                            });
                          }
                        }}
                        isDisabled={batchBusy || !pagedHosts.length}
                        aria-label={t("clientMarket.batchSelectPage")}
                        className="shrink-0"
                      >
                        <Checkbox.Control>
                          <Checkbox.Indicator />
                        </Checkbox.Control>
                      </Checkbox>
                    </th>
                  ) : null}
                  <HostSortHeader columnKey="status" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader columnKey="region" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader columnKey="owner" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader columnKey="offer" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader columnKey="subdomain" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader columnKey="ip" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <th
                    scope="col"
                    className="sticky top-0 z-10 whitespace-nowrap border-b border-border bg-card px-2 py-2 text-right text-xs font-medium text-muted-foreground"
                  >
                    {t("clientMarket.col.actions")}
                  </th>
                </tr>
              </thead>
              <tbody>
                {pagedHosts.map((host) => (
                  <HostRow
                    key={host.id}
                    host={host}
                    billing={host.installationId ? billingByInstallation.get(host.installationId) : undefined}
                    isAdmin={isAdmin}
                    selectionMode={selectionMode}
                    selected={selectedIds.has(host.id)}
                    onSelectedChange={(next) => setHostSelected(host.id, next)}
                    selectionDisabled={batchBusy}
                    onChanged={() => void load()}
                    onCreate={setFixedHost}
                  />
                ))}
              </tbody>
            </table>
          </div>
          {visibleHosts.length > HOST_PAGE_SIZE ? (
            <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2 border-t border-border bg-muted/30 px-3 py-2.5">
              <p className="text-xs text-muted-foreground">
                {t("clientMarket.paginationSummary", {
                  start: (safePage - 1) * HOST_PAGE_SIZE + 1,
                  end: Math.min(safePage * HOST_PAGE_SIZE, visibleHosts.length),
                  total: visibleHosts.length,
                })}
              </p>
              <nav className="flex items-center gap-1" aria-label={t("clientMarket.paginationPage", { page: safePage, pages: totalPages })}>
                <button
                  type="button"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-35"
                  disabled={safePage <= 1}
                  aria-label={t("clientMarket.paginationPrev")}
                  onClick={() => setPage((current) => Math.max(1, current - 1))}
                >
                  <ChevronLeft className="h-4 w-4" />
                </button>
                {buildHostPageItems(safePage, totalPages).map((item, index) =>
                  item === "ellipsis" ? (
                    <span
                      key={`ellipsis-${index}`}
                      className="inline-flex h-8 w-6 items-center justify-center text-xs text-muted-foreground/60"
                      aria-hidden
                    >
                      …
                    </span>
                  ) : (
                    <button
                      key={item}
                      type="button"
                      aria-label={t("clientMarket.paginationGoTo", { page: item })}
                      aria-current={item === safePage ? "page" : undefined}
                      className={
                        item === safePage
                          ? "inline-flex h-8 min-w-8 items-center justify-center rounded-lg bg-accent px-2 text-xs font-medium text-accent-foreground shadow-sm shadow-accent/20"
                          : "inline-flex h-8 min-w-8 items-center justify-center rounded-lg px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
                      }
                      onClick={() => setPage(item)}
                    >
                      {item}
                    </button>
                  ),
                )}
                <button
                  type="button"
                  className="inline-flex h-8 w-8 items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-35"
                  disabled={safePage >= totalPages}
                  aria-label={t("clientMarket.paginationNext")}
                  onClick={() => setPage((current) => Math.min(totalPages, current + 1))}
                >
                  <ChevronRight className="h-4 w-4" />
                </button>
              </nav>
            </div>
          ) : null}
        </div>
      )}

      <AddHostDialog open={addOpen} onOpenChange={setAddOpen} onAdded={() => void load()} />
      <CreateClientDialog
        open={!!fixedHost}
        onOpenChange={(next) => { if (!next) setFixedHost(null); }}
        fixedHost={fixedHost}
        onCreated={() => void load()}
      />
      <Modal.Backdrop isOpen={!!importResult} onOpenChange={(next) => { if (!next) setImportResult(null); }}>
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(620px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header><Modal.Heading>{t("clientMarket.importResult")}</Modal.Heading></Modal.Header>
            <Modal.Body className="grid max-h-[65vh] gap-3 overflow-y-auto">
              {importResult ? <div className="flex flex-wrap gap-2 text-sm"><Chip size="sm" variant="soft">{t("clientMarket.importedCount", { count: importResult.imported })}</Chip><Chip size="sm" variant="soft">{t("clientMarket.skippedCount", { count: importResult.skipped })}</Chip><Chip size="sm" variant="soft">{t("clientMarket.failedCount", { count: importResult.failed })}</Chip></div> : null}
              <div className="grid gap-1.5">{importResult?.items.map((item) => <div key={`${item.ip}:${item.port}`} className="grid grid-cols-[minmax(0,1fr)_auto] gap-3 rounded-md border px-3 py-2 text-xs"><span className="min-w-0 truncate font-mono">{item.ip}:{item.port}</span><span className={item.status === "failed" ? "text-rose-600" : item.status === "imported" ? "text-emerald-700" : "text-muted-foreground"}>{item.error || item.status}</span></div>)}</div>
            </Modal.Body>
            <Modal.Footer><Button variant="primary" onClick={() => setImportResult(null)}>{t("common.close")}</Button></Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      {batchConfirmCopy ? (
        <ConfirmAlertDialog
          open
          title={batchConfirmCopy.title}
          description={batchConfirmCopy.description}
          confirmLabel={batchConfirmCopy.confirmLabel}
          cancelLabel={t("common.cancel")}
          tone="danger"
          busy={batchBusy}
          onConfirm={() => {
            setBatchConfirm(null);
            batchConfirmCopy.run();
          }}
          onOpenChange={(nextOpen) => {
            if (!nextOpen && !batchBusy) setBatchConfirm(null);
          }}
        />
      ) : null}

      <Modal.Backdrop
        isOpen={batchProgressOpen}
        onOpenChange={(next) => {
          if (!next && !batchBusy) setBatchProgressOpen(false);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>
                {t("clientMarket.batchProgressTitle", { action: batchActionLabel })}
              </Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid max-h-[65vh] gap-2 overflow-y-auto">
              {batchProgressItems.map((item) => (
                <div
                  key={item.hostId}
                  className="grid grid-cols-[minmax(0,1fr)_auto] items-start gap-3 rounded-md border px-3 py-2 text-xs"
                >
                  <div className="min-w-0">
                    <div className="truncate font-medium text-foreground">{item.label}</div>
                    {item.detail ? (
                      <div className="mt-0.5 whitespace-normal break-words text-muted-foreground">{item.detail}</div>
                    ) : null}
                  </div>
                  <span
                    className={
                      item.status === "failed"
                        ? "text-rose-600"
                        : item.status === "succeeded"
                          ? "text-emerald-700"
                          : item.status === "running"
                            ? "text-primary"
                            : "text-muted-foreground"
                    }
                  >
                    {item.status === "queued"
                      ? t("clientMarket.batchStatus.queued")
                      : item.status === "running"
                        ? t("clientMarket.batchStatus.running")
                        : item.status === "succeeded"
                          ? t("clientMarket.batchStatus.succeeded")
                          : item.status === "failed"
                            ? t("clientMarket.batchStatus.failed")
                            : t("clientMarket.batchStatus.skipped")}
                  </span>
                </div>
              ))}
            </Modal.Body>
            <Modal.Footer>
              <Button
                variant="primary"
                isDisabled={batchBusy}
                onClick={() => setBatchProgressOpen(false)}
              >
                {batchBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("common.close")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </div>
  );
}
