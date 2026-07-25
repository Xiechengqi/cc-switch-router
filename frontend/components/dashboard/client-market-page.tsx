"use client";

import * as React from "react";
import { createPortal } from "react-dom";
import { Button, Checkbox, Chip, Dropdown, Modal, Tabs, toast, Tooltip } from "@heroui/react";
import { ArrowDown, ArrowUp, Check, CheckSquare, ChevronDown, ChevronLeft, ChevronRight, Circle, Download, Loader2, MoreHorizontal, Pencil, Plus, RefreshCw, Trash2, Upload, X } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CompactRegionMultiSelect } from "@/components/common/compact-region-multi-select";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CountryFlag } from "@/components/common/country-flag";
import { ClientMarketBillingBanner } from "@/components/dashboard/client-market-billing-banner";
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
import { billingUrgencyTier } from "@/lib/billing-urgency";
import { mergeBillingMap, mergeHosts } from "@/lib/client-market-refresh";
import { DASHBOARD_ACCOUNT_PATH } from "@/lib/dashboard-nav";
import type {
  ClientMarketBilling,
  ClientMarketHost,
  ClientMarketHostImportResponse,
  ClientMarketHostTransferDocument,
  HostIpIntel,
  ProvisionSshKey,
  ProvisioningJob,
} from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";
import { usePersistentState } from "@/lib/use-persistent-state";

const CLIENT_MARKET_POLL_MS = 20_000;

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

function hostDisplayLabel(host: ClientMarketHost) {
  return host.hostname || host.ip || host.id.slice(0, 8);
}

function hostCanManage(host: ClientMarketHost, viewerEmail?: string | null) {
  if (host.isHostOwner === true) return true;
  // Fallback when API ownership flags lag behind host_owner_email.
  const viewer = viewerEmail?.trim().toLowerCase();
  const owner = host.hostOwnerEmail?.trim().toLowerCase();
  return !!viewer && !!owner && viewer === owner;
}

function hostCanCleanup(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    !!host.installationId &&
    (host.status === "allocated" || host.status === "unreachable" || host.status === "draining") &&
    (hostCanManage(host, viewerEmail) || host.isClientOwner === true)
  );
}

function hostCanReverify(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    hostCanManage(host, viewerEmail) &&
    (host.status === "unreachable" || host.status === "disabled" || host.status === "abnormal")
  );
}

function hostCanDelete(host: ClientMarketHost, viewerEmail?: string | null) {
  return (
    hostCanManage(host, viewerEmail) &&
    !host.installationId &&
    (host.status === "idle" || host.status === "disabled" || host.status === "abnormal")
  );
}

function hostCanExport(host: ClientMarketHost, viewerEmail?: string | null) {
  return hostCanManage(host, viewerEmail) && !!host.ip && host.port != null;
}

function hostExportKey(host: { ip?: string | null; port?: number | null }) {
  if (!host.ip || host.port == null) return "";
  return formatHostEndpoint(host.ip, host.port);
}

/** Fixed line format: ip:port|note|priceCents|periodDays|fingerprint */
type HostTransferLineEntry = ClientMarketHostTransferDocument["hosts"][number];

function formatHostEndpoint(ip: string, port: number) {
  return ip.includes(":") ? `[${ip}]:${port}` : `${ip}:${port}`;
}

function splitHostEndpoint(endpoint: string): { ip: string; port: number } | null {
  const trimmed = endpoint.trim();
  if (!trimmed) return null;
  if (trimmed.startsWith("[")) {
    const close = trimmed.indexOf("]");
    if (close < 1 || trimmed[close + 1] !== ":") return null;
    const ip = trimmed.slice(1, close).trim();
    const port = Number(trimmed.slice(close + 2));
    if (!ip || !Number.isInteger(port) || port <= 0 || port > 65535) return null;
    return { ip, port };
  }
  const idx = trimmed.lastIndexOf(":");
  if (idx <= 0) return null;
  const ip = trimmed.slice(0, idx).trim();
  const port = Number(trimmed.slice(idx + 1));
  if (!ip || !Number.isInteger(port) || port <= 0 || port > 65535) return null;
  return { ip, port };
}

function encodeHostTransferLine(entry: HostTransferLineEntry): string {
  const endpoint = formatHostEndpoint(entry.ip, entry.port);
  const note = entry.note?.trim() || "";
  const price = entry.priceCents != null ? String(entry.priceCents) : "";
  const period = entry.rentalPeriodDays != null ? String(entry.rentalPeriodDays) : "";
  const fingerprint = entry.expectedFingerprint?.trim() || "";
  const status = entry.informationalStatus?.trim();
  const line = `${endpoint}|${note}|${price}|${period}|${fingerprint}`;
  return status ? `${line} # ${status}` : line;
}

function encodeHostTransferDocument(document: ClientMarketHostTransferDocument): string {
  return document.hosts.map(encodeHostTransferLine).join("\n");
}

function parseHostTransferLines(text: string): { document?: ClientMarketHostTransferDocument; errorLine?: string } {
  const hosts: HostTransferLineEntry[] = [];
  const seen = new Set<string>();
  for (const raw of text.split(/\r?\n/)) {
    const trimmed = raw.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const line = trimmed.replace(/\s+#.*$/, "").trim();
    if (!line) continue;
    const [endpointPart, note = "", priceRaw = "", periodRaw = "", fingerprint = ""] = line
      .split("|")
      .map((part) => part.trim());
    const endpoint = splitHostEndpoint(endpointPart);
    if (!endpoint) return { errorLine: trimmed };
    const key = formatHostEndpoint(endpoint.ip, endpoint.port);
    if (seen.has(key)) continue;
    seen.add(key);
    let priceCents: number | undefined;
    let rentalPeriodDays: number | undefined;
    if (priceRaw) {
      if (!/^\d+$/.test(priceRaw)) return { errorLine: trimmed };
      priceCents = Number(priceRaw);
    }
    if (periodRaw) {
      if (!/^\d+$/.test(periodRaw)) return { errorLine: trimmed };
      rentalPeriodDays = Number(periodRaw);
    }
    hosts.push({
      ip: endpoint.ip,
      port: endpoint.port,
      note: note || undefined,
      priceCents,
      rentalPeriodDays,
      expectedFingerprint: fingerprint || undefined,
    });
  }
  if (!hosts.length) return {};
  return { document: { version: 1, hosts } };
}

function cleanupReasonForHost(host: ClientMarketHost) {
  const isClientOwner = host.isClientOwner === true;
  if (host.isHostOwner === true && !isClientOwner) return "provider_release" as const;
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
  selectionMode,
  selected,
  onSelectedChange,
  selectionDisabled,
  onChanged,
  onCreate,
  onUiBusyChange,
}: {
  host: ClientMarketHost;
  billing?: ClientMarketBilling;
  selectionMode: boolean;
  selected: boolean;
  onSelectedChange: (selected: boolean) => void;
  selectionDisabled?: boolean;
  onChanged: () => void;
  onCreate: (host: ClientMarketHost) => void;
  onUiBusyChange?: (hostId: string, busy: boolean) => void;
}) {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const viewerEmail = session?.user?.email;
  const { openTerminal } = useWebTerminal();
  const [busy, setBusy] = React.useState(false);
  const [confirmAction, setConfirmAction] = React.useState<"delete" | "cleanup" | "unpaid" | null>(null);
  const [cleanupJob, setCleanupJob] = React.useState<ProvisioningJob | null>(null);
  const [cleanupOpen, setCleanupOpen] = React.useState(false);
  const [offerOpen, setOfferOpen] = React.useState(false);

  React.useEffect(() => {
    const locked = offerOpen || cleanupOpen || confirmAction != null || busy;
    onUiBusyChange?.(host.id, locked);
    return () => onUiBusyChange?.(host.id, false);
  }, [busy, cleanupOpen, confirmAction, host.id, offerOpen, onUiBusyChange]);
  const canManageHost = hostCanManage(host, viewerEmail);
  const canDelete = hostCanDelete(host, viewerEmail);
  const isClientOwner = host.isClientOwner === true;
  const canCleanup = hostCanCleanup(host, viewerEmail);
  const canMarkUnpaid =
    !!host.installationId &&
    host.status === "allocated" &&
    canManageHost &&
    !isClientOwner;
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
            reason: cleanupReasonForHost(host),
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
  // Keep the subrow in sync with what ClientMarketBillingBanner actually paints:
  // silent (>7d) and non-owner billing render nothing and must not leave an empty row.
  const urgency = billingUrgencyTier(billing);
  const showBillingRow = !!(
    billing &&
    billing.isClientOwner &&
    billing.status !== "released" &&
    (billing.status === "releasing" ||
      billing.status === "release_failed" ||
      (urgency != null && urgency !== "silent"))
  );
  const noteText = host.note?.trim() || "";
  const showSubrow = showBillingRow || !!noteText;
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
          <span className="block truncate text-xs font-medium text-foreground" title={host.hostOwnerEmail}>
            {host.hostOwnerEmail}
          </span>
        </td>
        <td className="max-w-[9rem] whitespace-nowrap px-2 py-2 align-middle">
          <span className="block text-xs font-semibold text-foreground" title={t("clientMarket.currentOffer")}>
            {formatHostOffer(host.priceCents, host.rentalPeriodDays, locale)}
          </span>
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
              {showBillingRow ? (
                <ClientMarketBillingBanner billing={billing} onChanged={onChanged} compact showPayButton />
              ) : null}
              {noteText ? (
                <span className="min-w-0 whitespace-normal break-words" title={noteText}>
                  {noteText}
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
const PAYMENT_FILTER_KEY = "cc_switch_router_client_market_payment_filter_v1";
const STATUS_FILTER_KEY = "cc_switch_router_client_market_status_filter_v2";
const SORT_PREFS_KEY = "cc_switch_router_client_market_sort_v1";
const HOST_PAGE_SIZE = 10;

const PAYMENT_FILTER_KINDS = ["alipay", "wechat", "binance", "crypto", "custom"] as const;

function paymentKindLabelKey(kind: string): MessageKey {
  switch (kind) {
    case "alipay":
      return "billing.payment.alipay";
    case "wechat":
      return "billing.payment.wechat";
    case "binance":
      return "billing.payment.binance";
    case "crypto":
      return "billing.payment.crypto";
    case "custom":
      return "billing.payment.custom";
    default:
      return "billing.payment.custom";
  }
}

function hostSupportsPaymentKind(hostKinds: string[] | undefined, required: string): boolean {
  const kinds = new Set((hostKinds || []).map((kind) => kind.toLowerCase()));
  if (required === "crypto") {
    return kinds.has("crypto") || kinds.has("usdt") || kinds.has("usdc");
  }
  if (required === "usdt" || required === "usdc") {
    return kinds.has(required) || kinds.has("crypto");
  }
  return kinds.has(required);
}

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
  filter,
}: {
  columnKey: HostSortKey;
  sortPrefs: HostSortPrefs;
  onSort: (key: HostSortKey) => void;
  filter?: React.ReactNode;
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
      className="sticky top-0 z-10 border-b border-border bg-card px-2 py-2 text-left text-xs font-medium text-muted-foreground"
    >
      <div className={`grid ${filter ? "gap-1" : "gap-1.5"}`}>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-md px-1 py-0.5 text-left transition-colors hover:bg-muted/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2"
          onClick={() => onSort(columnKey)}
          aria-label={t("clientMarket.sortBy", { column: label })}
        >
          <span className="whitespace-nowrap">{label}</span>
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
        {filter}
      </div>
    </th>
  );
}

export function ClientMarketPage() {
  const { locale, t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const viewerUserId = session?.user?.id;
  const viewerEmail = session?.user?.email;

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [billingByInstallation, setBillingByInstallation] = React.useState<Map<string, ClientMarketBilling>>(new Map());
  const [loading, setLoading] = React.useState(true);
  const [addOpen, setAddOpen] = React.useState(false);
  const [pendingAddAfterLogin, setPendingAddAfterLogin] = React.useState(false);
  const [ownerFilters, setOwnerFilters] = usePersistentState<string[]>(OWNER_FILTER_KEY, []);
  const [regionFilters, setRegionFilters] = usePersistentState<string[]>(REGION_FILTER_KEY, []);
  const [paymentFilters, setPaymentFilters] = usePersistentState<string[]>(PAYMENT_FILTER_KEY, []);
  const [statusFilterRaw, setStatusFilter] = usePersistentState<HostStatusFilter>(STATUS_FILTER_KEY, "all");
  const [sortPrefsRaw, setSortPrefs] = usePersistentState<HostSortPrefs>(SORT_PREFS_KEY, DEFAULT_HOST_SORT);
  const sortPrefs = React.useMemo(() => normalizeHostSortPrefs(sortPrefsRaw), [sortPrefsRaw]);
  const statusFilter = normalizeHostStatusFilter(statusFilterRaw);
  const [page, setPage] = React.useState(1);
  const [error, setError] = React.useState("");
  const [fixedHost, setFixedHost] = React.useState<ClientMarketHost | null>(null);
  const [transferBusy, setTransferBusy] = React.useState(false);
  const [importOpen, setImportOpen] = React.useState(false);
  const [exportOpen, setExportOpen] = React.useState(false);
  const [importText, setImportText] = React.useState("");
  const [exportText, setExportText] = React.useState("");
  const [importResult, setImportResult] = React.useState<ClientMarketHostImportResponse | null>(null);
  const [selectionMode, setSelectionMode] = React.useState(false);
  const [selectedIds, setSelectedIds] = React.useState<Set<string>>(new Set());
  const [batchBusy, setBatchBusy] = React.useState(false);
  const [batchConfirm, setBatchConfirm] = React.useState<"cleanup" | "delete" | "reverify" | null>(null);
  const [batchProgressOpen, setBatchProgressOpen] = React.useState(false);
  const [batchProgressAction, setBatchProgressAction] = React.useState<"cleanup" | "delete" | "reverify">("cleanup");
  const [batchProgressItems, setBatchProgressItems] = React.useState<BatchProgressItem[]>([]);
  const [rowUiBusyCount, setRowUiBusyCount] = React.useState(0);
  const refreshAbortRef = React.useRef<AbortController | null>(null);
  const rowBusyIdsRef = React.useRef<Set<string>>(new Set());

  const setRowUiBusy = React.useCallback((hostId: string, busy: boolean) => {
    const ids = rowBusyIdsRef.current;
    const before = ids.size;
    if (busy) ids.add(hostId);
    else ids.delete(hostId);
    if (ids.size !== before) setRowUiBusyCount(ids.size);
  }, []);

  const refreshPaused =
    batchBusy ||
    transferBusy ||
    addOpen ||
    importOpen ||
    exportOpen ||
    !!fixedHost ||
    batchConfirm != null ||
    batchProgressOpen ||
    rowUiBusyCount > 0;

  const load = React.useCallback(
    async (options?: { silent?: boolean }) => {
      const silent = options?.silent === true;
      // Silent polls never interrupt each other or a visible load.
      if (silent && refreshAbortRef.current) return;
      if (!silent) refreshAbortRef.current?.abort();
      const controller = new AbortController();
      refreshAbortRef.current = controller;
      if (!silent) {
        setLoading(true);
        setError("");
      }
      try {
        const [nextHosts, billing] = await Promise.all([
          getClientMarketHosts(),
          authed ? getMyClientMarketBilling() : Promise.resolve([]),
        ]);
        if (controller.signal.aborted) return;
        setHosts((prev) => mergeHosts(prev, nextHosts));
        setBillingByInstallation((prev) => mergeBillingMap(prev, billing));
      } catch (err) {
        if (controller.signal.aborted) return;
        if (!silent) {
          setError(err instanceof Error ? err.message : String(err));
        }
      } finally {
        if (refreshAbortRef.current === controller) refreshAbortRef.current = null;
        if (!silent) setLoading(false);
      }
    },
    [authed],
  );

  const silentRefresh = React.useCallback(() => load({ silent: true }), [load]);

  React.useEffect(() => {
    void load();
    return () => {
      refreshAbortRef.current?.abort();
    };
  }, [load, viewerUserId]);

  React.useEffect(() => {
    const tick = () => {
      if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
      if (refreshPaused) return;
      void silentRefresh();
    };
    const timer = window.setInterval(tick, CLIENT_MARKET_POLL_MS);
    const onVisibility = () => {
      if (document.visibilityState === "visible") tick();
    };
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [refreshPaused, silentRefresh]);

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

  const paymentOptions = React.useMemo(
    () =>
      PAYMENT_FILTER_KINDS.map((kind) => ({
        value: kind,
        label: t(paymentKindLabelKey(kind)),
      })),
    [t],
  );

  const scopedHosts = React.useMemo(() => {
    const ownerSet = new Set(ownerFilters.map((email) => email.toLowerCase()));
    const regionSet = new Set(regionFilters.map((code) => code.toUpperCase()));
    return hosts.filter((host) => {
      if (ownerSet.size > 0 && !ownerSet.has(host.hostOwnerEmail.toLowerCase())) return false;
      if (regionSet.size > 0) {
        const code = (host.countryCode || "").trim().toUpperCase();
        if (!code || !regionSet.has(code)) return false;
      }
      if (
        paymentFilters.length > 0 &&
        !paymentFilters.every((kind) => hostSupportsPaymentKind(host.paymentMethodKinds, kind))
      ) {
        return false;
      }
      return true;
    });
  }, [hosts, ownerFilters, paymentFilters, regionFilters]);

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
  }, [ownerFilters, paymentFilters, regionFilters, sortPrefs.key, sortPrefs.dir, statusFilter]);

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
    () => selectedHosts.filter((host) => hostCanCleanup(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );
  const reverifyEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanReverify(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );
  const deleteEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanDelete(host, viewerEmail)),
    [selectedHosts, viewerEmail],
  );
  const exportEligible = React.useMemo(
    () => selectedHosts.filter((host) => hostCanExport(host, viewerEmail)),
    [selectedHosts, viewerEmail],
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
      void silentRefresh();
    },
    [silentRefresh, t],
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
    const skippedHosts = selectedHosts.filter((host) => !hostCanCleanup(host, viewerEmail));
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
            reason: cleanupReasonForHost(host),
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
    const skippedHosts = selectedHosts.filter((host) => !hostCanReverify(host, viewerEmail));
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
    const skippedHosts = selectedHosts.filter((host) => !hostCanDelete(host, viewerEmail));
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

  const openExportDialog = async (selectedOnly: boolean) => {
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
      if (!document.hosts.length) {
        toast.danger(t("clientMarket.exportEmpty"));
        return;
      }
      setExportText(encodeHostTransferDocument(document));
      setExportOpen(true);
      if (selectedOnly) clearSelection();
      toast.success(t("clientMarket.exportedHosts", { count: document.hosts.length }));
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setTransferBusy(false);
    }
  };

  const copyExportText = async () => {
    try {
      await navigator.clipboard.writeText(exportText);
      toast.success(t("clientMarket.exportCopied"));
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
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

  const submitImportText = async () => {
    if (new TextEncoder().encode(importText).length > 1024 * 1024) {
      toast.danger(t("clientMarket.importSizeLimit"));
      return;
    }
    const parsed = parseHostTransferLines(importText);
    if (parsed.errorLine) {
      toast.danger(t("clientMarket.importParseError", { line: parsed.errorLine }));
      return;
    }
    if (!parsed.document) {
      toast.danger(t("clientMarket.importEmpty"));
      return;
    }
    setTransferBusy(true);
    try {
      const result = await importMyClientMarketHosts(parsed.document);
      setImportOpen(false);
      setImportText("");
      setImportResult(result);
      await silentRefresh();
    } catch (reason) {
      toast.danger(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setTransferBusy(false);
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
          {authed ? (
            <>
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
                    onClick={() => setImportOpen(true)}
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
                    onClick={() => void openExportDialog(false)}
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
            onClick={() => void openExportDialog(true)}
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
          {scopedHosts.length ||
          ownerFilters.length ||
          regionFilters.length ||
          paymentFilters.length ||
          statusFilter !== "all" ? (
            <button
              type="button"
              className="text-xs font-medium text-primary hover:underline"
              onClick={() => {
                setStatusFilter("all");
                setOwnerFilters([]);
                setRegionFilters([]);
                setPaymentFilters([]);
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
                        <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
                          <Checkbox.Indicator />
                        </Checkbox.Control>
                      </Checkbox>
                    </th>
                  ) : null}
                  <HostSortHeader columnKey="status" sortPrefs={sortPrefs} onSort={toggleHostSort} />
                  <HostSortHeader
                    columnKey="region"
                    sortPrefs={sortPrefs}
                    onSort={toggleHostSort}
                    filter={
                      <CompactRegionMultiSelect
                        compact
                        values={regionFilters}
                        onChange={setRegionFilters}
                        options={regionOptions}
                        allLabel={t("clientMarket.allRegions")}
                        moreLabel={(count) => t("clientMarket.regionsMore", { count })}
                        clearLabel={t("clientMarket.clearRegionSelection")}
                        ariaLabel={t("clientMarket.filterRegions")}
                        className="w-full max-w-[10.5rem]"
                      />
                    }
                  />
                  <HostSortHeader
                    columnKey="owner"
                    sortPrefs={sortPrefs}
                    onSort={toggleHostSort}
                    filter={
                      <CompactRegionMultiSelect
                        compact
                        values={ownerFilters}
                        onChange={setOwnerFilters}
                        options={ownerOptions}
                        allLabel={t("clientMarket.allOwners")}
                        moreLabel={(count) => t("clientMarket.ownersMore", { count })}
                        clearLabel={t("clientMarket.clearOwnerSelection")}
                        ariaLabel={t("clientMarket.filterOwners")}
                        className="w-full max-w-[12rem]"
                      />
                    }
                  />
                  <HostSortHeader
                    columnKey="offer"
                    sortPrefs={sortPrefs}
                    onSort={toggleHostSort}
                    filter={
                      <CompactRegionMultiSelect
                        compact
                        values={paymentFilters}
                        onChange={setPaymentFilters}
                        options={paymentOptions}
                        allLabel={t("clientMarket.allPayments")}
                        moreLabel={(count) => t("clientMarket.paymentsMore", { count })}
                        clearLabel={t("clientMarket.clearPaymentSelection")}
                        ariaLabel={t("clientMarket.filterPayments")}
                        className="w-full max-w-[10.5rem]"
                      />
                    }
                  />
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
                    selectionMode={selectionMode}
                    selected={selectedIds.has(host.id)}
                    onSelectedChange={(next) => setHostSelected(host.id, next)}
                    selectionDisabled={batchBusy}
                    onChanged={() => void silentRefresh()}
                    onCreate={setFixedHost}
                    onUiBusyChange={setRowUiBusy}
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

      <AddHostDialog open={addOpen} onOpenChange={setAddOpen} onAdded={() => void silentRefresh()} />
      <CreateClientDialog
        open={!!fixedHost}
        onOpenChange={(next) => { if (!next) setFixedHost(null); }}
        fixedHost={fixedHost}
        onCreated={() => void silentRefresh()}
      />
      <Modal.Backdrop
        isOpen={importOpen}
        onOpenChange={(next) => {
          if (!next && !transferBusy) {
            setImportOpen(false);
          }
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("clientMarket.importDialogTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p className="text-xs leading-relaxed text-muted-foreground">{t("clientMarket.transferFormatHint")}</p>
              <textarea
                value={importText}
                onChange={(event) => setImportText(event.target.value)}
                placeholder={t("clientMarket.importPlaceholder")}
                spellCheck={false}
                className="min-h-56 w-full resize-y rounded-lg border border-border bg-white px-3 py-2 font-mono text-xs leading-5 text-foreground outline-none focus-visible:ring-2 focus-visible:ring-accent"
              />
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" isDisabled={transferBusy} onClick={() => setImportOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button variant="primary" isDisabled={transferBusy} onClick={() => void submitImportText()}>
                {transferBusy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                {t("clientMarket.importSubmit")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

      <Modal.Backdrop
        isOpen={exportOpen}
        onOpenChange={(next) => {
          if (!next) setExportOpen(false);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(640px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("clientMarket.exportDialogTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p className="text-xs leading-relaxed text-muted-foreground">{t("clientMarket.transferFormatHint")}</p>
              <textarea
                value={exportText}
                readOnly
                spellCheck={false}
                className="min-h-56 w-full resize-y rounded-lg border border-border bg-muted/30 px-3 py-2 font-mono text-xs leading-5 text-foreground outline-none"
              />
            </Modal.Body>
            <Modal.Footer>
              <Button variant="ghost" onClick={() => setExportOpen(false)}>
                {t("common.close")}
              </Button>
              <Button variant="primary" onClick={() => void copyExportText()}>
                {t("clientMarket.exportCopy")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>

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
