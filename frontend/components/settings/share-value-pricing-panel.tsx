"use client";
import * as React from "react";
import { Button, Checkbox, Input } from "@heroui/react";
import { CompactSelect } from "@/components/common/compact-select";
import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {inspectExecutionPricing,newExecutionPriceModel,executionRateMicros,executionRateInput,executionCachePolicy,executionReferenceDraft,
  type ExecutionDraft,type ExecutionModel,type ExecutionRate} from "@/lib/share-value-pricing";
import type { AdminModelPriceListItem } from "@/lib/types";

type Settings = {version:number;revision:string|null;snapshot:unknown;effectiveFromMs:number|null};
// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
function RateField({label,value,onChange,onValidity}:{label:string;value:string|null;onChange:(v:string|null)=>void;onValidity:(ok:boolean)=>void}) {
  const [raw,setRaw]=React.useState(()=>executionRateInput(value));
  const valid=raw==="" || executionRateMicros(raw)!==undefined;
  return <label className="grid min-w-0 gap-1 text-xs">{label}
    <Input inputMode="decimal" value={raw} maxLength={32} aria-invalid={!valid} placeholder="—" onChange={event=>{
      const next=event.target.value;setRaw(next);const parsed=executionRateMicros(next);
      onValidity(next==="" || parsed!==undefined);
      if(next==="" || parsed!==undefined)onChange(next===""?null:parsed!);
    }}/>
  </label>;
}
export function ShareValuePricingPanel({referenceModels=[]}:{referenceModels?:AdminModelPriceListItem[]}) {
  const {t}=useLocaleText();
  const [settings,setSettings]=React.useState<Settings>();
  const [draft,setDraft]=React.useState("");
  const [error,setError]=React.useState("");
  const [busy,setBusy]=React.useState(false);
  const [model,setModel]=React.useState("");
  const [health,setHealth]=React.useState<{[key:string]:number|string|null}>();
  const [reviewed,setReviewed]=React.useState(false);
  const [epoch,setEpoch]=React.useState(0);
  const [invalidRates,setInvalidRates]=React.useState<Set<string>>(new Set());
  const [loadedDraft,setLoadedDraft]=React.useState("");
  const [refreshing,setRefreshing]=React.useState(false);
  const [saved,setSaved]=React.useState(false);
  const [visitedModels,setVisitedModels]=React.useState<Set<string>>(new Set());
  const coverage=React.useMemo(()=>{try{return inspectExecutionPricing(JSON.parse(draft));}catch{return [];}},[draft]);
  const parsed=React.useMemo(()=>{try{const value=JSON.parse(draft);return value?.models && typeof value.models==="object" && !Array.isArray(value.models)?value as ExecutionDraft:undefined;}catch{return undefined;}},[draft]);
  const replaceDraft=(value:string)=>{setDraft(value);setReviewed(false);setSaved(false);setInvalidRates(new Set());setVisitedModels(new Set());setEpoch(n=>n+1);};
  const mutate=(edit:(value:ExecutionDraft)=>void)=>{
    try {const value=JSON.parse(draft) as ExecutionDraft;edit(value);setDraft(JSON.stringify(value,null,2));setReviewed(false);setSaved(false);}
    catch(cause){setError(String(cause));}
  };
  const reference=referenceModels.find(row=>row.modelKey===model.trim() && row.priced && row.ratesMicrosPer1m);
  const discovered=referenceModels.filter(row=>row.discoveredFrom.some(source=>source==="share"||source==="usage"));
  const publicationState=!settings?(error?"unavailable":"loading"):!settings.revision?"unpublished":!health?"unavailable":
    health.publishedRevision!==settings.revision?"changed":(settings.effectiveFromMs??0)>Date.now()?"pendingEffective":
    Number(health.acknowledgedInstallations??0)===0?"pendingAck":"available";
  const addModel=(useReference=false)=>{
    if(invalidRates.size)return;
    try {const value=JSON.parse(draft);const key=model.trim();if(!key || key.length>250 || Object.hasOwn(value.models??{},key))return;
      value.models={...value.models,[key]:useReference && reference?.ratesMicrosPer1m?executionReferenceDraft(reference.ratesMicrosPer1m):newExecutionPriceModel()};replaceDraft(JSON.stringify(value,null,2));setModel("");
    }catch(cause){setError(String(cause));}
  };
  React.useEffect(()=>{
    const controller=new AbortController();
    const loadHealth=()=>void authFetch("/v1/admin/share-value-pricing/health",{signal:controller.signal}).then(parseJson<{[key:string]:number|string|null}>).then(data=>{if(!controller.signal.aborted)setHealth(data);}).catch(()=>{if(!controller.signal.aborted)setHealth(undefined);});
    loadHealth();const timer=window.setInterval(loadHealth,30000);
    void authFetch("/v1/admin/share-value-pricing",{signal:controller.signal}).then(parseJson<Settings>).then((data)=>{
      if(controller.signal.aborted)return;
      setSettings(data);
      const text=JSON.stringify(data.snapshot ?? {schemaVersion:1,tariffPolicyId:"api-equivalent-v1",currency:"USD",models:{},exactAliases:{}},null,2);
      setDraft(text);setLoadedDraft(text);
    }).catch((cause)=>{if(!controller.signal.aborted)setError(String(cause));});
    return ()=>{controller.abort();window.clearInterval(timer);};
  },[]);
  const reload=async()=>{
    setRefreshing(true);setError("");
    try {
      const data=await parseJson<Settings>(await authFetch("/v1/admin/share-value-pricing",{cache:"no-store"}));
      // Never silently discard a local draft, especially after a CAS conflict.
      if((draft!==loadedDraft || invalidRates.size>0) && !window.confirm(t("dashboard.userLimit.value.pricingReloadConfirm")))return;
      const text=JSON.stringify(data.snapshot ?? {schemaVersion:1,tariffPolicyId:"api-equivalent-v1",currency:"USD",models:{},exactAliases:{}},null,2);
      setSettings(data);replaceDraft(text);setLoadedDraft(text);
    }catch(cause){setError(String(cause));}finally{setRefreshing(false);}
  };
  const publish=async()=>{
    if(!settings)return;
    setBusy(true);setError("");
    try {
      const snapshot:unknown=JSON.parse(draft);
      if(!window.confirm(t("dashboard.userLimit.value.pricingHint")))return;
      const data=await parseJson<Settings>(await authFetch("/v1/admin/share-value-pricing",{
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({expectedVersion:settings.version,effectiveFromMs:Date.now()+60_000,snapshot}),
      }));
      const text=JSON.stringify(data.snapshot,null,2);setSettings(data);replaceDraft(text);setLoadedDraft(text);setSaved(true);
      setHealth(previous=>({...previous,publishedRevision:data.revision,effectiveFromMs:data.effectiveFromMs,acknowledgedInstallations:0}));
    }catch(cause){setError(String(cause));}finally{setBusy(false);}
  };
  return <details className="rounded-lg border bg-background p-4">
    <summary className="cursor-pointer font-medium">{t("dashboard.userLimit.value.pricing")}</summary>
    <p className="my-3 text-sm text-muted-foreground">{t("dashboard.userLimit.value.pricingHint")}</p>
    {health ? <div className="my-3 flex flex-wrap gap-x-5 gap-y-2 text-xs">{(["activeUsdGrants","scheduledGrants","shadowGrants","staleGrants","acknowledgedInstallations"] as const).map(key=><span key={key}>{t(`dashboard.userLimit.value.health.${key}`)}: <strong>{health[key]}</strong></span>)}</div>:null}
    <div className="my-3 flex flex-wrap items-center gap-3"><span role="status" className="text-sm">{t(`dashboard.userLimit.value.pricingState.${publicationState}`)}</span>
      <Button size="sm" variant="secondary" isDisabled={refreshing||busy} onClick={()=>void reload()}>{t("common.reload")}</Button></div>
    {settings ? <fieldset className="min-w-0 border-0 p-0" disabled={busy||refreshing}><p className="break-all font-mono text-xs">{settings.revision || "—"} · v{settings.version}{settings.effectiveFromMs?` · ${new Date(settings.effectiveFromMs).toLocaleString()}`:null}</p>
    <div className="mt-3 flex flex-wrap gap-2"><Input className="min-w-0 flex-1" aria-label={t("dashboard.userLimit.value.modelId")} placeholder={t("dashboard.userLimit.value.modelId")} value={model} onChange={event=>setModel(event.target.value)} maxLength={250}/>
      <Button variant="secondary" isDisabled={busy || invalidRates.size>0 || !model.trim()} onClick={()=>addModel()}>{t("dashboard.userLimit.value.addModel")}</Button>
      <Button variant="secondary" isDisabled={busy || invalidRates.size>0 || !reference} onClick={()=>addModel(true)}>{t("dashboard.userLimit.value.referenceDraft")}</Button></div>
    <p className="mt-2 text-xs text-muted-foreground">{t("dashboard.userLimit.value.referenceHint")}</p>
    {discovered.length?<details className="mt-3 text-xs"><summary className="cursor-pointer">{t("dashboard.userLimit.value.discovered")} · {discovered.length}</summary>
      <div className="mt-2 grid max-h-48 gap-1 overflow-auto">{discovered.map(row=><div key={row.modelKey} className="flex min-w-0 flex-wrap items-center gap-2">
        <Button className="min-w-0 max-w-full" size="sm" variant="ghost" onClick={()=>setModel(row.modelKey)}><span className="min-w-0 truncate font-mono">{row.modelKey}</span></Button>
        {!row.priced?<span className="text-danger">{t("dashboard.userLimit.value.noReference")}</span>:null}
      </div>)}</div></details>:null}
    <p className="mt-3 text-xs font-medium">{t("dashboard.userLimit.value.formRates")}</p>
    {parsed ? <div className="mt-2 grid gap-2">{coverage.map(summary=>{
      const price=parsed.models[summary.model];
      if(!price || !price.rates || typeof price.rates!=="object")return null;
      const update=(edit:(p:ExecutionModel)=>void)=>mutate(value=>edit(value.models[summary.model]));
      return <details className="min-w-0 rounded-md border p-3" key={`${epoch}:${summary.model}`} onToggle={event=>{
        if(event.currentTarget.open)setVisitedModels(previous=>new Set(previous).add(summary.model));
      }}>
        <summary className="cursor-pointer break-all text-sm font-medium">{summary.model} <span className="text-muted-foreground">· {summary.rows.length}{summary.problems.length?` · ${t("dashboard.userLimit.value.incomplete")}`:""}</span></summary>
        {visitedModels.has(summary.model)?<><div className="mt-3 grid gap-3 sm:grid-cols-2">
          <label className="grid gap-1 text-xs">{t("dashboard.userLimit.value.contextBasis")}<CompactSelect value={price.contextInputBasis} options={[{value:"rawInput",label:t("dashboard.userLimit.value.rawInput")},{value:"freshInput",label:t("dashboard.userLimit.value.freshInput")}]} ariaLabel={t("dashboard.userLimit.value.contextBasis")} onChange={v=>update(p=>{p.contextInputBasis=v as ExecutionModel["contextInputBasis"];})}/></label>
          <label className="grid gap-1 text-xs">{t("dashboard.userLimit.value.contextThreshold")}<Input type="number" min={1} step={1} value={price.longContextThreshold??""} onChange={e=>{const n=e.target.value;update(p=>{p.longContextThreshold=n===""?null:Number(n);});}}/></label>
          <Checkbox isSelected={price.longContextInclusive===true} onChange={v=>update(p=>{p.longContextInclusive=v;})}><Checkbox.Control><Checkbox.Indicator/></Checkbox.Control><Checkbox.Content className="text-xs">{t("dashboard.userLimit.value.contextInclusive")}</Checkbox.Content></Checkbox>
        </div>
        {Object.entries(price.rates).map(([row,rate])=>rate && typeof rate==="object"?<div className="mt-3 border-t pt-3" key={row}>
          <div className="flex flex-wrap items-center justify-between gap-2"><span className="font-mono text-xs">{row}</span><Button variant="ghost" size="sm" onClick={()=>{update(p=>{delete p.rates[row];});setInvalidRates(previous=>new Set([...previous].filter(key=>{const [m,r]=JSON.parse(key);return m!==summary.model || r!==row;})));}}>{t("common.delete")}</Button></div>
          <div className="mt-2 grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">{(["input","output","cacheRead","cacheWrite5m","cacheWrite1h"] as const).map(field=>{
            const key=JSON.stringify([summary.model,row,field]);
            return <RateField key={key} label={t(`dashboard.userLimit.value.rate.${field}`)} value={rate[field]} onValidity={ok=>setInvalidRates(previous=>{const next=new Set(previous);if(ok)next.delete(key);else next.add(key);return next;})}
              onChange={v=>update(p=>{p.rates[row][field]=v;p.rates[row].cacheWritePolicy=executionCachePolicy(p.rates[row]);})}/>;
          })}</div>
        </div>:null)}
        <div className="mt-3 flex flex-wrap gap-2">{["standard/base","standard/long","priority/base","priority/long","flex/base","flex/long"].filter(row=>!price.rates[row]).map(row=><Button size="sm" variant="secondary" key={row} onClick={()=>update(p=>{p.rates[row]={...newExecutionPriceModel().rates["standard/base"]} as ExecutionRate;})}>+ {row}</Button>)}</div></>:null}
      </details>;
    })}</div>:null}
    <details className="mt-3"><summary className="cursor-pointer text-sm">{t("dashboard.userLimit.value.advanced")}</summary><label className="mt-3 grid gap-2 text-xs">{t("dashboard.userLimit.value.json")}
      <textarea className="h-64 w-full resize-y rounded-md border bg-background p-3 font-mono" value={draft} onChange={(event)=>replaceDraft(event.target.value)} spellCheck={false}/>
    </label></details>
    <p className="mt-2 text-xs text-muted-foreground">{t("dashboard.userLimit.value.matrixHint")}</p>
    {coverage.length ? <details className="mt-3 text-xs"><summary>{t("dashboard.userLimit.value.coverage")} · {coverage.length}</summary>
      <div className="mt-2 max-h-64 overflow-auto">{coverage.slice(0,100).map(row=><div className="border-t py-2" key={row.model}>
        <span className="break-all font-mono">{row.model}</span> · {row.rows.join(", ") || "—"}
        {row.problems.length ? <p className="text-danger">{t("dashboard.userLimit.value.incomplete")}: {row.problems.join(", ")}</p> : null}
      </div>)}</div></details>:null}
    <Checkbox className="mt-3" isSelected={reviewed} onChange={setReviewed}><Checkbox.Control><Checkbox.Indicator/></Checkbox.Control><Checkbox.Content className="text-sm">{t("dashboard.userLimit.value.reviewMatrix")}</Checkbox.Content></Checkbox>
    {invalidRates.size?<p role="alert" className="mt-2 text-sm text-danger">{t("dashboard.userLimit.value.invalidRate")}</p>:null}
    <Button className="mt-3" variant="primary" aria-busy={busy} isDisabled={busy || !reviewed || invalidRates.size>0 || !draft || !coverage.length || coverage.some(row=>row.problems.length>0)} onClick={()=>void publish()}>{t(busy?"dashboard.userLimit.value.publishing":"dashboard.userLimit.value.publish")}</Button>
    {saved?<p role="status" className="mt-2 text-sm">{t("dashboard.userLimit.value.publishedHint")}</p>:null}</fieldset> : null}
    {error ? <p role="alert" className="mt-3 text-sm text-danger">{error}</p> : null}
  </details>;
}
