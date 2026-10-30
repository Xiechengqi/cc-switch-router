"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { authFetch } from "@/lib/auth";
import { parseJson } from "@/lib/api";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {inspectExecutionPricing,newExecutionPriceModel} from "@/lib/share-value-pricing";

type Settings = {version:number;revision:string|null;snapshot:unknown;effectiveFromMs:number|null};
export function ShareValuePricingPanel() {
  const {t}=useLocaleText();
  const [settings,setSettings]=React.useState<Settings>();
  const [draft,setDraft]=React.useState("");
  const [error,setError]=React.useState("");
  const [busy,setBusy]=React.useState(false);
  const [model,setModel]=React.useState("");
  const [health,setHealth]=React.useState<Record<string,number>>();
  const coverage=React.useMemo(()=>{try{return inspectExecutionPricing(JSON.parse(draft));}catch{return [];}},[draft]);
  const addModel=()=>{
    try {const value=JSON.parse(draft);const key=model.trim();if(!key || key.length>250 || value.models?.[key])return;
      value.models={...value.models,[key]:newExecutionPriceModel()};setDraft(JSON.stringify(value,null,2));setModel("");
    }catch(cause){setError(String(cause));}
  };
  React.useEffect(()=>{
    const controller=new AbortController();
    const loadHealth=()=>void authFetch("/v1/admin/share-value-pricing/health",{signal:controller.signal}).then(parseJson<Record<string,number>>).then(data=>{if(!controller.signal.aborted)setHealth(data);}).catch(()=>{if(!controller.signal.aborted)setHealth(undefined);});
    loadHealth();const timer=window.setInterval(loadHealth,30000);
    void authFetch("/v1/admin/share-value-pricing",{signal:controller.signal}).then(parseJson<Settings>).then((data)=>{
      if(controller.signal.aborted)return;
      setSettings(data);
      setDraft(JSON.stringify(data.snapshot ?? {schemaVersion:1,tariffPolicyId:"api-equivalent-v1",currency:"USD",models:{},exactAliases:{}},null,2));
    }).catch((cause)=>{if(!controller.signal.aborted)setError(String(cause));});
    return ()=>{controller.abort();window.clearInterval(timer);};
  },[]);
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
      setSettings(data);setDraft(JSON.stringify(data.snapshot,null,2));
    }catch(cause){setError(String(cause));}finally{setBusy(false);}
  };
  return <details className="rounded-lg border bg-background p-4">
    <summary className="cursor-pointer font-medium">{t("dashboard.userLimit.value.pricing")}</summary>
    <p className="my-3 text-sm text-muted-foreground">{t("dashboard.userLimit.value.pricingHint")}</p>
    {health ? <div className="my-3 flex flex-wrap gap-x-5 gap-y-2 text-xs">{(["activeUsdGrants","scheduledGrants","shadowGrants","staleGrants","acknowledgedInstallations"] as const).map(key=><span key={key}>{t(`dashboard.userLimit.value.health.${key}`)}: <strong>{health[key]}</strong></span>)}</div>:null}
    {settings ? <><p className="break-all font-mono text-xs">{settings.revision || "—"} · v{settings.version}</p>
    <label className="mt-3 grid gap-2 text-xs">{t("dashboard.userLimit.value.json")}
      <textarea className="h-64 w-full resize-y rounded-md border bg-background p-3 font-mono" value={draft} onChange={(event)=>setDraft(event.target.value)} spellCheck={false} />
    </label>
    <div className="mt-3 flex flex-wrap gap-2"><input className="min-w-0 flex-1 rounded-md border bg-background px-3 py-2 text-sm" aria-label={t("dashboard.userLimit.value.modelId")} placeholder={t("dashboard.userLimit.value.modelId")} value={model} onChange={event=>setModel(event.target.value)} maxLength={250}/>
      <Button variant="secondary" isDisabled={busy || !model.trim()} onClick={addModel}>{t("dashboard.userLimit.value.addModel")}</Button></div>
    <p className="mt-2 text-xs text-muted-foreground">{t("dashboard.userLimit.value.matrixHint")}</p>
    {coverage.length ? <details className="mt-3 text-xs"><summary>{t("dashboard.userLimit.value.coverage")} · {coverage.length}</summary>
      <div className="mt-2 max-h-64 overflow-auto">{coverage.slice(0,100).map(row=><div className="border-t py-2" key={row.model}>
        <span className="break-all font-mono">{row.model}</span> · {row.rows.join(", ") || "—"}
        {row.problems.length ? <p className="text-danger">{t("dashboard.userLimit.value.incomplete")}: {row.problems.join(", ")}</p> : null}
      </div>)}</div></details>:null}
    <Button className="mt-3" variant="primary" isDisabled={busy || !draft || !coverage.length || coverage.some(row=>row.problems.length>0)} onClick={()=>void publish()}>{t("dashboard.userLimit.value.publish")}</Button></> : null}
    {error ? <p role="alert" className="mt-3 text-sm text-danger">{error}</p> : null}
  </details>;
}
