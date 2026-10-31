// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
"use client";
import * as React from "react";
import {Button,Input} from "@heroui/react";
import {ChevronDown,ChevronRight} from "lucide-react";
import type {TFn} from "../share-dashboard-utils";
import type {ShareUserGrant} from "@/lib/types";
import {InfoHint} from "@/components/common/info-hint";
import {editValueUsage,formatQuotaPicos,renderValueQuotaMessage,usdUsageToPicos,valueQuotaErrorMessage,
  type ValueQuotaStatus,type ValueUsageEdit} from "@/lib/share-value-quota";

/**
 * An immediate, audited reconcile — deliberately not part of the staged Share
 * save. It is collapsed by default; a blocked window opens it from the table.
 */
export function ShareValueUsageAdjustment({shareId,revision,grant,status,t,onUpdated,open,onOpenChange}:{
  shareId:string;revision:number;grant:ShareUserGrant;status:ValueQuotaStatus;t:TFn;onUpdated:(items:ValueQuotaStatus[])=>void;
  open:boolean;onOpenChange:(open:boolean)=>void;
}) {
  const [target,setTarget]=React.useState("");const [reason,setReason]=React.useState("");
  const [busy,setBusy]=React.useState(false);const [error,setError]=React.useState("");const [result,setResult]=React.useState("");
  const retry=React.useRef<{intent:string;edit:ValueUsageEdit}|undefined>(undefined);
  const id=React.useId();
  const blocked=status.meteringStatus==="blocked";
  const submit=async(action:"set"|"clear")=>{
    const targetPicos=action==="set"?usdUsageToPicos(target):undefined;
    if(!reason.trim() || (action==="set" && (targetPicos==null || BigInt(targetPicos)<BigInt(status.observedPicos)))) {
      setError(t("dashboard.userLimit.value.adjustmentInvalid"));return;
    }
    const intent=JSON.stringify([action,targetPicos,reason,grant.revision,status.windowId,status.usageWatermark]);
    const edit=retry.current?.intent===intent?retry.current.edit:{action,targetPicos,reason:reason.trim(),
      expectedGrantRevision:grant.revision??0,windowId:status.windowId,expectedWatermark:status.usageWatermark,operationId:crypto.randomUUID()};
    retry.current={intent,edit};setBusy(true);setError("");setResult("");
    try {
      const r=await editValueUsage(shareId,revision,{[grant.email]:edit});retry.current=undefined;
      // A per-row error leaves the last good status in place and keeps the panel open.
      if(r.items.length)onUpdated(r.items);
      const email=grant.email.trim().toLowerCase();
      const rowError=r.errors.find(item=>item.email===email);
      if(rowError){setError(renderValueQuotaMessage(valueQuotaErrorMessage({code:rowError.code}),t));return;}
      const updated=r.items.find(item=>item.email===email);
      setResult(t("dashboard.userLimit.value.applied",{used:formatQuotaPicos(updated?.usedPicos ?? targetPicos ?? status.observedPicos)}));
      setTarget("");setReason("");onOpenChange(false);
    }
    catch(e){setError(renderValueQuotaMessage(valueQuotaErrorMessage(e),t));}finally{setBusy(false);}
  };
  return <div className="mt-1 grid gap-1 text-xs">
    <div className="flex flex-wrap items-center gap-2">
      <button type="button" aria-expanded={open} aria-controls={`${id}-panel`}
        className={`inline-flex items-center gap-1 rounded px-1 py-0.5 font-medium transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-primary ${blocked?"text-danger":"text-muted-foreground hover:text-foreground"}`}
        onClick={()=>{setResult("");onOpenChange(!open);}}>
        {open?<ChevronDown className="h-3.5 w-3.5" aria-hidden="true"/>:<ChevronRight className="h-3.5 w-3.5" aria-hidden="true"/>}
        {t(blocked?"dashboard.userLimit.value.reconcile":"dashboard.userLimit.value.adjustment")}
      </button>
      {result?<span role="status" className="text-success">{result}</span>:null}
    </div>
    {open?<div id={`${id}-panel`} className="grid gap-2 rounded-md border bg-muted/20 p-2.5">
      <p className="text-muted-foreground">{t("dashboard.userLimit.value.observed")}: {formatQuotaPicos(status.observedPicos)} · {t("dashboard.userLimit.value.offset")}: {formatQuotaPicos(status.manualOffsetPicos)}</p>
      <div className="grid gap-2 sm:grid-cols-2">
        <label className="grid gap-1" htmlFor={`${id}-target`}><span className="text-muted-foreground">{t("dashboard.userLimit.value.target")}</span>
          <Input id={`${id}-target`} inputMode="decimal" autoComplete="off" placeholder={formatQuotaPicos(status.observedPicos)} value={target} onChange={e=>setTarget(e.target.value)} disabled={busy}/></label>
        <label className="grid gap-1" htmlFor={`${id}-reason`}><span className="text-muted-foreground">{t("dashboard.userLimit.value.reason")}</span>
          <Input id={`${id}-reason`} maxLength={1000} value={reason} onChange={e=>setReason(e.target.value)} disabled={busy}/></label>
      </div>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="primary" isDisabled={busy} aria-busy={busy} onPress={()=>void submit("set")}>{t(busy?"dashboard.userLimit.value.applying":"dashboard.userLimit.value.applyNow")}</Button>
        <Button size="sm" variant="secondary" isDisabled={busy} onPress={()=>void submit("clear")}>{t("dashboard.userLimit.value.clearOffset")}</Button>
        <InfoHint label={t("dashboard.userLimit.value.applyNow")}>{t("dashboard.userLimit.value.applyNowHint")}</InfoHint>
      </div>
      {error?<p role="alert" className="text-danger">{error}</p>:null}
    </div>:null}
  </div>;
}
