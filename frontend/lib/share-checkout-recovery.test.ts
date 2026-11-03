import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { clearShareCheckoutRecovery, parseShareCheckoutRecovery, readShareCheckoutRecovery, saveShareCheckoutRecovery } from "./share-checkout-recovery";
import { isUnknownShareCheckoutError } from "./share-checkout";
import { ApiError } from "./api";

const draft = { shareId:"share",seatId:"seat",autoRenew:true };
const pending = {...draft,quoteId:"quote",idempotencyKey:"original-request"};
test("recovery stores only a login-scoped draft and the exact original purchase identifiers", () => {
  assert.deepEqual(parseShareCheckoutRecovery({scope:"user:epoch",draft,pending,accessToken:"secret"},"user:epoch"),{scope:"user:epoch",draft,pending});
  assert.equal(parseShareCheckoutRecovery({scope:"user:epoch",draft,pending},"other:epoch"),null);
  assert.equal(parseShareCheckoutRecovery({scope:"user:epoch",draft,pending},"user:new-login"),null);
  assert.equal(parseShareCheckoutRecovery({scope:"user:epoch",draft,pending:{...pending,autoRenew:false}},"user:epoch"),null);
  assert.equal(parseShareCheckoutRecovery({scope:"user:epoch",draft,pending:{...pending,quoteId:""}},"user:epoch"),null);
});
test("network, timeout and server failures are unknown, but explicit business rejections are not", () => {
  assert.equal(isUnknownShareCheckoutError(new Error("network")),true);
  for(const status of [0,408,500,502,503]) assert.equal(isUnknownShareCheckoutError(new ApiError(status,"uncertain")),true);
  for(const status of [400,401,403,409,410,422]) assert.equal(isUnknownShareCheckoutError(new ApiError(status,"rejected")),false);
});
test("both market surfaces share frozen purchase retries without automatic purchase on return", () => {
  const code=readFileSync(new URL("../components/dashboard/share-market/share-seat-checkout.tsx",import.meta.url),"utf8");
  assert.match(code,/pending.current \?\?/);
  assert.match(code,/rentShareMarketSeat\(request.seatId, request.quoteId, request.idempotencyKey, request.autoRenew\)/);
  assert.match(code,/saveShareCheckoutRecovery\(scope, request, request\)/);
  assert.match(code,/Promise.resolve\(\).then\(onChanged\)/);
  assert.match(code,/if \(recovery\?\.pending\)/);
  assert.match(code,/if \(flight.current \|\| pending.current\) return/);
});

test("session-only storage preserves the original request across reload and isolates a new login", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "sessionStorage");
  const data = new Map<string,string>();
  Object.defineProperty(globalThis,"sessionStorage",{configurable:true,value:{getItem:(key:string)=>data.get(key)??null,setItem:(key:string,value:string)=>data.set(key,value),removeItem:(key:string)=>data.delete(key)}});
  try {
    assert.equal(saveShareCheckoutRecovery("user:epoch",draft,pending),true);
    assert.deepEqual(readShareCheckoutRecovery("user:epoch")?.pending,pending);
    assert.equal(readShareCheckoutRecovery("user:new-login"),null);
    clearShareCheckoutRecovery("other:epoch");
    assert.deepEqual(readShareCheckoutRecovery("user:epoch")?.pending,pending);
    clearShareCheckoutRecovery("user:epoch");
    assert.equal(readShareCheckoutRecovery("user:epoch"),null);
    Object.defineProperty(globalThis,"sessionStorage",{configurable:true,value:{setItem:()=>{throw new Error("blocked");}}});
    assert.equal(saveShareCheckoutRecovery("user:epoch",draft,pending),false);
  } finally {
    if(previous) Object.defineProperty(globalThis,"sessionStorage",previous);
    else Reflect.deleteProperty(globalThis,"sessionStorage");
  }
});
