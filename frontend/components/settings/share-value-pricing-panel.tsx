"use client";
import * as React from "react";
import { Button, Checkbox, Input, Modal } from "@heroui/react";
import { Trash2 } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { InfoHint } from "@/components/common/info-hint";
import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {inspectExecutionPricing,newExecutionPriceModel,executionRateMicros,executionRateInput,executionCachePolicy,executionReferenceDraft,
  type ExecutionDraft,type ExecutionModel,type ExecutionRate} from "@/lib/share-value-pricing";
import type { AdminModelPriceListItem } from "@/lib/types";
import { cn } from "@/lib/utils";

type Settings = {version:number;revision:string|null;snapshot:unknown;effectiveFromMs:number|null};
type Confirmation = {kind:"publish";snapshot:unknown;models:string[]} | {kind:"reload";data:Settings};
const RATE_FIELDS = ["input","output","cacheRead","cacheWrite5m","cacheWrite1h"] as const;
const TIER_ROWS = ["standard/base","standard/long","priority/base","priority/long","flex/base","flex/long"];
const MODEL_ID_MAX = 250;
const emptySnapshot = () => JSON.stringify({schemaVersion:1,tariffPolicyId:"api-equivalent-v1",currency:"USD",models:{},exactAliases:{}},null,2);
const errorText = (cause: unknown) => cause instanceof Error ? cause.message : String(cause);

// Hallmark · pre-emit critique: P4 H4 E4 S5 R5 V4 · component-scope, existing Router tokens.
/** One editable cell: USD per 1M on screen, exact integer micro-USD on the wire. */
function RateCell({label,value,missing,missingLabel,onChange,onValidity}:{label:string;value:string|null;missing:boolean;missingLabel:string;
  onChange:(v:string|null)=>void;onValidity:(ok:boolean)=>void}) {
  const [raw,setRaw]=React.useState(()=>executionRateInput(value));
  const valid=raw==="" || executionRateMicros(raw)!==undefined;
  return <div className="grid min-w-[5.5rem] gap-0.5">
    <Input inputMode="decimal" value={raw} maxLength={32} aria-label={label} aria-invalid={!valid || undefined} placeholder="—"
      className={cn("h-8 px-2 text-xs tabular-nums", !valid && "border-danger", missing && raw==="" && "border-warning")}
      onChange={event=>{
        const next=event.target.value;setRaw(next);const parsed=executionRateMicros(next);
        onValidity(next==="" || parsed!==undefined);
        if(next==="" || parsed!==undefined)onChange(next===""?null:parsed!);
      }}/>
    {missing && raw==="" ? <span className="text-[10px] font-medium text-warning">{missingLabel}</span> : null}
  </div>;
}
/** Blank means "no long-context tier"; anything else must be a positive whole token count. */
function ThresholdField({value,onChange,onValidity,label,error}:{value:number|null;onChange:(v:number|null)=>void;onValidity:(ok:boolean)=>void;label:string;error:string}) {
  const [raw,setRaw]=React.useState(()=>value==null?"":String(value));
  const parse=(text:string)=>{const trimmed=text.trim();if(!trimmed)return null;
    if(!/^\d+$/.test(trimmed))return undefined;const n=Number(trimmed);return Number.isSafeInteger(n) && n>0?n:undefined;};
  const parsed=parse(raw);
  const id=React.useId();
  return <label className="grid gap-1 text-xs" htmlFor={id}>{label}
    <Input id={id} inputMode="numeric" value={raw} aria-invalid={parsed===undefined || undefined} aria-describedby={parsed===undefined?`${id}-error`:undefined}
      onChange={event=>{const next=event.target.value;setRaw(next);const value=parse(next);onValidity(value!==undefined);if(value!==undefined)onChange(value);}}/>
    {parsed===undefined?<span id={`${id}-error`} className="text-danger">{error}</span>:null}
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
  const [invalidFields,setInvalidFields]=React.useState<Set<string>>(new Set());
  const [loadedDraft,setLoadedDraft]=React.useState("");
  const [refreshing,setRefreshing]=React.useState(false);
  const [saved,setSaved]=React.useState(false);
  const [visitedModels,setVisitedModels]=React.useState<Set<string>>(new Set());
  const [confirmation,setConfirmation]=React.useState<Confirmation|null>(null);
  const coverage=React.useMemo(()=>{try{return inspectExecutionPricing(JSON.parse(draft));}catch{return [];}},[draft]);
  const parsed=React.useMemo(()=>{try{const value=JSON.parse(draft);return value?.models && typeof value.models==="object" && !Array.isArray(value.models)?value as ExecutionDraft:undefined;}catch{return undefined;}},[draft]);
  const replaceDraft=(value:string)=>{setDraft(value);setReviewed(false);setSaved(false);setInvalidFields(new Set());setVisitedModels(new Set());setEpoch(n=>n+1);};
  const mutate=(edit:(value:ExecutionDraft)=>void)=>{
    try {const value=JSON.parse(draft) as ExecutionDraft;edit(value);setDraft(JSON.stringify(value,null,2));setReviewed(false);setSaved(false);}
    catch(cause){setError(errorText(cause));}
  };
  const setValidity=(key:string,ok:boolean)=>setInvalidFields(previous=>{
    if(ok===!previous.has(key))return previous;const next=new Set(previous);if(ok)next.delete(key);else next.add(key);return next;});
  const modelKey=model.trim();
  const modelError=!modelKey?"":modelKey.length>MODEL_ID_MAX?t("dashboard.userLimit.value.modelTooLong")
    :parsed && Object.hasOwn(parsed.models,modelKey)?t("dashboard.userLimit.value.modelDuplicate"):"";
  const reference=referenceModels.find(row=>row.modelKey===modelKey && row.priced && row.ratesMicrosPer1m);
  const discovered=referenceModels.filter(row=>row.discoveredFrom.some(source=>source==="share"||source==="usage"));
  const publicationState=!settings?(error?"unavailable":"loading"):!settings.revision?"unpublished":!health?"unavailable":
    health.publishedRevision!==settings.revision?"changed":(settings.effectiveFromMs??0)>Date.now()?"pendingEffective":
    Number(health.acknowledgedInstallations??0)===0?"pendingAck":"available";
  const addModel=(useReference=false)=>{
    if(invalidFields.size || !modelKey || modelError)return;
    try {const value=JSON.parse(draft);
      value.models={...value.models,[modelKey]:useReference && reference?.ratesMicrosPer1m?executionReferenceDraft(reference.ratesMicrosPer1m):newExecutionPriceModel()};replaceDraft(JSON.stringify(value,null,2));setModel("");
    }catch(cause){setError(errorText(cause));}
  };
  React.useEffect(()=>{
    const controller=new AbortController();
    const loadHealth=()=>void authFetch("/v1/admin/share-value-pricing/health",{signal:controller.signal}).then(parseJson<{[key:string]:number|string|null}>).then(data=>{if(!controller.signal.aborted)setHealth(data);}).catch(()=>{if(!controller.signal.aborted)setHealth(undefined);});
    loadHealth();const timer=window.setInterval(loadHealth,30000);
    void authFetch("/v1/admin/share-value-pricing",{signal:controller.signal}).then(parseJson<Settings>).then((data)=>{
      if(controller.signal.aborted)return;
      setSettings(data);
      const text=data.snapshot?JSON.stringify(data.snapshot,null,2):emptySnapshot();
      setDraft(text);setLoadedDraft(text);
    }).catch((cause)=>{if(!controller.signal.aborted)setError(errorText(cause));});
    return ()=>{controller.abort();window.clearInterval(timer);};
  },[]);
  const applyLoaded=(data:Settings)=>{
    const text=data.snapshot?JSON.stringify(data.snapshot,null,2):emptySnapshot();
    setSettings(data);replaceDraft(text);setLoadedDraft(text);
  };
  const reload=async()=>{
    setRefreshing(true);setError("");
    try {
      const data=await parseJson<Settings>(await authFetch("/v1/admin/share-value-pricing",{cache:"no-store"}));
      // Never silently discard a local draft, especially after a CAS conflict.
      if(draft!==loadedDraft || invalidFields.size>0)setConfirmation({kind:"reload",data});
      else applyLoaded(data);
    }catch(cause){setError(errorText(cause));}finally{setRefreshing(false);}
  };
  const requestPublish=()=>{
    if(!settings)return;
    setError("");
    try {
      const snapshot:unknown=JSON.parse(draft);
      setConfirmation({kind:"publish",snapshot,models:coverage.map(row=>row.model)});
    }catch(cause){setError(errorText(cause));}
  };
  const publish=async(snapshot:unknown)=>{
    if(!settings)return;
    setBusy(true);setError("");setConfirmation(null);
    try {
      const data=await parseJson<Settings>(await authFetch("/v1/admin/share-value-pricing",{
        method:"POST",headers:{"Content-Type":"application/json"},
        body:JSON.stringify({expectedVersion:settings.version,effectiveFromMs:Date.now()+60_000,snapshot}),
      }));
      const text=JSON.stringify(data.snapshot,null,2);setSettings(data);replaceDraft(text);setLoadedDraft(text);setSaved(true);
      setHealth(previous=>({...previous,publishedRevision:data.revision,effectiveFromMs:data.effectiveFromMs,acknowledgedInstallations:0}));
    }catch(cause){setError(errorText(cause));}finally{setBusy(false);}
  };
  const listedModels=confirmation?.kind==="publish"?confirmation.models:[];
  return <details className="rounded-lg border bg-background p-4">
    <summary className="cursor-pointer font-medium">{t("dashboard.userLimit.value.pricing")}</summary>
    <p className="my-3 text-sm text-muted-foreground">{t("dashboard.userLimit.value.pricingHint")}</p>
    {health ? <div className="my-3 flex flex-wrap gap-x-5 gap-y-2 text-xs">{(["activeUsdGrants","scheduledGrants","staleGrants","acknowledgedInstallations"] as const).map(key=><span key={key}>{t(`dashboard.userLimit.value.health.${key}`)}: <strong>{health[key]}</strong></span>)}</div>:null}
    <div className="my-3 flex flex-wrap items-center gap-3"><span role="status" className="text-sm">{t(`dashboard.userLimit.value.pricingState.${publicationState}`)}</span>
      <Button size="sm" variant="secondary" isDisabled={refreshing||busy} onClick={()=>void reload()}>{t("common.reload")}</Button></div>
    {settings ? <fieldset className="min-w-0 border-0 p-0" disabled={busy||refreshing}><p className="break-all font-mono text-xs">{settings.revision || "—"} · v{settings.version}{settings.effectiveFromMs?` · ${new Date(settings.effectiveFromMs).toLocaleString()}`:null}</p>
    <div className="mt-3 flex flex-wrap gap-2"><Input className="min-w-0 flex-1" aria-label={t("dashboard.userLimit.value.modelId")} placeholder={t("dashboard.userLimit.value.modelId")} value={model}
        aria-invalid={modelError?true:undefined} aria-describedby={modelError?"execution-model-error":undefined}
        onChange={event=>setModel(event.target.value)} onKeyDown={event=>{if(event.key==="Enter"){event.preventDefault();addModel();}}}/>
      <Button variant="secondary" isDisabled={busy || invalidFields.size>0 || !modelKey || !!modelError} onClick={()=>addModel()}>{t("dashboard.userLimit.value.addModel")}</Button>
      <Button variant="secondary" isDisabled={busy || invalidFields.size>0 || !reference || !!modelError} onClick={()=>addModel(true)}>{t("dashboard.userLimit.value.referenceDraft")}</Button>
      <InfoHint label={t("dashboard.userLimit.value.referenceDraft")}>{t("dashboard.userLimit.value.referenceHint")}</InfoHint></div>
    {modelError?<p id="execution-model-error" role="alert" className="mt-1 text-xs text-danger">{modelError}</p>:null}
    {discovered.length?<details className="mt-3 text-xs"><summary className="cursor-pointer">{t("dashboard.userLimit.value.discovered")} · {discovered.length}</summary>
      <div className="mt-2 grid max-h-48 gap-1 overflow-auto">{discovered.map(row=><div key={row.modelKey} className="flex min-w-0 flex-wrap items-center gap-2">
        <Button className="min-w-0 max-w-full" size="sm" variant="ghost" onClick={()=>setModel(row.modelKey)}><span className="min-w-0 truncate font-mono">{row.modelKey}</span></Button>
        {!row.priced?<span className="text-danger">{t("dashboard.userLimit.value.noReference")}</span>:null}
      </div>)}</div></details>:null}
    {parsed ? <div className="mt-3 grid gap-2">{coverage.map(summary=>{
      const price=parsed.models[summary.model];
      if(!price || !price.rates || typeof price.rates!=="object")return null;
      const update=(edit:(p:ExecutionModel)=>void)=>mutate(value=>edit(value.models[summary.model]));
      const problems=new Set(summary.problems);
      return <details className="min-w-0 rounded-md border p-3" key={`${epoch}:${summary.model}`} onToggle={event=>{
        if(event.currentTarget.open)setVisitedModels(previous=>new Set(previous).add(summary.model));
      }}>
        <summary className="cursor-pointer break-all text-sm font-medium">{summary.model} <span className="text-muted-foreground">· {summary.rows.length}{summary.problems.length?<span className="text-warning"> · {t("dashboard.userLimit.value.incomplete")}</span>:null}</span></summary>
        {visitedModels.has(summary.model)?<><div className="mt-3 grid gap-3 sm:grid-cols-2">
          <label className="grid gap-1 text-xs">{t("dashboard.userLimit.value.contextBasis")}<CompactSelect value={price.contextInputBasis} options={[{value:"rawInput",label:t("dashboard.userLimit.value.rawInput")},{value:"freshInput",label:t("dashboard.userLimit.value.freshInput")}]} ariaLabel={t("dashboard.userLimit.value.contextBasis")} onChange={v=>update(p=>{p.contextInputBasis=v as ExecutionModel["contextInputBasis"];})}/></label>
          <ThresholdField label={t("dashboard.userLimit.value.contextThreshold")} error={t("dashboard.userLimit.value.thresholdInvalid")} value={price.longContextThreshold}
            onValidity={ok=>setValidity(JSON.stringify([summary.model,"longContextThreshold"]),ok)} onChange={v=>update(p=>{p.longContextThreshold=v;})}/>
          <Checkbox isSelected={price.longContextInclusive===true} onChange={v=>update(p=>{p.longContextInclusive=v;})}><Checkbox.Control><Checkbox.Indicator/></Checkbox.Control><Checkbox.Content className="text-xs">{t("dashboard.userLimit.value.contextInclusive")}</Checkbox.Content></Checkbox>
        </div>
        <div className="mt-3 overflow-x-auto">
          <table className="w-full min-w-[640px] border-collapse text-xs">
            <caption className="pb-1.5 text-left text-[11px] text-muted-foreground">{t("dashboard.userLimit.value.rateTableCaption")}</caption>
            <thead className="text-left font-mono text-[10px] uppercase tracking-[0.08em] text-muted-foreground"><tr>
              <th scope="col" className="py-1 pr-2 font-medium">{t("dashboard.userLimit.value.tierColumn")}</th>
              {RATE_FIELDS.map(field=><th key={field} scope="col" className="px-1 py-1 font-medium">{t(`dashboard.userLimit.value.rate.${field}`)}</th>)}
              <th className="w-8"><span className="sr-only">{t("common.delete")}</span></th>
            </tr></thead>
            <tbody>{Object.entries(price.rates).map(([row,rate])=>rate && typeof rate==="object"?<tr key={row} className="border-t align-top">
              <th scope="row" className={cn("py-1.5 pr-2 text-left font-mono font-normal", problems.has(row) && "text-warning")}>{row}</th>
              {RATE_FIELDS.map(field=>{
                const key=JSON.stringify([summary.model,row,field]);
                return <td key={key} className="px-1 py-1.5"><RateCell label={`${row} · ${t(`dashboard.userLimit.value.rate.${field}`)}`} value={rate[field]}
                  missing={problems.has(`${row}.${field}`)} missingLabel={t("dashboard.userLimit.value.missing")}
                  onValidity={ok=>setValidity(key,ok)}
                  onChange={v=>update(p=>{p.rates[row][field]=v;p.rates[row].cacheWritePolicy=executionCachePolicy(p.rates[row]);})}/></td>;
              })}
              <td className="py-1.5 pl-1"><Button isIconOnly size="sm" variant="ghost" aria-label={`${t("common.delete")} ${row}`} onClick={()=>{
                update(p=>{delete p.rates[row];});
                setInvalidFields(previous=>new Set([...previous].filter(key=>{const [m,r]=JSON.parse(key);return m!==summary.model || r!==row;})));
              }}><Trash2 className="h-3.5 w-3.5"/></Button></td>
            </tr>:null)}</tbody>
          </table>
        </div>
        {[...problems].some(problem=>TIER_ROWS.includes(problem) && !price.rates[problem])?<p className="mt-2 text-[11px] text-warning">
          {t("dashboard.userLimit.value.missing")}: {[...problems].filter(problem=>TIER_ROWS.includes(problem) && !price.rates[problem]).join(", ")}</p>:null}
        <div className="mt-3 flex flex-wrap gap-2">{TIER_ROWS.filter(row=>!price.rates[row]).map(row=><Button size="sm" variant="secondary" key={row} onClick={()=>update(p=>{p.rates[row]={...newExecutionPriceModel().rates["standard/base"]} as ExecutionRate;})}>+ {row}</Button>)}</div></>:null}
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
    {invalidFields.size?<p role="alert" className="mt-2 text-sm text-danger">{t("dashboard.userLimit.value.invalidRate")}</p>:null}
    <Button className="mt-3" variant="primary" aria-busy={busy} isDisabled={busy || !reviewed || invalidFields.size>0 || !draft || !coverage.length || coverage.some(row=>row.problems.length>0)} onClick={requestPublish}>{t(busy?"dashboard.userLimit.value.publishing":"dashboard.userLimit.value.publish")}</Button>
    {saved?<p role="status" className="mt-2 text-sm">{t("dashboard.userLimit.value.publishedHint")}</p>:null}</fieldset> : null}
    {error ? <p role="alert" className="mt-3 text-sm text-danger">{error}</p> : null}
    <Modal.Backdrop isOpen={!!confirmation} onOpenChange={(open)=>!open && !busy && setConfirmation(null)}>
      <Modal.Container placement="center"><Modal.Dialog className="light w-[min(520px,calc(100vw-2rem))] !bg-white !text-slate-900">
        <Modal.CloseTrigger/>
        <Modal.Header><Modal.Heading>{t(confirmation?.kind==="reload"?"dashboard.userLimit.value.reloadConfirmTitle":"dashboard.userLimit.value.publishConfirmTitle")}</Modal.Heading></Modal.Header>
        <Modal.Body className="grid gap-2 text-sm">
          {confirmation?.kind==="reload"?<p>{t("dashboard.userLimit.value.pricingReloadConfirm")}</p>:<>
            <p className="break-words">{t("dashboard.userLimit.value.publishConfirmModels",{count:listedModels.length,
              models:listedModels.slice(0,8).join(", ")+(listedModels.length>8?` +${listedModels.length-8}`:"")})}</p>
            <p>{t("dashboard.userLimit.value.publishConfirmEffective")}</p>
            <p className="text-muted-foreground">{t("dashboard.userLimit.value.publishConfirmImmutable")}</p>
          </>}
        </Modal.Body>
        <Modal.Footer>
          <Button variant="outline" isDisabled={busy} onClick={()=>setConfirmation(null)}>{t("common.cancel")}</Button>
          <Button variant={confirmation?.kind==="reload"?"danger":"primary"} isDisabled={busy} onClick={()=>{
            if(confirmation?.kind==="publish")void publish(confirmation.snapshot);
            else if(confirmation?.kind==="reload"){applyLoaded(confirmation.data);setConfirmation(null);}
          }}>{t(confirmation?.kind==="reload"?"dashboard.userLimit.value.discard":"dashboard.userLimit.value.publish")}</Button>
        </Modal.Footer>
      </Modal.Dialog></Modal.Container>
    </Modal.Backdrop>
  </details>;
}
