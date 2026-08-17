"use client";

import { Alert, Button, Card, Chip } from "@heroui/react";
import { Loader2, RefreshCw } from "lucide-react";
import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAdminAudit } from "@/lib/api";
import type { AdminAuditEntry } from "@/lib/types";

export function AdminAuditPanel() {
  const { locale, t } = useLocaleText();
  const [entries, setEntries] = React.useState<AdminAuditEntry[]>([]);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");

  const load = React.useCallback(async () => {
    setLoading(true);
    try {
      const response = await getAdminAudit(100);
      setEntries(response.entries || []);
      setError("");
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    void load();
  }, [load]);

  return (
    <Card className="rounded-lg">
      <Card.Header className="flex-row items-start justify-between gap-4 space-y-0">
        <div>
          <Card.Title>{t("operations.audit.title")}</Card.Title>
          <Card.Description>{t("operations.audit.description")}</Card.Description>
        </div>
        <Button variant="outline" onClick={() => void load()} isDisabled={loading}>
          {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}
          {t("common.reload")}
        </Button>
      </Card.Header>
      <Card.Content className="grid gap-4">
        {error ? <Alert status="danger">{error}</Alert> : null}
        <div className="overflow-x-auto border">
          <table className="w-full min-w-[900px] text-left text-sm">
            <thead className="bg-muted/50 text-xs text-muted-foreground">
              <tr>
                <th className="px-4 py-3 font-medium">{t("operations.audit.time")}</th>
                <th className="px-4 py-3 font-medium">{t("operations.audit.actor")}</th>
                <th className="px-4 py-3 font-medium">{t("operations.audit.action")}</th>
                <th className="px-4 py-3 font-medium">{t("operations.audit.ip")}</th>
                <th className="px-4 py-3 font-medium">{t("operations.audit.details")}</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {entries.map((entry) => (
                <tr key={entry.id} className="align-top">
                  <td className="whitespace-nowrap px-4 py-3 text-xs">{formatAuditTime(entry.createdAt, locale)}</td>
                  <td className="max-w-[220px] px-4 py-3 font-mono text-xs">{entry.actorEmail || "-"}</td>
                  <td className="px-4 py-3"><Chip size="sm" variant="soft">{entry.action}</Chip></td>
                  <td className="whitespace-nowrap px-4 py-3 font-mono text-xs">{entry.ip || "-"}</td>
                  <td className="max-w-[360px] px-4 py-3">
                    <pre className="max-h-28 overflow-auto whitespace-pre-wrap break-all text-xs leading-5 text-muted-foreground">{formatPayload(entry.payloadJson)}</pre>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {!loading && entries.length === 0 ? (
            <div className="px-4 py-12 text-center text-sm text-muted-foreground">{t("operations.audit.empty")}</div>
          ) : null}
        </div>
      </Card.Content>
    </Card>
  );
}

function formatAuditTime(value: string, locale: string) {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString(locale);
}

function formatPayload(value?: string | null) {
  if (!value) return "-";
  try {
    return JSON.stringify(JSON.parse(value), null, 2);
  } catch {
    return value;
  }
}
