import assert from "node:assert/strict";
import test from "node:test";
import { parseValueTariffHealth, valueTariffState, type ValueTariffHealth } from "./value-tariff-delivery";

const revision = `sha256:${"a".repeat(64)}`;
const health = (ack = 0, active = 0): ValueTariffHealth => ({
  publishedRevision: revision, effectiveFromMs: 100, serverTimeMs: 100,
  publication: {state: "published"},
  delivery: {expectedInstallations: 2, acknowledgedInstallations: ack, activatedInstallations: active, unconfirmedInstallations: 2-active},
});

test("publication, effective time, ACK and activation remain distinct", () => {
  assert.equal(valueTariffState({...health(), publication: {state: "pending"}}), "pendingPublication");
  assert.equal(valueTariffState({publishedRevision: null, effectiveFromMs: null}), "missing");
  assert.equal(valueTariffState({...health(2), serverTimeMs: 99}), "pendingEffective");
  assert.equal(valueTariffState(health()), "pendingAck");
  assert.equal(valueTariffState(health(2)), "unconfirmed", "even every ACK does not prove activation");
  assert.equal(valueTariffState(health(2, 1)), "partial");
  assert.equal(valueTariffState(health(2, 2)), "live");
  assert.equal(valueTariffState({...health(), delivery: {expectedInstallations: 0, acknowledgedInstallations: 0, activatedInstallations: 0, unconfirmedInstallations: 0}}), "noServers");
});

test("legacy health and absence of Router time never infer live from the browser clock", () => {
  const previous = Date.now;
  try {
    for (const clock of [0, 9_000_000_000_000]) {
      Date.now = () => clock;
      assert.equal(valueTariffState({...health(2,2), serverTimeMs: 99}), "pendingEffective");
      assert.equal(valueTariffState({...health(2,2), serverTimeMs: undefined}), "unconfirmed");
      assert.equal(valueTariffState(parseValueTariffHealth({publishedRevision: revision, effectiveFromMs: 100, acknowledgedInstallations: 100})), "unconfirmed");
    }
  } finally { Date.now = previous; }
});

test("strict parsing rejects malformed clocks, revisions and impossible proof counts", () => {
  assert.deepEqual(parseValueTariffHealth(health(2,2)), health(2,2));
  for (const invalid of [null, [], {}, {...health(), publishedRevision: "not-a-revision"}, {...health(), effectiveFromMs: null},
    {...health(), effectiveFromMs: -1}, {...health(), serverTimeMs: "100"}, {...health(), serverTimeMs: Infinity},
    {...health(), publication: {state: "live"}}, {...health(), delivery: null},
    {...health(), delivery: {...health().delivery, activatedInstallations: 3}},
    {...health(), delivery: {...health().delivery, acknowledgedInstallations: 3}},
    {...health(), delivery: {...health().delivery, unconfirmedInstallations: 1}},
    {...health(), delivery: {...health().delivery, activatedInstallations: -1}},
    {...health(), delivery: {...health().delivery, expectedInstallations: 1.5}},
  ]) {
    // Null delivery is an older Router response: safe, but never confirmed.
    if (invalid && typeof invalid === "object" && "delivery" in invalid && invalid.delivery === null) {
      assert.equal(valueTariffState(parseValueTariffHealth(invalid)), "unconfirmed");
    } else assert.throws(() => parseValueTariffHealth(invalid));
  }
  assert.equal(valueTariffState(parseValueTariffHealth(health(2,0))), "unconfirmed", "stale receipts already excluded by the Router cannot be revived by ACK counts");
});
