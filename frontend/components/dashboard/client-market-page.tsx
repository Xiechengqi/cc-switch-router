"use client";

import * as React from "react";
import { Button, Card, Chip, Modal, toast } from "@heroui/react";
import { ChevronDown, Loader2, Plus, RefreshCw, Trash2 } from "lucide-react";
import { useAuth } from "@/components/auth/auth-provider";
import { CopyableCodeField } from "@/components/common/copyable-code-field";
import { ConfirmAlertDialog } from "@/components/common/confirm-alert-dialog";
import { CountryFlag } from "@/components/common/country-flag";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  cleanupClientMarketClient,
  createClientMarketHost,
  deleteClientMarketHost,
  getClientMarketHosts,
  getClientMarketJob,
  getProvisionSshKey,
  reverifyClientMarketHost,
  testClientMarketHostSsh,
} from "@/lib/api";
import type { ClientMarketHost, ProvisionSshKey } from "@/lib/types";
import type { MessageKey } from "@/lib/i18n";

const ROUTER_OPEN_LOGIN_EVENT = "router-open-login";

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
  const { t } = useLocaleText();
  const [sshKey, setSshKey] = React.useState<ProvisionSshKey | null>(null);
  const [sshKeyLoading, setSshKeyLoading] = React.useState(false);
  const [sshKeyOpen, setSshKeyOpen] = React.useState(true);
  const [ip, setIp] = React.useState("");
  const [port, setPort] = React.useState("22");
  const [note, setNote] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [testing, setTesting] = React.useState(false);
  const [error, setError] = React.useState("");

  React.useEffect(() => {
    if (!open) return;
    setError("");
    setBusy(false);
    setTesting(false);
    setSshKeyOpen(true);
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

  const testSsh = async () => {
    if (!ip.trim()) {
      setError(t("clientMarket.testSshNeedIp"));
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
    setBusy(true);
    setError("");
    try {
      await createClientMarketHost({
        ip: ip.trim(),
        port: parsedPort,
        note: note.trim() || undefined,
      });
      toast.success(t("clientMarket.hostAdded"));
      onOpenChange(false);
      setIp("");
      setPort("22");
      setNote("");
      onAdded();
    } catch (err) {
      setError(mapHostError(err instanceof Error ? err.message : String(err)));
    } finally {
      setBusy(false);
    }
  };

  const installCommand = sshKey
    ? authorizedKeysInstallCommand(sshKey.authorizedKeysLine)
    : "";

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Container placement="center">
        <Modal.Dialog className="w-[min(560px,calc(100vw-2rem))] max-w-none">
          <Modal.Header>
            <Modal.Heading>{t("clientMarket.addHostTitle")}</Modal.Heading>
          </Modal.Header>
          <Modal.Body className="grid gap-3">
            <div className="overflow-hidden rounded-xl border border-border">
              <button
                type="button"
                className="flex w-full items-center justify-between gap-3 px-3 py-2.5 text-left text-sm font-medium text-foreground transition-colors hover:bg-muted/60"
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

            <div className="grid grid-cols-[minmax(0,1fr)_9rem] gap-3">
              <label className="grid min-w-0 gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.hostIp")}</span>
                <input
                  value={ip}
                  onChange={(e) => setIp(e.target.value)}
                  className="h-11 w-full rounded-lg border px-3 outline-none focus:ring-2 focus:ring-primary/30"
                  autoComplete="off"
                />
              </label>
              <label className="grid gap-1 text-sm">
                <span className="text-muted-foreground">{t("clientMarket.hostPort")}</span>
                <input
                  value={port}
                  onChange={(e) => setPort(e.target.value)}
                  className="h-11 w-full rounded-lg border px-3 outline-none focus:ring-2 focus:ring-primary/30"
                  inputMode="numeric"
                  min={1}
                  max={65535}
                />
              </label>
            </div>
            <label className="grid gap-1 text-sm">
              <span className="text-muted-foreground">{t("clientMarket.hostNote")}</span>
              <input
                value={note}
                onChange={(e) => setNote(e.target.value)}
                className="h-11 rounded-lg border px-3 outline-none focus:ring-2 focus:ring-primary/30"
                maxLength={500}
              />
            </label>
            {error ? <p className="text-sm text-rose-600">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer className="flex-wrap">
            <Button variant="ghost" isDisabled={busy || testing} onClick={() => onOpenChange(false)}>
              {t("common.close")}
            </Button>
            <Button
              variant="outline"
              isDisabled={busy || testing || !ip.trim()}
              onClick={() => void testSsh()}
            >
              {testing ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("clientMarket.testSsh")}
            </Button>
            <Button
              variant="primary"
              isDisabled={busy || testing || !ip.trim()}
              onClick={() => void submit()}
            >
              {busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
              {t("clientMarket.addHost")}
            </Button>
          </Modal.Footer>
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
  const countryName = host.countryCode
    ? new Intl.DisplayNames([locale], { type: "region" }).of(host.countryCode) || host.countryCode
    : "—";

  const installationSnippet = host.installationId
    ? `${host.installationId.slice(0, 8)}…`
    : "—";

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

  return (
    <>
      <div className="flex flex-wrap items-center gap-3 rounded-lg border bg-white px-3 py-2 text-sm">
      <CountryFlag code={host.countryCode} className="h-4 w-6 shrink-0 rounded-sm object-cover" />
      <span className="text-xs text-muted-foreground">{countryName}</span>
      <span className="min-w-0 flex-1 truncate font-medium">{host.hostname || host.ip || host.id.slice(0, 8)}</span>
      <Chip size="sm" variant="soft">
        {t(statusLabelKey(host.status))}
      </Chip>
      {host.clientSubdomain ? (
        <span className="max-w-48 truncate font-mono text-xs text-muted-foreground" title={host.installationId}>
          {host.clientSubdomain}
        </span>
      ) : host.installationId ? (
        <span className="font-mono text-xs text-muted-foreground" title={host.installationId}>
          {installationSnippet}
        </span>
      ) : null}
      {host.ip ? (
        <span className="font-mono text-xs text-muted-foreground">
          {host.ip}
          {host.port ? `:${host.port}` : ""}
        </span>
      ) : null}
      {canDelete ? (
        <Button variant="outline" size="sm" isDisabled={busy} onClick={() => setConfirmAction("delete")}>
          <Trash2 className="h-3.5 w-3.5" />
          {t("clientMarket.deleteHost")}
        </Button>
      ) : null}
      {canCleanup ? (
        <Button variant="outline" size="sm" isDisabled={busy} onClick={() => setConfirmAction("cleanup")}>
          {t("clientMarket.cleanup")}
        </Button>
      ) : null}
      {canReverify ? (
        <Button variant="outline" size="sm" isDisabled={busy} onClick={() => void onReverify()}>
          <RefreshCw className="h-3.5 w-3.5" />
          {t("clientMarket.reverifyHost")}
        </Button>
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
    </>
  );
}

export function ClientMarketPage() {
  const { t } = useLocaleText();
  const { session } = useAuth();
  const authed = !!session?.authenticated;
  const viewerEmail = session?.user?.email;
  const isAdmin = !!session?.isAdmin;

  const [hosts, setHosts] = React.useState<ClientMarketHost[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [addOpen, setAddOpen] = React.useState(false);
  const [pendingAddAfterLogin, setPendingAddAfterLogin] = React.useState(false);
  const [mineOnly, setMineOnly] = React.useState(false);
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

  const grouped = React.useMemo(() => {
    const map = new Map<string, ClientMarketHost[]>();
    const visibleHosts = mineOnly && viewerEmail
      ? hosts.filter((host) => host.hostOwnerEmail.toLowerCase() === viewerEmail.toLowerCase())
      : hosts;
    for (const host of visibleHosts) {
      const key = host.hostOwnerEmail;
      const bucket = map.get(key) || [];
      bucket.push(host);
      map.set(key, bucket);
    }
    return Array.from(map.entries()).sort(([a], [b]) => a.localeCompare(b));
  }, [hosts, mineOnly, viewerEmail]);

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
    <div className="mx-auto grid w-[calc(100%-2rem)] max-w-7xl gap-6 pb-10">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-sm font-semibold">{t("clientMarket.hostOwner")}</h2>
        <div className="flex items-center gap-2">
          {authed ? (
            <Button variant={mineOnly ? "primary" : "outline"} size="sm" onClick={() => setMineOnly((value) => !value)}>
              {mineOnly ? t("clientMarket.allHosts") : t("clientMarket.myHosts")}
            </Button>
          ) : null}
          <Button variant="primary" size="sm" onClick={openAddHost}>
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
      ) : grouped.length === 0 ? (
        <p className="text-sm text-muted-foreground">{t("clientMarket.noHosts")}</p>
      ) : (
        <div className="grid gap-4">
          {grouped.map(([owner, ownerHosts]) => (
            <Card key={owner} className="p-4">
              <div className="mb-3 flex flex-wrap items-center justify-between gap-2 text-sm">
                <span className="font-medium text-foreground">{owner}</span>
                <span className="font-mono text-xs text-muted-foreground">
                  {t("clientMarket.ownerCapacity", {
                    idle: ownerHosts.filter((host) => host.status === "idle").length,
                    total: ownerHosts.length,
                  })}
                </span>
              </div>
              <div className="grid gap-2">
                {ownerHosts.map((host) => (
                  <HostRow
                    key={host.id}
                    host={host}
                    viewerEmail={viewerEmail}
                    isAdmin={isAdmin}
                    onChanged={() => void load()}
                  />
                ))}
              </div>
            </Card>
          ))}
        </div>
      )}

      <AddHostDialog open={addOpen} onOpenChange={setAddOpen} onAdded={() => void load()} />
    </div>
  );
}
