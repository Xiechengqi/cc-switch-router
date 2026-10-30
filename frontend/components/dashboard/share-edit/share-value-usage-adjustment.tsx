"use client";
import * as React from "react";
import {Button,Input} from "@heroui/react";
import type {TFn} from "../share-dashboard-utils";
import type {ShareUserGrant} from "@/lib/types";
import {editValueUsage,formatQuotaPicos,usdUsageToPicos,type ValueQuotaStatus,type ValueUsageEdit} from "@/lib/share-value-quota";

export function ShareValueUsageAdjustment({shareId,revision,grant,status,t,onUpdated}:{
  shareId:string;revision:number;grant:ShareUserGrant;status:ValueQuotaStatus;t:TFn;onUpdated:(items:ValueQuotaStatus[])=>void;
}) {
  const [target,setTarget]=React.useState("");const [reason,setReason]=React.useState("");
  const [busy,setBusy]=React.useState(false);const [error,setError]=React.useState("");
  const retry=React.useRef<{intent:string;edit:ValueUsageEdit}|undefined>(undefined);
  const submit=async(action:"set"|"clear")=>{
    const targetPicos=action==="set"?usdUsageToPicos(target):undefined;
    if(!reason.trim() || (action==="set" && (targetPicos==null || BigInt(targetPicos)<BigInt(status.observedPicos)))) {
      setError(t("dashboard.userLimit.value.adjustmentInvalid"));return;
    }
    const intent=JSON.stringify([action,targetPicos,reason,grant.revision,status.windowId,status.usageWatermark]);
    const edit=retry.current?.intent===intent?retry.current.edit:{action,targetPicos,reason:reason.trim(),
      expectedGrantRevision:grant.revision??0,windowId:status.windowId,expectedWatermark:status.usageWatermark,operationId:crypto.randomUUID()};
    retry.current={intent,edit};setBusy(true);setError("");
    try {const r=await editValueUsage(shareId,revision,{[grant.email]:edit});retry.current=undefined;onUpdated(r.items);}
    catch(e){setError(e instanceof Error?e.message:String(e));}finally{setBusy(false);}
  };
  return <details className="mt-2 text-xs"><summary>{t("dashboard.userLimit.value.adjustment")}</summary>
    <p className="my-2">{t("dashboard.userLimit.value.observed")}: {formatQuotaPicos(status.observedPicos)} · {t("dashboard.userLimit.value.offset")}: {formatQuotaPicos(status.manualOffsetPicos)}</p>
    <div className="flex flex-wrap gap-2"><Input aria-label={t("dashboard.userLimit.value.target")} placeholder={t("dashboard.userLimit.value.target")} inputMode="decimal" value={target} onChange={e=>setTarget(e.target.value)} disabled={busy}/>
    <Input aria-label={t("dashboard.userLimit.value.reason")} placeholder={t("dashboard.userLimit.value.reason")} maxLength={1000} value={reason} onChange={e=>setReason(e.target.value)} disabled={busy}/>
    <Button size="sm" isDisabled={busy} onPress={()=>void submit("set")}>{t("common.save")}</Button>
    <Button size="sm" variant="secondary" isDisabled={busy} onPress={()=>void submit("clear")}>{t("dashboard.userLimit.value.clearOffset")}</Button></div>
    {error?<p role="alert" className="mt-2 text-danger">{error}</p>:null}
  </details>;
}
