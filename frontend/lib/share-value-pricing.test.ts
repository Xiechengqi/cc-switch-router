import assert from "node:assert/strict";
import test from "node:test";
import { inspectExecutionPricing, newExecutionPriceModel,executionRateMicros,executionRateInput,executionReferenceDraft,executionCachePolicy } from "./share-value-pricing";
import { valueQuotaMessagesEn, valueQuotaMessagesZh } from "./share-value-quota-messages";

test("form rates preserve exact micro-USD, distinguish blank from zero and never infer cache prices from reference",()=>{
  for(const [input,micros] of [["0","0"],["0.000001","1"],["1.234567","1234567"],["9223372036854.775807","9223372036854775807"]]) {
    assert.equal(executionRateMicros(input),micros);
    assert.equal(executionRateInput(micros),input);
  }
  for(const invalid of ["","01","-1","1.",".1","1e6","1.0000001","9223372036854.775808"])assert.equal(executionRateMicros(invalid),undefined);
  assert.equal(executionRateInput(null),"");
  const draft=executionReferenceDraft({input:3_000_000,output:15_000_000});
  assert.equal(draft.rates["standard/base"].input,"3000000");
  assert.equal(draft.rates["standard/base"].cacheRead,null);
  assert.equal(draft.rates["standard/base"].cacheWrite1h,null);
  assert.deepEqual(Object.keys(draft.rates),["standard/base"]);
  assert.throws(()=>executionReferenceDraft({input:1.5,output:1}));
  assert.equal(executionCachePolicy({...draft.rates["standard/base"],cacheWrite5m:"0"}),"fiveMinutesOnly");
});

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
