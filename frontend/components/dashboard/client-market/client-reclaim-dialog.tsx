"use client";
/*
 * A seller takes a rented Client back. By default the tenant gets 24 hours
 * to export the workspace; reclaiming at once is only for a documented tenant
 * violation. Either way the tenant's unused rent is returned in full.
 */
import * as React from "react";
import { Button, Checkbox, Modal, TextArea } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { onlineMarketError, reclaimOnlineLease, type ReclaimReason } from "@/lib/online-client-market";
import { cn } from "@/lib/utils";

/** `escalate`: a notice is already running; only an immediate reclaim is left. */
export type ClientReclaimTarget = {
  leaseId: string;
  name: string;
  escalate?: boolean;
  /** Not yet opened: there is no workspace to hand over, so it ends at once. */
  starting?: boolean;
};

export function ClientReclaimDialog({
  target,
  onClose,
  onDone,
}: {
  target: ClientReclaimTarget | null;
  onClose: () => void;
  onDone: () => void;
}) {
  const { t } = useLocaleText();
  const [mode, setMode] = React.useState<"notice" | "immediate">("notice");
  const [reason, setReason] = React.useState<ReclaimReason>("need_machine");
  const [note, setNote] = React.useState("");
  const [deny, setDeny] = React.useState(false);
  const [unlist, setUnlist] = React.useState(true);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    setMode(target?.escalate ? "immediate" : "notice");
    setReason("need_machine");
    setNote("");
    setDeny(false);
    setUnlist(true);
    setBusy(false);
    setError("");
  }, [target?.leaseId, target?.escalate]);
  const immediate = mode === "immediate";
  // Reclaiming at once is only for a documented violation.
  const effectiveReason: ReclaimReason = immediate ? "tenant_violation" : reason;
  const noteLength = [...note.trim()].length;
  const valid = noteLength <= 2000 && (!immediate || noteLength > 0);
  async function submit() {
    if (!target || busy || !valid) return;
    setBusy(true);
    setError("");
    try {
      await reclaimOnlineLease(target.leaseId, {
        mode,
        reason: effectiveReason,
        note: note.trim(),
        denyFutureAccess: deny,
        unlistAfter: unlist,
      });
      onDone();
      onClose();
    } catch (cause) {
      setError(onlineMarketError(cause, t));
    } finally {
      setBusy(false);
    }
  }
  const option = (value: "notice" | "immediate", label: string, hint: string) => (
    <label
      key={value}
      className={cn(
        "grid min-h-11 cursor-pointer content-start gap-0.5 rounded-xl border px-3 py-2.5 transition-colors duration-200 has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-accent has-[:focus-visible]:ring-offset-2",
        mode === value
          ? value === "immediate"
            ? "border-rose-300 bg-rose-50"
            : "border-accent bg-accent/5"
          : "border-border hover:border-accent/30",
        busy && "cursor-not-allowed opacity-60",
      )}
    >
      <input
        type="radio"
        className="sr-only"
        name="client-reclaim-mode"
        value={value}
        checked={mode === value}
        disabled={busy}
        onChange={() => setMode(value)}
      />
      <span className={cn("text-sm font-semibold", mode === value && (value === "immediate" ? "text-rose-700" : "text-accent"))}>{label}</span>
      <span className="text-xs leading-5 text-muted-foreground">{hint}</span>
    </label>
  );
  const check = (selected: boolean, onChange: (value: boolean) => void, label: string) => (
    <Checkbox isSelected={selected} isDisabled={busy} onChange={onChange}>
      <Checkbox.Control className="border border-slate-300 bg-white shadow-none">
        <Checkbox.Indicator />
      </Checkbox.Control>
      <Checkbox.Content>
        <span className="text-sm text-foreground">{label}</span>
      </Checkbox.Content>
    </Checkbox>
  );
  return (
    <Modal.Backdrop isOpen={!!target} isDismissable={!busy} onOpenChange={(open) => !open && !busy && onClose()}>
      <Modal.Container placement="center" className="p-0 sm:p-4">
        <Modal.Dialog className="light flex h-dvh w-full max-w-none flex-col overflow-hidden rounded-none bg-card text-card-foreground sm:h-auto sm:max-h-[calc(100dvh-2rem)] sm:max-w-[30rem] sm:rounded-2xl">
          <Modal.CloseTrigger className="!bg-slate-100 !text-slate-700 hover:!bg-slate-200" isDisabled={busy} />
          <Modal.Header>
            <div className="min-w-0 pr-8">
              <Modal.Heading>{t("clientMarket.reclaim.title")}</Modal.Heading>
              <p className="mt-1 truncate text-xs text-muted-foreground" title={target?.name}>{target?.name}</p>
            </div>
          </Modal.Header>
          <Modal.Body className="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto overscroll-contain text-sm">
            <fieldset className="grid gap-2" disabled={busy}>
              <legend className="mb-2 text-xs font-medium text-muted-foreground">{t("clientMarket.reclaim.mode")}</legend>
              <div className={cn("grid gap-2", !target?.escalate && "sm:grid-cols-2")}>
                {target?.escalate
                  ? null
                  : target?.starting
                    ? option("notice", t("clientMarket.reclaim.cancelOrder"), t("clientMarket.reclaim.cancelOrderHint"))
                    : option("notice", t("clientMarket.reclaim.notice"), t("clientMarket.reclaim.noticeHint"))}
                {option("immediate", t("clientMarket.reclaim.immediate"), t("clientMarket.reclaim.immediateHint"))}
              </div>
            </fieldset>
            {!immediate ? (
              <div className="grid gap-1.5">
                <span className="text-xs font-medium text-muted-foreground">{t("clientMarket.reclaim.reason")}</span>
                <CompactSelect
                  value={reason}
                  ariaLabel={t("clientMarket.reclaim.reason")}
                  disabled={busy}
                  triggerClassName="min-h-11 text-sm"
                  onChange={(value) => setReason(value as ReclaimReason)}
                  options={(["need_machine", "tenant_violation", "other"] as const).map((value) => ({
                    value,
                    label: t(`clientMarket.reclaim.reason.${value}`),
                  }))}
                />
              </div>
            ) : null}
            <label className="grid gap-1.5">
              <span className="flex items-baseline justify-between gap-2 text-xs font-medium text-muted-foreground">
                {t(immediate ? "clientMarket.reclaim.noteRequired" : "clientMarket.reclaim.note")}
                <span className="tabular-nums">{noteLength}/2000</span>
              </span>
              <TextArea
                value={note}
                rows={3}
                maxLength={2000}
                disabled={busy}
                aria-invalid={immediate && noteLength === 0}
                placeholder={t("clientMarket.reclaim.notePlaceholder")}
                onChange={(event) => setNote(event.target.value)}
              />
            </label>
            <div className="grid gap-2">
              {check(deny, setDeny, t("clientMarket.reclaim.deny"))}
              {check(unlist, setUnlist, t("clientMarket.reclaim.unlist"))}
            </div>
            <p className="rounded-lg bg-muted/60 px-3 py-2 text-xs leading-5 text-muted-foreground">{t("clientMarket.reclaim.refund")}</p>
            {error ? <p role="alert" className="text-destructive">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer className="flex-wrap justify-between gap-2 border-t border-border pt-3">
            <Button className="min-h-11" variant="ghost" isDisabled={busy} onPress={onClose}>
              {t("common.cancel")}
            </Button>
            <Button className="min-h-11" variant={immediate ? "danger" : "primary"} isDisabled={busy || !valid} onPress={() => void submit()}>
              {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
              {t(immediate ? "clientMarket.reclaim.confirmImmediate" : target?.starting ? "clientMarket.reclaim.confirmCancelOrder" : "clientMarket.reclaim.confirmNotice")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
