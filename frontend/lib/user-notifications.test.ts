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
  for (const kind of ["topup_credited", "contract_suspended", "client_offline", "client_recovered", "client_registered", "client_removed", "operator_alert_firing", "operator_alert_resolved"]) {
    const key = notificationKindKey(kind);
    assert.notEqual(key, "notificationCenter.other");
    assert.ok(messages.en[key]);
    assert.ok(messages["zh-CN"][key]);
  }
  assert.equal(notificationKindKey("future_kind"), "notificationCenter.other");
});

test("notification links use existing local routes and cannot follow payload URLs", () => {
  assert.equal(notificationHref({ ...item, payload: { href: "javascript:alert(1)" } }), "/account/wallet/");
  assert.equal(notificationHref({ ...item, eventKind: "c2c_order_updated", payload: { orderId: "one/two?x=1" } }), "/account/wallet/?c2cOrder=one%2Ftwo%3Fx%3D1");
  const client = new URL(notificationHref({ ...item, category: "client", objectId: "one&two" }), "https://router.test");
  assert.equal(client.pathname, "/clients/");
  assert.equal(client.searchParams.get("focusId"), "one&two");
  assert.equal(client.searchParams.get("drawerId"), "one&two");
  assert.equal(notificationHref({ ...item, eventKind: "client_termination_claim" }), "/account/client/");
  assert.equal(notificationHref({ ...item, category: "system" }), "/metrics/?tab=alerts");
});
