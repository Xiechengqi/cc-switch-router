"use client";

import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2, Plus, UserRoundX } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";

export type UserBlacklistEntry = {
  id: string;
  email: string;
  reason: string;
  createdAt: string;
};

function parseEmails(raw: string): string[] {
  const seen = new Set<string>();
  const emails: string[] = [];
  for (const part of raw.split(/[\s,;]+/)) {
    const email = part.trim().toLowerCase();
    if (!email || !email.includes("@") || seen.has(email)) continue;
    seen.add(email);
    emails.push(email);
  }
  return emails;
}

/** Shared Share/Client Market user blacklist panel. */
export function UserBlacklistPanel({
  enabled,
  hosting = false,
  entries,
  loading = false,
  hint,
  empty,
  reasonLabel,
  onAdd,
  onLift,
  onReload,
}: {
  enabled: boolean;
  hosting?: boolean;
  entries: UserBlacklistEntry[];
  loading?: boolean;
  hint: string;
  empty: string;
  reasonLabel: (reason: string) => string;
  onAdd: (emails: string[]) => Promise<void>;
  onLift: (id: string) => Promise<void>;
  onReload?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [dialogOpen, setDialogOpen] = React.useState(false);
  const [draft, setDraft] = React.useState("");
  const [adding, setAdding] = React.useState(false);
  const [liftingId, setLiftingId] = React.useState("");
  const parsed = parseEmails(draft);

  const submit = async () => {
    if (!parsed.length || adding) return;
    setAdding(true);
    try {
      await onAdd(parsed);
      setDraft("");
      setDialogOpen(false);
      onReload?.();
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setAdding(false);
    }
  };

  const lift = async (entry: UserBlacklistEntry) => {
    setLiftingId(entry.id);
    try {
      await onLift(entry.id);
      onReload?.();
    } catch (error) {
      toast.danger(error instanceof Error ? error.message : String(error));
    } finally {
      setLiftingId("");
    }
  };

  if (!enabled) return null;
  if (!hosting && !loading && !entries.length) return null;

  return (
    <section className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-3 rounded-xl border border-border bg-card p-4 shadow-sm">
      <div className="flex min-w-0 flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <UserRoundX className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">
              {t("market.userBlacklist")}
              {entries.length ? ` · ${entries.length}` : ""}
            </h2>
          </div>
          <p className="mt-0.5 text-xs text-muted-foreground">{hint}</p>
        </div>
        <Button
          size="sm"
          variant="outline"
          className="h-8"
          onClick={() => {
            setDraft("");
            setDialogOpen(true);
          }}
        >
          <Plus className="h-4 w-4" />
          {t("common.add")}
        </Button>
      </div>

      {loading && !entries.length ? (
        <div className="flex items-center gap-2 py-2 text-xs text-muted-foreground">
          <Loader2 className="h-3.5 w-3.5 animate-spin" />
          {t("common.loading")}
        </div>
      ) : null}

      {!loading && !entries.length ? (
        <p className="py-1 text-xs text-muted-foreground">{empty}</p>
      ) : null}

      {entries.length ? (
        <div className="overflow-x-auto rounded-md border border-border">
          <table className="min-w-full text-left text-sm">
            <thead className="bg-muted/40 text-xs text-muted-foreground">
              <tr>
                <th className="px-3 py-2 font-medium">{t("market.blacklistCol.email")}</th>
                <th className="px-3 py-2 font-medium">{t("market.blacklistCol.reason")}</th>
                <th className="px-3 py-2 font-medium">{t("market.blacklistCol.since")}</th>
                <th className="px-3 py-2 font-medium">{t("common.actions")}</th>
              </tr>
            </thead>
            <tbody>
              {entries.map((entry) => (
                <tr key={entry.id} className="border-t border-border/80">
                  <td className="max-w-[18rem] truncate px-3 py-2 font-medium" title={entry.email}>
                    {entry.email}
                  </td>
                  <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
                    {reasonLabel(entry.reason)}
                  </td>
                  <td className="whitespace-nowrap px-3 py-2 text-muted-foreground">
                    {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(
                      new Date(entry.createdAt),
                    )}
                  </td>
                  <td className="px-3 py-2">
                    <Button size="sm" variant="outline" isDisabled={!!liftingId} onClick={() => void lift(entry)}>
                      {liftingId === entry.id ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                      {t("market.blacklistUnblock")}
                    </Button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : null}

      <Modal.Backdrop
        isOpen={dialogOpen}
        onOpenChange={(next) => {
          if (!adding) setDialogOpen(next);
        }}
      >
        <Modal.Container placement="center">
          <Modal.Dialog className="light w-[min(480px,calc(100vw-2rem))] max-w-none !bg-white !text-slate-900">
            <Modal.Header>
              <Modal.Heading>{t("market.blacklistAddTitle")}</Modal.Heading>
            </Modal.Header>
            <Modal.Body className="grid gap-3">
              <p className="text-sm text-muted-foreground">{t("market.blacklistAddHint")}</p>
              <textarea
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                rows={5}
                placeholder={t("market.blacklistAddPlaceholder")}
                className="min-h-[7.5rem] w-full rounded-lg border border-border bg-white px-3 py-2 text-sm outline-none focus:ring-2 focus:ring-primary/20"
                disabled={adding}
              />
              {parsed.length ? (
                <p className="text-xs text-muted-foreground">
                  {t("market.blacklistAddCount", { count: parsed.length })}
                </p>
              ) : null}
            </Modal.Body>
            <Modal.Footer>
              <Button variant="outline" isDisabled={adding} onClick={() => setDialogOpen(false)}>
                {t("common.cancel")}
              </Button>
              <Button variant="primary" isDisabled={adding || !parsed.length} onClick={() => void submit()}>
                {adding ? <Loader2 className="h-4 w-4 animate-spin" /> : <Plus className="h-4 w-4" />}
                {t("common.add")}
              </Button>
            </Modal.Footer>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
