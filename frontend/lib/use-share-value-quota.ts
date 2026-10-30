"use client";
import * as React from "react";
import {readAuthState} from "./auth";
import {getValueQuotaStatus,getMyValueQuotaStatus,mergeValueStatuses,type ValueQuotaStatus} from "./share-value-quota";

const identity = () => { const a=readAuthState(); return `${a.email || ""}:${a.loginEpoch || ""}:${!!a.accessToken}`; };
/** Private responses never survive a login-context change, including another tab. */
export function useShareValueQuota(shareId: string | undefined, manage: boolean, enabled=true) {
  const [actor,setActor]=React.useState(identity);
  const [snapshot,setSnapshot]=React.useState<{actor:string;share:string;items:ValueQuotaStatus[]}>();
  React.useEffect(()=>{
    const change=()=>setActor(identity());
    window.addEventListener("router-auth-changed",change);window.addEventListener("storage",change);
    return ()=>{window.removeEventListener("router-auth-changed",change);window.removeEventListener("storage",change);};
  },[]);
  React.useEffect(()=>{
    setSnapshot(undefined);
    if(!shareId || !enabled || !readAuthState().accessToken)return;
    const controller=new AbortController();
    const accept=(items:ValueQuotaStatus[])=>setSnapshot(previous=>({actor,share:shareId,
      items:mergeValueStatuses(previous?.actor===actor && previous.share===shareId ? previous.items : [],items)}));
    const refresh=()=>{ void (manage?getValueQuotaStatus:getMyValueQuotaStatus)(shareId,controller.signal)
      .then(r=>{if(!controller.signal.aborted && actor===identity())accept(r.items);})
      .catch(()=>{if(!controller.signal.aborted)setSnapshot(undefined);}); };
    const updated=(event:Event)=>{const data=(event as CustomEvent<{shareId:string;items:ValueQuotaStatus[]}>).detail;
      if(data?.shareId===shareId && actor===identity() && !controller.signal.aborted)accept(data.items);};
    window.addEventListener("share-value-quota-updated",updated);
    refresh();const timer=window.setInterval(refresh,15000);
    return ()=>{controller.abort();window.clearInterval(timer);window.removeEventListener("share-value-quota-updated",updated);};
  },[shareId,manage,enabled,actor]);
  return snapshot?.actor===identity() && snapshot.share===shareId ? snapshot.items : [];
}
