"use client";
import * as React from "react";
import {readAuthState} from "./auth";
import {getValueQuotaStatus,getMyValueQuotaStatus,mergeValueStatuses,upsertValueStatuses,type ValueQuotaStatus} from "./share-value-quota";

const identity = () => { const a=readAuthState(); return `${a.email || ""}:${a.loginEpoch || ""}:${!!a.accessToken}`; };
type Snapshot = {actor:string;share:string;items:ValueQuotaStatus[];stale:boolean};
/**
 * Private responses never survive a login-context change, including another
 * tab. A failed poll keeps the last good rows and marks them stale instead of
 * wiping what the owner was reading.
 */
export function useShareValueQuotaSnapshot(shareId: string | undefined, manage: boolean, enabled=true) {
  const [actor,setActor]=React.useState(identity);
  const [snapshot,setSnapshot]=React.useState<Snapshot>();
  React.useEffect(()=>{
    const change=()=>setActor(identity());
    window.addEventListener("router-auth-changed",change);window.addEventListener("storage",change);
    return ()=>{window.removeEventListener("router-auth-changed",change);window.removeEventListener("storage",change);};
  },[]);
  React.useEffect(()=>{
    if(!shareId || !enabled || !readAuthState().accessToken)return;
    const controller=new AbortController();
    const sameScope=(previous?:Snapshot)=>previous?.actor===actor && previous.share===shareId;
    // A poll is the whole list; a published mutation result covers only its users.
    const accept=(items:ValueQuotaStatus[],partial=false)=>setSnapshot(previous=>{
      const before=sameScope(previous) ? previous!.items : [];
      return {actor,share:shareId,stale:partial ? previous?.stale ?? false : false,
        items:partial ? upsertValueStatuses(before,items) : mergeValueStatuses(before,items)};
    });
    const refresh=()=>{ void (manage?getValueQuotaStatus:getMyValueQuotaStatus)(shareId,controller.signal)
      .then(r=>{if(!controller.signal.aborted && actor===identity())accept(r.items);})
      .catch(()=>{if(!controller.signal.aborted)setSnapshot(previous=>sameScope(previous)?{...previous!,stale:true}:previous);}); };
    const updated=(event:Event)=>{const data=(event as CustomEvent<{shareId:string;items:ValueQuotaStatus[]}>).detail;
      if(data?.shareId===shareId && actor===identity() && !controller.signal.aborted)accept(data.items,true);};
    window.addEventListener("share-value-quota-updated",updated);
    refresh();const timer=window.setInterval(refresh,15000);
    return ()=>{controller.abort();window.clearInterval(timer);window.removeEventListener("share-value-quota-updated",updated);};
  },[shareId,manage,enabled,actor]);
  const current=snapshot?.actor===identity() && snapshot.share===shareId ? snapshot : undefined;
  return {items:current?.items ?? [],stale:current?.stale ?? false};
}
export function useShareValueQuota(shareId: string | undefined, manage: boolean, enabled=true) {
  return useShareValueQuotaSnapshot(shareId,manage,enabled).items;
}
/** Push fresh rows from a mutation to every mounted reader of this Share. */
export function publishValueQuotaStatuses(shareId: string | undefined, items: ValueQuotaStatus[]) {
  if(shareId)window.dispatchEvent(new CustomEvent("share-value-quota-updated",{detail:{shareId,items}}));
}
