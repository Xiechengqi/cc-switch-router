"use client";

import { Loader2, RefreshCw, Rocket, RotateCcw } from "lucide-react";
import * as React from "react";
import { readAuthState } from "@/lib/auth";
import { getVersion, restartService, startUpgrade } from "@/lib/api";
import type { VersionResponse } from "@/lib/types";
import { Alert } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ScrollArea } from "@/components/ui/scroll-area";
import { formatDateTime } from "@/lib/utils";

function formatUptime(secs: number) {
  if (!secs || secs < 0) return "--";
  const days = Math.floor(secs / 86400);
  const hours = Math.floor((secs % 86400) / 3600);
  const minutes = Math.floor((secs % 3600) / 60);
  return [days ? `${days}d` : "", hours || days ? `${hours}h` : "", `${minutes}m`].filter(Boolean).join(" ");
}

function formatBytes(bytes?: number | null) {
  if (!bytes) return "--";
  const mib = bytes / 1024 / 1024;
  return mib >= 1 ? `${mib.toFixed(1)} MiB` : `${(bytes / 1024).toFixed(1)} KiB`;
}

export function VersionPanel({ isAdmin }: { isAdmin: boolean }) {
  const [info, setInfo] = React.useState<VersionResponse | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState<string | null>(null);
  const [upgradeOpen, setUpgradeOpen] = React.useState(false);
  const [logs, setLogs] = React.useState<string[]>([]);

  const refresh = React.useCallback(async () => {
    try {
      setInfo(await getVersion());
      setError("");
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  React.useEffect(() => {
    refresh().catch(console.error);
  }, [refresh]);

  async function restart() {
    if (!window.confirm("Restart cc-switch-router now?")) return;
    setBusy("restart");
    setError("");
    try {
      await restartService();
      setLogs((prev) => [...prev, "Restart scheduled. Waiting for service health..."]);
      pollHealthAndReload().catch(console.error);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  }

  async function upgrade() {
    if (!window.confirm("Download latest binary, replace current binary, and restart?")) return;
    setBusy("upgrade");
    setError("");
    setLogs([]);
    setUpgradeOpen(true);
    try {
      const { taskId } = await startUpgrade();
      streamUpgrade(taskId);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(null);
    }
  }

  function streamUpgrade(taskId: string) {
    const token = readAuthState().accessToken;
    const params = new URLSearchParams({ taskId });
    if (token) params.set("accessToken", token);
    const source = new EventSource(`/v1/admin/upgrade/stream?${params}`);
    source.addEventListener("log", (event) => {
      try {
        const data = JSON.parse((event as MessageEvent).data);
        setLogs((prev) => [...prev, `${data.ts || ""} ${data.level || "info"} ${data.message || ""}`.trim()]);
      } catch {
        setLogs((prev) => [...prev, (event as MessageEvent).data]);
      }
    });
    source.addEventListener("done", (event) => {
      setLogs((prev) => [...prev, `done ${(event as MessageEvent).data}`]);
      source.close();
      pollHealthAndReload().catch(console.error);
    });
    source.onerror = () => {
      setLogs((prev) => [...prev, "upgrade stream disconnected"]);
      source.close();
      setBusy(null);
    };
  }

  return (
    <Card className="rounded-lg">
      <CardHeader className="flex-row items-start justify-between gap-4 space-y-0">
        <div>
          <CardTitle>Version</CardTitle>
          <CardDescription>Build, service status, and binary lifecycle.</CardDescription>
        </div>
        <Button variant="outline" size="icon" onClick={() => refresh()} aria-label="Refresh version">
          <RefreshCw className="h-4 w-4" />
        </Button>
      </CardHeader>
      <CardContent className="grid gap-4">
        {error ? <Alert variant="destructive">{error}</Alert> : null}
        <div className="grid gap-3 sm:grid-cols-2">
          <Info label="Current" value={`${info?.version || "--"} (${info?.commit || "--"})`} />
          <Info label="Built" value={formatDateTime(info?.buildTime)} />
          <Info label="Uptime" value={formatUptime(info?.uptimeSecs || 0)} />
          <Info label="Service" value={<Badge variant={info?.service.active ? "success" : "outline"}>{info?.service.manager || "--"} / {info?.service.activeState || (info?.service.active ? "active" : "inactive")}</Badge>} />
          <Info label="Latest binary" value={info?.latest.available ? `available ${formatBytes(info.latest.contentLength)}` : info?.latest.error || "unknown"} />
          <Info label="Binary path" value={isAdmin ? info?.binaryPath || "--" : "admin only"} />
        </div>
        {isAdmin ? (
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" onClick={restart} disabled={!!busy}>
              {busy === "restart" ? <Loader2 className="h-4 w-4 animate-spin" /> : <RotateCcw className="h-4 w-4" />}
              Restart
            </Button>
            <Button onClick={upgrade} disabled={!!busy}>
              {busy === "upgrade" ? <Loader2 className="h-4 w-4 animate-spin" /> : <Rocket className="h-4 w-4" />}
              Upgrade
            </Button>
          </div>
        ) : null}
      </CardContent>
      <Dialog open={upgradeOpen} onOpenChange={setUpgradeOpen}>
        <DialogContent className="max-w-3xl">
          <DialogHeader>
            <DialogTitle>Upgrade Log</DialogTitle>
            <DialogDescription>Live output from the binary upgrade task.</DialogDescription>
          </DialogHeader>
          <ScrollArea className="h-96 rounded-lg border bg-slate-950 p-4 font-mono text-xs text-slate-100">
            <div className="grid gap-2 pr-3">
              {logs.length ? logs.map((line, index) => <div key={`${index}-${line}`}>{line}</div>) : <div>Waiting for log entries...</div>}
            </div>
          </ScrollArea>
        </DialogContent>
      </Dialog>
    </Card>
  );
}

function Info({ label, value }: { label: string; value?: React.ReactNode }) {
  return (
    <div className="rounded-lg border bg-muted/30 p-3">
      <div className="mono-label text-muted-foreground">{label}</div>
      <div className="mt-2 break-words text-sm font-medium">{value || "--"}</div>
    </div>
  );
}

async function pollHealthAndReload(maxAttempts = 60) {
  for (let i = 0; i < maxAttempts; i += 1) {
    await new Promise((resolve) => window.setTimeout(resolve, 1000));
    try {
      const res = await fetch("/v1/healthz", { cache: "no-store" });
      if (res.ok) {
        window.location.reload();
        return;
      }
    } catch {
      // service may be restarting
    }
  }
}
