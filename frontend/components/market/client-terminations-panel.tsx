"use client";
import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { WALLET_INPUT } from "@/components/market/wallet-primitives";
import { decideClientTermination, disputeClientTermination, getClientTermination, getClientTerminations, parseTerminationAward, terminationCanDispute, terminationCanDecide, terminationCanWithdraw, terminationCanRespond, terminationTimeoutDue, terminationRemainingCompensation, terminationExplanationValid, respondClientTermination, withdrawClientTerminationClaim, type ClientTermination, type ClientTerminationPage } from "@/lib/client-termination";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";

type Role = "buyer" | "provider" | "owner";
type Action = "dispute" | "decision" | "respond" | "withdraw";
export function ClientTerminationsPanel({ role }: { role: Role }) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id || session.user?.email : undefined;
  const { locale, t } = useLocaleText();
  const [page, setPage] = React.useState<(ClientTerminationPage & { actor: string; role: Role }) | null>(null);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [selected, setSelected] = React.useState<ClientTermination | null>(null);
  const [action, setAction] = React.useState<Action>("decision");
  const [note, setNote] = React.useState("");
  const [responsibility, setResponsibility] = React.useState("provider");
  const [awardChoice, setAwardChoice] = React.useState("zero");
  const [customAward, setCustomAward] = React.useState("");
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
  const reloadSelected = async () => {
    if (!selected || busy) return;
    const gen = generation.current;
    setBusy(true);
    try {
      const updated = await getClientTermination(selected.installationId);
      if (generation.current !== gen) return;
      setSelected(updated); setError("");
    } catch (cause) { if (generation.current === gen) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { if (generation.current === gen) setBusy(false); }
  };
  const open = (item: ClientTermination, nextAction: Action) => { setSelected(item); setAction(nextAction); setNote(""); setAwardChoice("zero"); setCustomAward(""); setResponsibility("provider"); setError(""); };
  const cap = selected ? terminationRemainingCompensation(selected) : "0";
  const actionLabel = action === "withdraw" ? "clientTermination.withdraw" : action === "respond" ? "clientTermination.respond" : action === "dispute" ? "clientTermination.dispute" : role === "owner" ? "clientTermination.resolve" : "clientTermination.waive";
  const actionAllowed = !!selected && (action === "withdraw" ? terminationCanWithdraw(selected) : action === "respond" ? terminationCanRespond(selected) : action === "dispute" ? terminationCanDispute(selected) : terminationCanDecide(selected));
  const submit = async () => {
    if (!selected || busy) return;
    const gen = generation.current;
    const units = awardChoice === "zero" ? "0" : awardChoice === "full" ? cap : parseTerminationAward(customAward);
    if (action === "decision" && (units === null || BigInt(units) > BigInt(cap))) { setError(t("clientTermination.invalidAward")); return; }
    setBusy(true); setError("");
    try {
      if (action === "dispute") await disputeClientTermination(selected, note);
      else if (action === "withdraw") await withdrawClientTerminationClaim(selected);
      else if (action === "respond") await respondClientTermination(selected, note);
      else await decideClientTermination(selected, role === "owner", units!, units === "0" ? responsibility : "buyer", note);
      if (generation.current !== gen) return;
      if (actor) notifyMarketBalanceChanged(actor);
      setSelected(null); toast.success(t("clientTermination.saved")); await load(true);
    } catch (cause) { if (generation.current === gen) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { if (generation.current === gen) setBusy(false); }
  };
  const current = actor && page?.actor === actor && page.role === role ? page : null;
  if (!actor) return null;
  return <section className="grid min-w-0 gap-3 rounded-xl border border-border bg-card p-4" aria-label={t("clientTermination.title")}>
    <header className="flex flex-wrap items-center justify-between gap-2"><h3 className="font-semibold">{t("clientTermination.title")}</h3><Button size="sm" variant="ghost" className="min-h-11" onPress={() => void load()}>{t("common.reload")}</Button></header>
    <p className="text-xs leading-5 text-muted-foreground">{t("clientTermination.help")}</p>
    {error && !selected ? <p role="alert" className="text-sm text-danger">{error}</p> : null}
    {current?.items.length ? <div className="grid max-h-96 gap-3 overflow-y-auto overscroll-contain" role="region" aria-label={t("clientTermination.title")} tabIndex={0}>
      {current.items.map(item => <article key={item.id} className="grid gap-2 rounded-lg border border-border p-3 text-xs">
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
          {(role === "buyer" ? terminationCanDispute(item) : terminationCanDecide(item)) ? <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item, role === "buyer" ? "dispute" : "decision")}>{t(role === "buyer" ? "clientTermination.dispute" : role === "provider" ? "clientTermination.waive" : "clientTermination.resolve")}</Button> : null}
          {role === "buyer" && terminationCanWithdraw(item) ? <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item, "withdraw")}>{t("clientTermination.withdraw")}</Button> : null}
          {role === "provider" && terminationCanRespond(item) ? <Button size="sm" variant="ghost" className="min-h-11" onPress={() => open(item, "respond")}>{t("clientTermination.respond")}</Button> : null}
        </div>
      </article>)}
      {current.nextCursor ? <Button variant="ghost" size="sm" className="min-h-11" isDisabled={busy} onPress={() => void more()}>{t("marketWallet.more")}</Button> : null}
    </div> : <p className="text-xs text-muted-foreground">{t(current ? "clientTermination.empty" : "common.loading")}</p>}
    <Modal.Backdrop isOpen={!!selected} onOpenChange={next => { if (!next && !busy) {setSelected(null); setError("");} }}>
      <Modal.Container><Modal.Dialog className="light w-[min(560px,calc(100vw-2rem))] !bg-white !text-slate-900">
        <Modal.Header><Modal.Heading>{t(actionLabel)}</Modal.Heading></Modal.Header>
        <Modal.Body className="grid max-h-[65vh] gap-4 overflow-y-auto">
          {action === "withdraw" ? <p className="rounded-lg bg-amber-50 p-3 text-sm leading-6 text-amber-950">{t("clientTermination.withdrawHelp", { amount: formatWalletUnits(cap, "$", locale) })}</p> : action === "dispute" || action === "respond" ? <p className="text-xs leading-5">{t(action === "dispute" ? "clientTermination.claimHelp" : "clientTermination.responseHelp")}</p> : <>
            <label className="grid gap-1 text-sm">{t("clientTermination.award")}<select className={WALLET_INPUT} value={awardChoice} onChange={e => setAwardChoice(e.target.value)}><option value="zero">{t("clientTermination.waiveAll")}</option>{role === "owner" ? <option value="full">{t("clientTermination.fullAward")} · {formatWalletUnits(cap, "$", locale)}</option> : null}<option value="custom">{t("clientTermination.customAward")}</option></select></label>
            {awardChoice === "custom" ? <label className="grid gap-1 text-sm">{t("clientTermination.customAward")}<input className={WALLET_INPUT} inputMode="decimal" value={customAward} onChange={e => setCustomAward(e.target.value)} /></label> : null}
            {role === "owner" && awardChoice === "zero" ? <label className="grid gap-1 text-sm">{t("clientTermination.responsibility")}<select className={WALLET_INPUT} value={responsibility} onChange={e => setResponsibility(e.target.value)}><option value="provider">{t("clientTermination.provider")}</option><option value="platform">{t("clientTermination.platform")}</option><option value="none">{t("clientTermination.none")}</option></select></label> : null}
          </>}
          {action !== "withdraw" ? <label className="grid gap-1 text-sm">{t(action === "dispute" ? "clientTermination.claim" : "clientTermination.note")}<textarea className={WALLET_INPUT} value={note} maxLength={2000} onChange={e => setNote(e.target.value)} /></label> : null}
          {action !== "withdraw" && !terminationExplanationValid(note) ? <p role="alert" className="text-xs text-danger">{t("clientTermination.explanationTooLong")}</p> : null}
          {selected?.chatAvailable && action !== "withdraw" ? <div className="grid gap-1 text-xs leading-5"><p>{t("clientTermination.chatHelp")}</p><a href={`/clients/?chat=${encodeURIComponent(selected.installationId)}`} target="_blank" rel="noopener noreferrer" className="underline">{t("clientTermination.chat")}</a></div> : null}
          {error ? <div className="grid gap-2"><p role="alert" className="text-sm text-danger">{error}</p><Button size="sm" variant="ghost" className="min-h-11" isDisabled={busy} onPress={() => void reloadSelected()}>{t("common.reload")}</Button></div> : null}
        </Modal.Body>
        <Modal.Footer><Button variant="ghost" isDisabled={busy} onPress={() => { setSelected(null); setError(""); }}>{t("common.cancel")}</Button><Button variant="primary" isDisabled={busy || !actionAllowed || (action !== "withdraw" && !terminationExplanationValid(note, action !== "dispute"))} onPress={() => void submit()}>{t(busy ? "common.loading" : action === "withdraw" ? "clientTermination.withdraw" : "common.save")}</Button></Modal.Footer>
      </Modal.Dialog></Modal.Container>
    </Modal.Backdrop>
  </section>;
}
