"use client";

import * as React from "react";
import { Button, Modal } from "@heroui/react";
import { useAuth } from "@/components/auth/auth-provider";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ProvisionJobLog } from "@/components/dashboard/provision-job-log";
import { cleanupClientMarketProviderRental, getClientMarketJob } from "@/lib/api";
import type { ProvisioningJob } from "@/lib/types";
import { WALLET_INPUT } from "./wallet-primitives";
import { notifyMarketBalanceChanged } from "@/lib/market-balance-events";
import { formatWalletUnits } from "@/lib/market-wallet-money";
import {
  decideClientTermination, disputeClientTermination, getClientTerminationDetail,
  parseTerminationAward, respondClientTermination, terminationActionAllowed,
  terminationExplanationValid, terminationRemainingCompensation, terminationServerNow,
  terminationTimeoutDue, withdrawClientTerminationClaim,
  type ClientTerminationAction, type ClientTerminationDetail, type ClientTerminationRole,
} from "@/lib/client-termination";
import { ClientTerminationWorkList } from "./client-termination-work-list";

/** One stable dialog for Market rows, deep links, wallet/history and Owner tools.
 * It belongs to the page, not a row that a poll or pagination can unmount. */
export function ClientTerminationDialog({ terminationId, initialRole, initialAction, onClose, onChanged }: {
  terminationId: string | null;
  initialRole?: ClientTerminationRole;
  initialAction?: ClientTerminationAction;
  onClose: () => void;
  onChanged?: () => void;
}) {
  const { session } = useAuth();
  const actor = session?.authenticated ? session.user?.id : undefined;
  if (!actor || !terminationId) return null;
  return <TerminationDialogContent key={`${actor}:${terminationId}:${initialRole || "auto"}`} actor={actor} id={terminationId}
    initialRole={initialRole} initialAction={initialAction} onClose={onClose} onChanged={onChanged} />;
}

function TerminationDialogContent({ actor, id, initialRole, initialAction, onClose, onChanged }: {
  actor: string; id: string; initialRole?: ClientTerminationRole; initialAction?: ClientTerminationAction;
  onClose: () => void; onChanged?: () => void;
}) {
  const { locale, t } = useLocaleText();
  const [detail, setDetail] = React.useState<ClientTerminationDetail | null>(null);
  const [role, setRole] = React.useState<ClientTerminationRole | null>(initialRole || null);
  const [action, setAction] = React.useState<ClientTerminationAction | null>(initialAction || null);
  const [note, setNote] = React.useState("");
  const [awardChoice, setAwardChoice] = React.useState("zero");
  const [customAward, setCustomAward] = React.useState("");
  const [responsibility, setResponsibility] = React.useState("provider");
  const [busy, setBusy] = React.useState(false);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [saved, setSaved] = React.useState(false);
  const [reviewRequired, setReviewRequired] = React.useState(false);
  const [receivedAt, setReceivedAt] = React.useState(Date.now());
  const [clock, setClock] = React.useState(Date.now());
  const [job, setJob] = React.useState<ProvisioningJob | null>(null);
  const [jobError, setJobError] = React.useState("");
  const [hostHistoryOpen, setHostHistoryOpen] = React.useState(false);
  const [historyId, setHistoryId] = React.useState(id);
  const mounted = React.useRef(true);
  const sequence = React.useRef(0);
  const requestRef = React.useRef<AbortController | null>(null);
  const busyRef = React.useRef(false);
  const actionRef = React.useRef(action); actionRef.current = action;
  const callbacks = React.useRef({ onChanged, onClose }); callbacks.current = { onChanged, onClose };

  const load = React.useCallback(async (silent = false, preserveError = false) => {
    if (busyRef.current) return;
    requestRef.current?.abort();
    const controller = new AbortController(); requestRef.current = controller;
    const read = ++sequence.current;
    if (!silent) setLoading(true);
    try {
      const value = await getClientTerminationDetail(historyId, controller.signal);
      if (!mounted.current || read !== sequence.current) return;
      // Events are immutable. Keep already loaded older pages when a live read
      // adds a new head page, instead of collapsing the reader back to page one.
      setDetail(previous => {
        if (previous?.id !== value.id || previous.events.length <= value.events.length) return value;
        const seen = new Set(value.events.map(event => event.id));
        return { ...value, events: [...value.events, ...previous.events.filter(event => !seen.has(event.id))], eventsNextCursor: previous.eventsNextCursor };
      });
      setReceivedAt(Date.now()); if (!preserveError) setError("");
      if (!silent) setReviewRequired(false);
      setRole(previous => previous && value.permissions[previous] ? previous
        : (value.attention === "owner_decision" || value.attention === "finance_review") && value.permissions.owner ? "owner"
        : value.permissions.provider ? "provider" : value.permissions.buyer ? "buyer" : "owner");
    } catch (cause) {
      if (mounted.current && !controller.signal.aborted && read === sequence.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (mounted.current && read === sequence.current) setLoading(false);
    }
  }, [historyId]);
  React.useEffect(() => {
    mounted.current = true; setDetail(null); setError(""); void load();
    const timer = window.setInterval(() => {
      setClock(Date.now());
      if (document.visibilityState === "visible" && !actionRef.current && !busyRef.current) void load(true);
    }, 15000);
    return () => { mounted.current = false; sequence.current++; requestRef.current?.abort(); window.clearInterval(timer); };
  }, [load]);
  React.useEffect(() => {
    const timer = window.setInterval(() => setClock(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  const jobId = job?.id;
  const jobFinished = job?.status === "succeeded" || job?.status === "failed";
  React.useEffect(() => {
    if (!jobId || jobFinished) return;
    let active = true;
    let timer: number;
    const poll = async () => {
      try {
        const next = await getClientMarketJob(jobId);
        if (!active || !mounted.current) return;
        setJob(next); setJobError("");
        if (next.status === "succeeded" || next.status === "failed") {
          await load(true); callbacks.current.onChanged?.(); return;
        }
      } catch (cause) { if (active && mounted.current) setJobError(cause instanceof Error ? cause.message : String(cause)); }
      // A transient lookup failure must not orphan an already submitted job.
      if (active && mounted.current) timer = window.setTimeout(() => void poll(), 1500);
    };
    timer = window.setTimeout(() => void poll(), 1500);
    return () => { active = false; window.clearTimeout(timer); };
  }, [jobId, jobFinished, load]);

  const current = detail?.id === historyId ? detail : null;
  const now = current ? terminationServerNow(current, receivedAt, clock) : clock;
  const cap = current ? terminationRemainingCompensation(current) : "0";
  const units = awardChoice === "zero" ? "0" : awardChoice === "full" ? cap : parseTerminationAward(customAward);
  const validAward = units !== null && BigInt(units) <= BigInt(cap)
    && (role !== "provider" || BigInt(units) < BigInt(cap));
  const allowed = !!current && !!role && !!action && terminationActionAllowed(current, role, action, now);
  const close = () => { if (!busyRef.current) callbacks.current.onClose(); };
  const choose = (next: ClientTerminationAction) => {
    // A head read started before editing must not advance the version under
    // this form. Only an explicit reload or post-submit recovery may do that.
    sequence.current++; requestRef.current?.abort();
    setAction(next); setError(""); setSaved(false); setReviewRequired(false); setNote(""); setAwardChoice("zero"); setCustomAward("");
  };
  const money = (value: string) => formatWalletUnits(value, "$", locale);
  const date = (value: string) => new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(new Date(value));
  const submit = async () => {
    if (!current || !role || !action || busyRef.current || reviewRequired || !allowed) return;
    if (action === "decision" && !validAward) { setError(t("clientTermination.invalidAward")); return; }
    busyRef.current = true; sequence.current++; requestRef.current?.abort(); setBusy(true); setError("");
    try {
      if (action === "dispute") await disputeClientTermination(current, note);
      else if (action === "withdraw") await withdrawClientTerminationClaim(current);
      else if (action === "respond") await respondClientTermination(current, note);
      else await decideClientTermination(current, role === "owner", units!, units === "0" ? responsibility : "buyer", note);
      if (!mounted.current) return;
      setAction(null); setSaved(true); setNote(""); notifyMarketBalanceChanged(actor); callbacks.current.onChanged?.();
    } catch (cause) {
      if (mounted.current) { setReviewRequired(true); setError(cause instanceof Error ? cause.message : String(cause)); }
    } finally {
      busyRef.current = false;
      if (mounted.current) { setBusy(false); await load(true, true); }
    }
  };
  const retryCleanup = async () => {
    if (!current?.permissions.retryCleanup || busyRef.current) return;
    busyRef.current = true; setBusy(true); setError(""); setJobError("");
    try {
      const result = await cleanupClientMarketProviderRental(current.installationId, { reason: "provider_release" });
      if (!result.jobId) throw new Error(t("clientMarket.cleanupFailed"));
      // Persist the returned identity in UI state before polling. A failed
      // first lookup must not lose the job or encourage another cleanup POST.
      if (mounted.current) {
        const at = new Date().toISOString();
        setJob({ id: result.jobId, jobType: "cleanup", status: "pending", phase: "pending", log: "", createdAt: at, updatedAt: at });
        callbacks.current.onChanged?.();
      }
    } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { busyRef.current = false; if (mounted.current) { setBusy(false); await load(true, true); } }
  };
  const moreEvents = async () => {
    if (!current?.eventsNextCursor || busyRef.current || loading) return;
    sequence.current++; requestRef.current?.abort();
    busyRef.current = true; setBusy(true);
    try {
      const next = await getClientTerminationDetail(historyId, undefined, current.eventsNextCursor);
      if (mounted.current) {
        setDetail(previous => {
          if (previous?.id !== next.id) return previous;
          const seen = new Set(previous.events.map(event => event.id));
          // Timeline paging cannot silently upgrade an edited decision's
          // financial snapshot/expectedVersion or reset its server clock.
          return { ...previous, events: [...previous.events, ...next.events.filter(event => !seen.has(event.id))], eventsNextCursor: next.eventsNextCursor };
        });
      }
    } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { busyRef.current = false; if (mounted.current) setBusy(false); }
  };

  return <Modal.Backdrop isOpen onOpenChange={open => { if (!open) close(); }}>
    <Modal.Container><Modal.Dialog className="light w-[min(600px,calc(100vw-2rem))] !bg-white !text-slate-900">
      <Modal.Header><Modal.Heading>{t("clientTermination.details")}</Modal.Heading></Modal.Header>
      <Modal.Body className="grid min-w-0 max-h-[70vh] gap-4 overflow-y-auto overscroll-contain">
        {loading && !current ? <p role="status">{t("common.loading")}</p> : null}
        {current && role ? <>
          <div className="grid min-w-0 gap-1">
            <p className="break-all font-mono text-sm font-semibold" data-testid="termination-original-client">{current.clientSubdomain || current.installationId}</p>
            <p className="break-all text-xs text-muted-foreground">{t("clientTermination.originalRental")} · {current.buyerEmail || current.buyerUserId}</p>
            <p className="text-xs text-muted-foreground">{t("clientTermination.cutoff")}: {date(current.cutoffAt)}</p>
            <p className="text-sm font-medium" role="status">{t(`clientTermination.attention.${terminationTimeoutDue(current, now) ? "timeout_pending" : current.attention}`)}</p>
            <p className="text-xs text-muted-foreground">{t(`clientTermination.${current.status}`)}</p>
          </div>
          <dl className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-2 text-sm tabular-nums">{([
            ["clientTermination.return", current.returnedUnits], ["clientTermination.compensation", current.compensationUnits],
            ["clientTermination.held", current.responsibility === "undetermined" ? cap : current.heldUnits],
          ] as const).map(([key, value]) => <React.Fragment key={key}><dt>{t(key)}</dt><dd className="text-right">{money(value)}</dd></React.Fragment>)}</dl>
          <p className="text-xs">{t("clientTermination.reason")}: {t(current.reason === "provider_issue" ? "clientTermination.providerIssue" : current.reason === "platform_issue" ? "clientTermination.platformIssue" : current.reason === "voluntary" ? "clientTermination.voluntary" : "clientTermination.systemExit")}</p>
          {current.claim ? <p className="whitespace-pre-wrap break-words rounded-lg bg-muted p-3 text-sm">{current.claim}</p> : null}
          {current.disputeDeadlineAt && current.responsibility === "undetermined" && !current.providerRespondedAt ? <p className="text-xs">{t("clientTermination.responseDeadline")}: {date(current.disputeDeadlineAt)}</p> : null}
          {(["buyer", "provider", "owner"] as const).filter(value => current.permissions[value]).length > 1 ? <label className="grid gap-1 text-xs">{t("clientTermination.actingAs")}<select aria-label={t("clientTermination.actingAs")} className={WALLET_INPUT} value={role} disabled={busy || !!action} onChange={e => setRole(e.target.value as ClientTerminationRole)}>
            {(["buyer", "provider", "owner"] as const).filter(value => current.permissions[value]).map(value => <option key={value} value={value}>{t(`clientTermination.role.${value}`)}</option>)}
          </select></label> : null}
          {!action ? <div className="flex flex-wrap gap-2">{([
            ["decision", role === "owner" ? "clientTermination.resolve" : "clientTermination.waive"],
            ["respond", "clientTermination.respond"], ["dispute", "clientTermination.dispute"], ["withdraw", "clientTermination.withdraw"],
          ] as const).filter(([next]) => terminationActionAllowed(current, role, next, now)).map(([next,label]) => <Button key={next} size="sm" variant="outline" className="h-auto min-h-11 max-w-full whitespace-normal py-2" isDisabled={busy || loading} onPress={() => choose(next)}>{t(label)}</Button>)}</div> : null}
          {!action && current.permissions.retryCleanup ? <div className="grid gap-2"><p className="text-xs text-muted-foreground">{t("clientTermination.retryHelp")}</p><Button variant="outline" className="justify-self-start whitespace-nowrap" isDisabled={busy || job?.status === "running" || job?.status === "pending"} onPress={() => void retryCleanup()}>{t("clientMarket.retryCleanup")}</Button></div> : null}
          {current.status === "finance_pending" ? <div className="grid gap-2 rounded-lg bg-muted p-3 text-xs"><p>{t("clientTermination.financeHelp")}</p>{current.permissions.owner ? <a href="/settings/?panel=market_finance" className="underline">{t("clientTermination.openFinance")}</a> : null}</div> : null}
          {action ? <section className="grid gap-3 rounded-lg border border-border p-3" aria-label={t("clientTermination.handle")}>
            {action === "withdraw" ? <p className="text-sm leading-6">{t("clientTermination.withdrawHelp", { amount: money(cap) })}</p>
              : action === "decision" ? <>
                <label className="grid gap-1 text-sm">{t("clientTermination.award")}<select aria-label={t("clientTermination.award")} className={WALLET_INPUT} disabled={busy} value={awardChoice} onChange={e => setAwardChoice(e.target.value)}><option value="zero">{t("clientTermination.waiveAll")}</option>{role === "owner" ? <option value="full">{t("clientTermination.fullAward")}</option> : null}<option value="custom">{t("clientTermination.customAward")}</option></select></label>
                {awardChoice === "custom" ? <label className="grid gap-1 text-sm">{t("clientTermination.customAward")}<input aria-label={t("clientTermination.customAward")} className={WALLET_INPUT} disabled={busy} inputMode="decimal" value={customAward} onChange={e => setCustomAward(e.target.value)} /></label> : null}
                {role === "owner" && awardChoice === "zero" ? <label className="grid gap-1 text-sm">{t("clientTermination.responsibility")}<select aria-label={t("clientTermination.responsibility")} className={WALLET_INPUT} disabled={busy} value={responsibility} onChange={e => setResponsibility(e.target.value)}><option value="provider">{t("clientTermination.provider")}</option><option value="platform">{t("clientTermination.platform")}</option><option value="none">{t("clientTermination.none")}</option></select></label> : null}
                {validAward ? <p className="text-xs leading-5" data-testid="termination-award-preview">{t("clientTermination.awardPreview", { returned: money((BigInt(cap)-BigInt(units!)).toString()), award: money(units!) })}</p> : <p role="alert" className="text-xs text-danger">{t("clientTermination.invalidAward")}</p>}
              </> : <p className="text-xs leading-5">{t(action === "respond" ? "clientTermination.responseHelp" : "clientTermination.claimHelp")}</p>}
            {action !== "withdraw" ? <label className="grid gap-1 text-sm">{t(action === "dispute" ? "clientTermination.claim" : "clientTermination.note")}<textarea aria-label={t(action === "dispute" ? "clientTermination.claim" : "clientTermination.note")} className={WALLET_INPUT} value={note} disabled={busy} maxLength={2000} onChange={e => setNote(e.target.value)} /></label> : null}
            {action !== "withdraw" && !terminationExplanationValid(note) ? <p role="alert" className="text-xs text-danger">{t("clientTermination.explanationTooLong")}</p> : null}
            {!allowed || reviewRequired ? <p role="status" className="text-xs">{t("clientTermination.changed")}</p> : null}
            <div className="flex flex-wrap justify-end gap-2"><Button variant="ghost" isDisabled={busy} onPress={() => { setAction(null); setError(""); }}>{t("common.cancel")}</Button><Button variant="primary" className="whitespace-nowrap" isDisabled={busy || loading || reviewRequired || !allowed || action === "decision" && !validAward || action !== "withdraw" && !terminationExplanationValid(note, action !== "dispute")} onPress={() => void submit()}>{t(busy ? "common.loading" : "common.save")}</Button></div>
          </section> : null}
          {job ? <ProvisionJobLog log={job.log} intent="release" phase={job.status === "succeeded" ? "success" : job.status === "failed" ? "failed" : "running"} /> : null}
          {jobError ? <p role="alert" className="break-words text-xs text-danger">{jobError}</p> : null}
          {current.chatAvailable ? <div className="grid gap-1 text-xs"><a className="justify-self-start underline" href={`/clients/?chat=${encodeURIComponent(current.installationId)}`} target="_blank" rel="noopener noreferrer">{t("clientTermination.chat")}</a><p className="text-muted-foreground">{t("clientTermination.chatHelp")}</p></div> : null}
          <details className="min-w-0 text-xs"><summary className="min-h-9 cursor-pointer py-2 font-medium">{t("clientTermination.timeline")}</summary><ol className="grid gap-3 py-2">{current.events.map(event => <li key={event.id} className="grid min-w-0 gap-1 border-l-2 border-border pl-3"><p>{t(`clientTermination.event.${["accepted","provider_waiver","owner_decision","provider_response","buyer_withdrawal","timeout_waiver","disputed"].includes(event.action) ? event.action as "accepted" : "other"}`)} · {date(event.createdAt)}</p>{event.note ? <p className="whitespace-pre-wrap break-words text-muted-foreground">{event.note}</p> : null}</li>)}</ol>{current.eventsNextCursor ? <Button size="sm" variant="ghost" isDisabled={busy || loading} onPress={() => void moreEvents()}>{t("marketWallet.more")}</Button> : null}</details>
          {!action && (current.permissions.provider || current.permissions.owner) ? <details className="min-w-0 text-xs" open={hostHistoryOpen} onToggle={event => setHostHistoryOpen(event.currentTarget.open)}><summary className="min-h-9 cursor-pointer py-2 font-medium">{t("clientTermination.hostHistory")}</summary>{hostHistoryOpen ? <ClientTerminationWorkList hostId={current.hostId} excludeId={current.id} revision={current.version} onOpen={next => { if (!busyRef.current) { setHistoryId(next); setAction(null); setSaved(false); setJob(null); setJobError(""); setHostHistoryOpen(false); } }} /> : null}</details> : null}
        </> : null}
        {saved ? <p role="status" className="text-sm text-success">{t("clientTermination.saved")}</p> : null}
        {error ? <p role="alert" className="break-words text-sm text-danger">{error}</p> : null}
      </Modal.Body>
      <Modal.Footer><Button variant="ghost" className="whitespace-nowrap" isDisabled={busy || loading} onPress={() => void load()}>{t("common.reload")}</Button><Button variant="secondary" isDisabled={busy} onPress={close}>{t("common.close")}</Button></Modal.Footer>
    </Modal.Dialog></Modal.Container>
  </Modal.Backdrop>;
}
