import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "@/components/i18n/locale-provider";
import { ClientNextTrialNotice } from "./client-next-trial-notice";
import type { ClientTrialState } from "@/lib/types";

const restored: ClientTrialState = { epoch: 1, allowanceSeconds: 43200, consumedSeconds: 0, remainingSeconds: 43200, lastResetAt: "2026-10-28T00:00:00Z" };
const render = (trial?: ClientTrialState) => renderToStaticMarkup(<LocaleProvider><ClientNextTrialNotice trial={trial} /></LocaleProvider>);

test("next-rental entitlement never masquerades as the current trial timer", () => {
  assert.equal(render(), "");
  assert.equal(render({ ...restored, epoch: 0 }), "");
  assert.equal(render({ ...restored, lastResetAt: null }), "");
  assert.equal(render({ ...restored, allowanceSeconds: NaN }), "");
  const html = render(restored);
  assert.match(html, /Next-rental trial eligibility restored/);
  assert.match(html, /12 hr/);
  assert.match(html, /does not interrupt current paid billing/);
  assert.doesNotMatch(html, /service hours remaining/);
  assert.match(render({ ...restored, allowanceSeconds: 0, remainingSeconds: 0 }), /Host currently has trials disabled/);
});
