import assert from "node:assert/strict";
import test from "node:test";
import { forgetShareLocalTestConsent, hasShareLocalTestConsent, rememberShareLocalTestConsent } from "./share-local-test-consent";
import { SHARE_LOCAL_TEST_ORIGIN } from "./share-local-test";

function storage() {
  const records = new Map<string, string>();
  return {
    records,
    getItem: (key: string) => records.get(key) ?? null,
    setItem: (key: string, value: string) => { records.set(key, value); },
    removeItem: (key: string) => { records.delete(key); },
  };
}

test("local-test trust is opt-in and isolated by account, target origin and notice version", () => {
  const store = storage();
  assert.equal(hasShareLocalTestConsent("account-one", store), false);
  assert.equal(rememberShareLocalTestConsent("account-one", store), true);
  assert.equal(hasShareLocalTestConsent("account-one", store), true);
  assert.equal(hasShareLocalTestConsent("account-two", store), false);
  assert.equal(hasShareLocalTestConsent("account-one", store, "https://different.example"), false);
  assert.deepEqual([...store.records.values()], ["accepted"]);
  const [key] = store.records.keys();
  assert.ok(key.includes(SHARE_LOCAL_TEST_ORIGIN));
  store.records.set(key.replace("[1,", "[0,"), "accepted");
  store.records.delete(key);
  assert.equal(hasShareLocalTestConsent("account-one", store), false);
});

test("trust can be revoked without affecting other accounts or browser data", () => {
  const store = storage();
  store.setItem("unrelated-setting", "preserve");
  rememberShareLocalTestConsent("account-one", store);
  rememberShareLocalTestConsent("account-two", store);
  forgetShareLocalTestConsent("account-one", store);
  assert.equal(hasShareLocalTestConsent("account-one", store), false);
  assert.equal(hasShareLocalTestConsent("account-two", store), true);
  assert.equal(store.getItem("unrelated-setting"), "preserve");
});

test("anonymous, malformed and unavailable storage always require explicit confirmation", () => {
  const store = storage();
  assert.equal(rememberShareLocalTestConsent("", store), false);
  assert.equal(hasShareLocalTestConsent("", store), false);
  assert.equal(store.records.size, 0);
  rememberShareLocalTestConsent("account-one", store);
  const [key] = store.records.keys();
  for (const invalid of ["true", "1", "{}", '"accepted"', ""]) {
    store.records.set(key, invalid);
    assert.equal(hasShareLocalTestConsent("account-one", store), false);
  }
  const unavailable = {
    getItem: () => { throw new Error("Storage denied"); },
    setItem: () => { throw new Error("Storage denied"); },
    removeItem: () => { throw new Error("Storage denied"); },
  };
  assert.equal(hasShareLocalTestConsent("account-one", unavailable), false);
  assert.equal(rememberShareLocalTestConsent("account-one", unavailable), false);
  assert.doesNotThrow(() => forgetShareLocalTestConsent("account-one", unavailable));
  assert.equal(hasShareLocalTestConsent("account-one"), false, "SSR has no ambient browser storage");
});
