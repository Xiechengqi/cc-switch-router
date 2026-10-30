import assert from "node:assert/strict";
import test from "node:test";
import { inspectExecutionPricing, newExecutionPriceModel } from "./share-value-pricing";
import { valueQuotaMessagesEn, valueQuotaMessagesZh } from "./share-value-quota-messages";

test("tariff drafts never invent free prices and expose full tier/context gaps", () => {
  const draft = newExecutionPriceModel();
  assert.deepEqual(inspectExecutionPricing({models:{"exact-model":draft}})[0].problems.sort(), ["standard/base.input", "standard/base.output"]);
  const priced = {...draft,rates:{"standard/base":{...draft.rates["standard/base"],input:"0",output:"15000000"}}};
  assert.deepEqual(inspectExecutionPricing({models:{"exact-model":priced}})[0].problems, []);
  const price = {...priced,longContextThreshold:200000};
  const gaps = inspectExecutionPricing({models:{model:price}})[0];
  assert.deepEqual(gaps.problems,["standard/long"]);
  assert.equal(inspectExecutionPricing({models:{model:{...draft, rates:{"unknown/base":draft.rates["standard/base"]}}}})[0].problems[0],"unknown/base");
});
test("tariff preview validates canonical rates and cache TTL coverage", () => {
  const base = {input:"3000000",output:"15000000",cacheRead:null,cacheWrite5m:null,cacheWrite1h:null,cacheWritePolicy:"unavailable"};
  const inspect = (rates:unknown) => inspectExecutionPricing({models:{model:{...newExecutionPriceModel(),rates:{"standard/base":rates}}}})[0];
  assert.deepEqual(inspect(base).problems,[]);
  for (const input of [0, "01", "-1", "1e6", "9223372036854775808"]) {
    assert.ok(inspect({...base,input}).problems.includes("standard/base.input"));
  }
  assert.ok(inspect({...base,cacheWritePolicy:"both",cacheWrite5m:"1"}).problems.includes("standard/base.cacheWritePolicy"));
  assert.deepEqual(inspect({...base,cacheWritePolicy:"both",cacheWrite5m:"1",cacheWrite1h:"2"}).problems,[]);
  assert.deepEqual(inspectExecutionPricing(null),[]);
});
test("every new quota message has independent English and Chinese copy", () => {
  assert.deepEqual(Object.keys(valueQuotaMessagesEn).sort(),Object.keys(valueQuotaMessagesZh).sort());
  for (const key of Object.keys(valueQuotaMessagesEn) as Array<keyof typeof valueQuotaMessagesEn>) {
    assert.ok(valueQuotaMessagesEn[key].trim()); assert.ok(valueQuotaMessagesZh[key].trim());
  }
});
