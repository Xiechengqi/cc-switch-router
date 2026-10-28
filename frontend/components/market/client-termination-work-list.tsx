"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientTerminationWorkRecords, type ClientTerminationWorkPage } from "@/lib/client-termination";

/** Independent from public Host filters, including deleted/unlisted Hosts. */
export function ClientTerminationWorkList({ hostId, pendingOnly = false, excludeId, revision = 0, onOpen }: {
  hostId?: string; pendingOnly?: boolean; excludeId?: string; revision?: number; onOpen: (id: string) => void;
}) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : undefined;
  const { locale, t } = useLocaleText();
  const scope = JSON.stringify([actor, hostId, pendingOnly]);
  const [loaded, setLoaded] = React.useState<{ scope: string; page: ClientTerminationWorkPage } | null>(null);
  const page = loaded?.scope === scope ? loaded.page : null;
  const pageRef = React.useRef(page); pageRef.current = page;
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [retry, setRetry] = React.useState(0);
  const generation = React.useRef(0);
  const pending = React.useRef(false);
  React.useEffect(() => {
    const gen = ++generation.current;
    const controller = new AbortController();
    pending.current = true; setBusy(true); setError("");
    const refresh = async () => {
      try {
        // Preserve the reader's loaded depth when a summary poll changes the
        // task count. A vanished pending item must not trigger an unbounded
        // search for an anchor that can no longer exist in this queue.
        const depth = Math.max(1, Math.ceil((pageRef.current?.items.length || 0) / 50));
        let value = await getClientTerminationWorkRecords({ hostId, pendingOnly }, controller.signal);
        const items = [...value.items];
        const seen = new Set(items.map(item => item.id));
        const cursors = new Set<string>();
        for (let n = 1; n < depth && value.nextCursor; n++) {
          if (cursors.has(value.nextCursor)) throw new Error("Repeated termination cursor");
          cursors.add(value.nextCursor);
          value = await getClientTerminationWorkRecords({ hostId, pendingOnly, cursor: value.nextCursor }, controller.signal);
          for (const item of value.items) if (!seen.has(item.id)) { seen.add(item.id); items.push(item); }
        }
        if (generation.current === gen) setLoaded({ scope, page: { ...value, items } });
      } catch (cause) {
        if (generation.current === gen && !controller.signal.aborted) setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (generation.current === gen) { pending.current = false; setBusy(false); }
      }
    };
    if (actor) void refresh();
    else { pending.current = false; setBusy(false); }
    return () => { controller.abort(); generation.current++; };
  }, [actor, hostId, pendingOnly, scope, revision, retry]);
  const more = async () => {
    if (!page?.nextCursor || pending.current) return;
    const gen = generation.current; pending.current = true; setBusy(true);
    try {
      const value = await getClientTerminationWorkRecords({ hostId, pendingOnly, cursor: page.nextCursor });
      if (generation.current === gen) {
        if (value.nextCursor === page.nextCursor) throw new Error("Repeated termination cursor");
        const seen = new Set(page.items.map(item => item.id));
        setLoaded({ scope, page: { ...value, items: [...page.items, ...value.items.filter(item => !seen.has(item.id))] } }); setError("");
      }
    } catch (cause) { if (generation.current === gen) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { if (generation.current === gen) { pending.current = false; setBusy(false); } }
  };
  if (!actor) return null;
  const visibleItems = page?.items.filter(item => item.id !== excludeId) || [];
  return <section className="grid min-w-0 gap-2" aria-label={t(pendingOnly ? "clientTermination.queue" : "clientTermination.hostHistory")}>
    {error ? <div className="flex flex-wrap items-center gap-2"><p role="alert" className="break-words text-xs text-danger">{error}</p><Button size="sm" variant="ghost" onPress={() => setRetry(n => n + 1)}>{t("common.reload")}</Button></div> : null}
    {page ? <div className="grid max-h-80 gap-2 overflow-y-auto overscroll-contain">{visibleItems.map(item => <button key={item.id} type="button" className="grid min-w-0 gap-1 rounded-lg border border-border p-3 text-left text-xs transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" onClick={() => onOpen(item.id)}>
      <span className="break-all font-mono font-medium">{item.clientSubdomain || item.installationId}</span>
      <span className="break-all text-muted-foreground">{item.buyerEmail} · {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.cutoffAt))}</span>
      <span>{t(`clientTermination.attention.${item.attention}`)}</span>
    </button>)}{page.nextCursor ? <Button variant="ghost" size="sm" isDisabled={busy} onPress={() => void more()}>{t("marketWallet.more")}</Button> : null}{!visibleItems.length && !page.nextCursor ? <p className="py-2 text-xs text-muted-foreground">{t(pendingOnly ? "clientTermination.noTasks" : "clientTermination.empty")}</p> : null}</div> : !error ? <p role="status" className="text-xs text-muted-foreground">{t("common.loading")}</p> : null}
  </section>;
}
