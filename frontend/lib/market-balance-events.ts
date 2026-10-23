// Account-scoped invalidation only; never store financial balances in the browser.
const CHANGE_EVENT = "router-market-balance-changed";
const CHANGE_KEY = "router_market_balance_revision_v1";
/** No amounts or credentials in the cross-tab invalidation signal. */
export function notifyMarketBalanceChanged(actor: string) {
  window.dispatchEvent(new CustomEvent(CHANGE_EVENT, { detail: actor }));
  try { localStorage.setItem(CHANGE_KEY, JSON.stringify({ actor, nonce: crypto.randomUUID() })); } catch { /* Same-tab refresh still works. */ }
}
export function subscribeMarketBalanceChanged(actor: string, refresh: () => void) {
  const onLocal = (event: Event) => { if ((event as CustomEvent).detail === actor) refresh(); };
  const onStorage = (event: StorageEvent) => {
    if (event.key !== CHANGE_KEY || !event.newValue) return;
    try { if (JSON.parse(event.newValue).actor === actor) refresh(); } catch { /* Ignore malformed browser storage. */ }
  };
  window.addEventListener(CHANGE_EVENT, onLocal);
  window.addEventListener("storage", onStorage);
  return () => { window.removeEventListener(CHANGE_EVENT, onLocal); window.removeEventListener("storage", onStorage); };
}
