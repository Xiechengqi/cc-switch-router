"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getClientTerminations, terminationCanDispute, terminationCanDecide, terminationCanWithdraw, terminationCanRespond, terminationTimeoutDue, type ClientTermination, type ClientTerminationPage, type ClientTerminationAction } from "@/lib/client-termination";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { ClientTerminationDialog } from "./client-termination-dialog";

type Role = "buyer" | "provider" | "owner";
export function ClientTerminationsPanel({ role }: { role: Role }) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id || session.user?.email : undefined;
  const { locale, t } = useLocaleText();
  const [page, setPage] = React.useState<(ClientTerminationPage & { actor: string; role: Role }) | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [selected, setSelected] = React.useState<ClientTermination | null>(null);
  const [action, setAction] = React.useState<ClientTerminationAction | undefined>();
  const generation = React.useRef(0);
  const readSequence = React.useRef(0);
  const pendingRead = React.useRef<number | null>(null);
  const pageRef = React.useRef(page);
  pageRef.current = page;
  const selectedRef = React.useRef(selected);
  selectedRef.current = selected;
  const load = React.useCallback(async (force = false) => {
    if (!actor || (!force && pendingRead.current !== null)) return;
    const gen = generation.current;
    const read = ++readSequence.current;
    pendingRead.current = read;
    try {
      const previous = pageRef.current;
      const anchor = previous?.actor === actor && previous.role === role ? previous.items.at(-1)?.id : undefined;
      let current = await getClientTerminations(role);
      if (!Array.isArray(current.items)) throw new Error("Invalid termination records");
      const items = [...current.items];
      const seen = new Set<string>();
      while (current.nextCursor && anchor && !current.items.some(item => item.id === anchor)) {
        if (seen.has(current.nextCursor)) throw new Error("Repeated termination cursor");
        seen.add(current.nextCursor);
        current = await getClientTerminations(role, current.nextCursor);
        items.push(...current.items);
      }
      if (generation.current !== gen || pendingRead.current !== read) return;
      setPage({ actor, role, items, nextCursor: current.nextCursor });
      if (!selectedRef.current) setError("");
    } catch (cause) {
      if (generation.current === gen && pendingRead.current === read && !selectedRef.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (pendingRead.current === read) pendingRead.current = null;
    }
  }, [actor, role]);
  React.useEffect(() => {
    generation.current++; pendingRead.current = null; setPage(null); setSelected(null); setError(""); setBusy(false);
    if (!actor) return;
    void load();
    const timer = window.setInterval(() => { if (document.visibilityState === "visible") void load(); }, 15000);
    return () => { generation.current++; window.clearInterval(timer); };
  }, [actor, load]);
  const more = async () => {
    if (!actor || !page?.nextCursor || page.actor !== actor || page.role !== role || busy || pendingRead.current !== null) return;
    const gen = generation.current;
    const read = ++readSequence.current;
    pendingRead.current = read;
    setBusy(true);
    try {
      const next = await getClientTerminations(role, page.nextCursor);
      if (gen === generation.current && pendingRead.current === read) setPage({ actor, role, items: [...page.items, ...next.items], nextCursor: next.nextCursor });
    } catch (cause) { if (gen === generation.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally {
      if (pendingRead.current === read) pendingRead.current = null;
      if (gen === generation.current) setBusy(false);
    }
  };
  const open = (item: ClientTermination, nextAction?: ClientTerminationAction) => { setSelected(item); setAction(nextAction); setError(""); };
  const current = actor && page?.actor === actor && page.role === role ? page : null;
  if (!actor) return null;
  return <section className="grid min-w-0 gap-3 rounded-xl border border-border bg-card p-4" aria-label={t("clientTermination.title")}>
    <header className="flex flex-wrap items-center justify-between gap-2"><h3 className="font-semibold">{t("clientTermination.title")}</h3><Button size="sm" variant="ghost" className="min-h-11" onPress={() => void load()}>{t("common.reload")}</Button></header>
    <p className="text-xs leading-5 text-muted-foreground">{t("clientTermination.help")}</p>
    {error && !selected ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
    {current?.items.length ? <div className="grid max-h-96 gap-3 overflow-y-auto overscroll-contain" role="region" aria-label={t("clientTermination.title")} tabIndex={0}>
      {current.items.map(item => <ClientTerminationRecord key={item.id} item={item} role={role} onOpen={open} />)}
      {current.nextCursor ? <Button variant="ghost" size="sm" className="min-h-11" isDisabled={busy} onPress={() => void more()}>{t("marketWallet.more")}</Button> : null}
    </div> : <p className="text-xs text-muted-foreground">{t(current ? "clientTermination.empty" : "common.loading")}</p>}
    <ClientTerminationDialog terminationId={selected?.id || null} initialRole={role} initialAction={action}
      onClose={() => setSelected(null)} onChanged={() => void load(true)} />
  </section>;
}

/** The same settlement facts/actions in an expanded account rental, without a
 * second page-level inbox or one network request per collapsed rental. */
export function ClientTerminationRecord({ item, role, onOpen }: {
  item: ClientTermination;
  role: Role;
  onOpen: (item: ClientTermination, action?: ClientTerminationAction) => void;
}) {
  const { locale, t } = useLocaleText();
  const open = onOpen;
  return <article key={item.id} className="grid min-w-0 gap-2 text-sm">
        <div className="flex flex-wrap justify-between gap-2"><span className="font-mono">{item.clientSubdomain || item.installationId}</span><span>{t(`clientTermination.${item.status}`)}</span></div>
        <p className="break-all text-muted-foreground">{role !== "buyer" ? item.buyerEmail : item.providerEmail}</p>
        <p className="text-muted-foreground">{t("clientTermination.cutoff")}: {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.cutoffAt))}</p>
        <dl className="grid grid-cols-2 gap-x-4 gap-y-1 tabular-nums">{([
          ["clientTermination.return", item.returnedUnits], ["clientTermination.compensation", item.compensationUnits],
          ["clientTermination.held", item.status === "disputed" && BigInt(item.heldUnits) === 0n ? item.compensationUnits : item.heldUnits],
          ["clientTermination.credited", item.creditedUnits],
        ] as const).map(([key, value]) => <React.Fragment key={key}><dt>{t(key)}</dt><dd className="text-right">{formatWalletUnits(value, "$", locale)}</dd></React.Fragment>)}</dl>
        {item.claim ? <p className="whitespace-pre-wrap break-words rounded bg-muted p-2">{item.claim}</p> : null}
        <p>{t("clientTermination.responsibility")}: {t(`clientTermination.${item.responsibility as "buyer" | "provider" | "platform" | "none" | "undetermined"}`)}</p>
        {item.unlockAt ? <p className="text-muted-foreground">{t("clientTermination.deadline")}: {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.unlockAt))}</p> : null}
        {item.decisionNote ? <p className="whitespace-pre-wrap break-words rounded bg-muted p-2">{item.decisionNote}</p> : null}
        {item.responsibility === "undetermined" && item.disputeUsed ? <div className="grid gap-1 rounded-lg bg-amber-50 p-2 text-amber-950">
          {item.disputeDeadlineAt && !item.providerRespondedAt ? <p>{t("clientTermination.responseDeadline")}: {new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(item.disputeDeadlineAt))}</p> : null}
          <p>{t(item.providerRespondedAt ? "clientTermination.responded" : terminationTimeoutDue(item) ? "clientTermination.autoWaiverPending" : "clientTermination.claimHelp")}</p>
          {role === "provider" && !item.providerRespondedAt ? <p>{t("clientTermination.responseHelp")}</p> : null}
        </div> : item.resolution ? <p>{t(`clientTermination.${item.resolution}`)}</p> : null}
        {item.disputeUsed && item.chatAvailable ? <div className="grid gap-1"><p className="text-muted-foreground">{t("clientTermination.chatHelp")}</p><a href={`/clients/?chat=${encodeURIComponent(item.installationId)}`} target="_blank" rel="noopener noreferrer" className="justify-self-start underline">{t("clientTermination.chat")}</a></div> : null}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item)}>{t("clientTermination.rowHistory")}</Button>
          {(role === "buyer" ? terminationCanDispute(item) : terminationCanDecide(item)) ? <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item, role === "buyer" ? "dispute" : "decision")}>{t(role === "buyer" ? "clientTermination.dispute" : role === "provider" ? "clientTermination.waive" : "clientTermination.resolve")}</Button> : null}
          {role === "buyer" && terminationCanWithdraw(item) ? <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item, "withdraw")}>{t("clientTermination.withdraw")}</Button> : null}
          {role === "provider" && terminationCanRespond(item) ? <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item, "respond")}>{t("clientTermination.respond")}</Button> : null}
        </div>
      </article>;
}
