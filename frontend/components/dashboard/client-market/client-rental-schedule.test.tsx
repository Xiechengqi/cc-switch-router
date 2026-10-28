import assert from "node:assert/strict";
import test from "node:test";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleProvider } from "@/components/i18n/locale-provider";
import { ClientRentalSchedule } from "./client-rental-schedule";
import type { ClientMarketRental } from "@/lib/types";

const rental: ClientMarketRental = {
  installationId: "client", hostId: "host", providerId: "provider", hostOwnerEmail: "provider@example.com",
  clientOwnerEmail: "buyer@example.com", status: "active", pricingModel: "prepaid_calendar_month",
  cyclePriceMinor: 5, offerRevision: 1, isClientOwner: true, canRelease: true, canFinalizeRelease: false,
  hostCleanupPending: false, providerTerminalAccessActive: false, canManageProviderTerminal: false,
  updatedAt: "2026-10-28T00:00:00Z", walletContract: { id: "contract", status: "active",
    trialSecondsRemaining: 0, renewalPolicy: "manual", periodEnd: "2026-11-28T06:02:00Z" },
};
const render = (value: ClientMarketRental, summaryOnly = false) => renderToStaticMarkup(<LocaleProvider>
  <ClientRentalSchedule contract={value.walletContract!} summaryOnly={summaryOnly} compact={summaryOnly} showManageLink={false} />
</LocaleProvider>);

test("normal rental dates and wallet navigation do not occupy the compact status cell", () => {
  assert.equal(render(rental, true), "");
  const detail = render(rental);
  assert.match(detail, /Ends on/);
  assert.doesNotMatch(detail, /href=|Platform wallet/);
  assert.match(render({ ...rental, walletContract: { ...rental.walletContract!, renewalPolicy: "automatic" } }), /Renews on/);
});

test("trial and wallet exception states remain visible in the status cell", () => {
  for (const status of ["trialing", "suspended", "review_required", "pending_activation", "ending"]) {
    const html = render({ ...rental, walletContract: { ...rental.walletContract!, status, trialSecondsRemaining: 3600 } }, true);
    assert.ok(html.length > 0, status);
    assert.doesNotMatch(html, /href=|Platform wallet/);
  }
});

test("expanded rental details retain the restored next-rental trial allowance", () => {
  const value = { ...rental, walletContract: { ...rental.walletContract!, nextRentalTrial: { epoch: 1,
    allowanceSeconds: 7200, consumedSeconds: 0, remainingSeconds: 7200, lastResetAt: "2026-10-28T00:00:00Z" } } };
  assert.match(render(value), /data-next-rental-trial/);
  assert.equal(render(value, true), "");
});
