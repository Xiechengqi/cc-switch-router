import assert from "node:assert/strict";
import test from "node:test";
import { messages } from "./i18n";
import { notificationFeed, notificationHref, notificationKindKey, type UserNotification } from "./user-notifications";

const item: UserNotification = { id: "market:1", category: "wallet", eventKind: "topup_credited",
  objectKind: "topup", objectId: "one", payload: {}, createdAt: "2026-10-28T00:00:00Z", readAt: null };

test("notification center validates the envelope and isolates malformed events", () => {
  const page = notificationFeed({ items: [item, { ...item, createdAt: "bad" }, { ...item, payload: null },
    { ...item, category: "unknown" }, null], unread: 1, nextCursor: null, readThrough: item.id });
  assert.deepEqual(page.items, [item]);
  assert.equal(page.readThrough, item.id);
  for (const value of [null, {}, { items: {}, unread: 0 }, { items: [], unread: -1 }, { items: [], unread: "2" }]) {
    assert.throws(() => notificationFeed(value), /Invalid notification feed/);
  }
});

test("notification center labels cover every category and known new kind in both locales", () => {
  for (const category of ["wallet", "service", "client", "system"] as const) {
    for (const locale of ["en", "zh-CN"] as const) assert.ok(messages[locale][`notificationCenter.category.${category}`]);
  }
  // Owner alerts render their localized title from the payload, so the kind
  // label may fall back to the generic account update.
  const labeled = ["topup_credited", "contract_suspended", "client_offline", "client_recovered", "client_registered", "client_removed", "client_termination_started", "client_termination_decision_required", "share_termination_updated"];
  for (const kind of labeled) {
    const key = notificationKindKey(kind);
    assert.notEqual(key, "notificationCenter.other");
    assert.ok(messages.en[key]);
    assert.ok(messages["zh-CN"][key]);
  }
  for (const kind of ["operator_alert_firing", "operator_alert_resolved"]) {
    const key = notificationKindKey(kind);
    assert.ok(messages.en[key]);
    assert.ok(messages["zh-CN"][key]);
  }
  assert.equal(notificationKindKey("future_kind"), "notificationCenter.other");
});

test("notification links use existing local routes and cannot follow payload URLs", () => {
  for (const eventKind of ["client_workspace_ready", "client_workspace_access", "client_workspace_management"]) {
    assert.equal(notificationHref({ ...item, eventKind, objectKind: "client", objectId: "lease/1" }), "/account/rentals/?market=client&focus=lease%2F1");
  }
  for (const eventKind of ["client_lease_reclaim_scheduled", "client_lease_reclaim_cancelled", "client_lease_reclaimed", "client_egress_changed"]) {
    assert.equal(notificationHref({ ...item, category: "client", eventKind, objectKind: "client_lease", objectId: "lease/1" }), "/account/rentals/?market=client&focus=lease%2F1");
    assert.notEqual(notificationKindKey(eventKind), "notificationCenter.other");
  }
  assert.equal(notificationHref({ ...item, payload: { href: "javascript:alert(1)" } }), "/account/wallet/");
  assert.equal(notificationHref({ ...item, eventKind: "c2c_order_updated", payload: { orderId: "one/two?x=1" } }), "/account/wallet/?c2cOrder=one%2Ftwo%3Fx%3D1");
  const client = new URL(notificationHref({ ...item, category: "client", objectId: "one&two" }), "https://router.test");
  assert.equal(client.pathname, "/");
  assert.equal(client.searchParams.get("focusId"), "one&two");
  assert.equal(client.searchParams.get("drawerId"), "one&two");
  assert.equal(notificationHref({ ...item, eventKind: "client_termination_claim" }), "/client-market/?terminationId=one");
  assert.equal(notificationHref({ ...item, eventKind: "share_termination_updated", payload: { terminationId: "old/id?x=1", href: "https://evil.test" } }), "/share-market/?terminationId=old%2Fid%3Fx%3D1");
  assert.equal(notificationHref({ ...item, objectKind: "share_termination", objectId: "original", payload: { terminationId: 42 } }), "/share-market/?terminationId=original");
  assert.equal(notificationHref({ ...item, category: "system", objectKind: "client_termination", objectId: "old/id?x=1", payload: { href: "https://evil.test" } }), "/client-market/?terminationId=old%2Fid%3Fx%3D1");
  assert.equal(notificationHref({ ...item, category: "system" }), "/metrics/?tab=alerts");
});
