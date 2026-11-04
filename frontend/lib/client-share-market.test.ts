import assert from "node:assert/strict";
import test from "node:test";
import { readFileSync } from "node:fs";
import { clientSeatAction, clientSeatGroups, clientShareEntryLabel, clientShareHref, parseClientSeatContext, parseClientOwnerSeatContext, parseClientShareEntries, type ClientShareSeatContext } from "./client-share-market";
import type { ShareMarketSeat } from "./types";

const entry = { shareId: "share", entryKind: "rent", resourceState: "listed", vacantSeats: 1, subdomain: "owner--share" };
test("batched entry projections reject malformed or incomplete snapshots", () => {
  assert.equal(parseClientShareEntries({ items: [entry], nextCursor: null }, ["share"]).items.length, 1);
  for (const value of [{items:[],nextCursor:null}, {items:[entry,entry],nextCursor:null}, {items:[{...entry,entryKind:"buy"}],nextCursor:null}, {items:[{...entry,vacantSeats:-1}],nextCursor:null}, {items:[{...entry,shareId:"other"}],nextCursor:null}]) assert.throws(() => parseClientShareEntries(value,["share"]));
});
test("owner management has its own typed entry and never masquerades as Rent", () => {
  const owner = { ...entry, entryKind: "manage", vacantSeats: 0, resourceState: "unlisted" };
  assert.equal(parseClientShareEntries({ items: [owner], nextCursor: null }, ["share"]).items[0].entryKind, "manage");
  for (const [kind, label] of [["manage", "clientShares.manage"], ["rent", "clientShares.rent"], ["rental", "clientShares.mine"], ["settlement", "clientShares.settlement"]] as const) {
    assert.equal(clientShareEntryLabel(kind), label);
  }
});
test("owner seat context is Share-scoped and validates private subscriptions separately from public seats", () => {
  const seat = { id: "seat", position: 1, status: "occupied", canDelete: false, readOnly: false,
    subscription: { id: "sub", shareId: "share", listingId: "listing", seatId: "seat" } };
  const listing = { id: "listing", shareId: "share", isOwner: true, ownerEmail: "owner@example.test", seats: [seat] };
  const context = { shareId: "share", listings: [listing] };
  assert.equal(parseClientOwnerSeatContext(context, "share").listings[0].seats[0].subscription?.id, "sub");
  for (const value of [
    { ...context, shareId: "other" }, { ...context, listings: [] }, { ...context, listings: [listing, listing] },
    { ...context, listings: [{ ...listing, shareId: "other" }] }, { ...context, listings: [{ ...listing, isOwner: false }] },
    { ...context, listings: [{ ...listing, seats: [seat, seat] }] },
    ...["shareId", "listingId", "seatId"].map(key => ({ ...context, listings: [{ ...listing, seats: [{ ...seat, subscription: { ...seat.subscription, [key]: "other" } }] }] })),
  ]) assert.throws(() => parseClientOwnerSeatContext(value, "share"));
  assert.throws(() => parseClientSeatContext({ shareId: "share", resourceState: "listed", nextCursor: null, subscriptions: [], listing }, "share"), "owner data must not enter the public seat projection");
});
test("private seat context cannot contain another renter's subscription", () => {
  const context = { shareId:"share",resourceState:"listed",nextCursor:null,subscriptions:[], listing:{id:"listing",shareId:"share",seats:[{id:"seat",position:1,status:"occupied",canRent:false}]} };
  assert.equal(parseClientSeatContext(context,"share").listing?.seats.length,1);
  assert.throws(() => parseClientSeatContext({...context,shareId:"other"},"share"));
  assert.throws(() => parseClientSeatContext({...context,listing:{...context.listing,seats:[{...context.listing.seats[0],subscription:{id:"private"}}]}},"share"));
  assert.throws(() => parseClientSeatContext({...context,subscriptions:[{id:"private",shareId:"other",seatId:"s",status:"active",canRelease:true,canTerminateImmediately:true}]},"share"));
});
test("own live seats precede empty and occupied seats; old settlements never claim new tenants' seats", () => {
  const context = { subscriptions:[{id:"mine",shareId:"s",seatId:"s2",seatPosition:2,status:"active"},{id:"old",shareId:"s",seatId:"s3",seatPosition:3,status:"released"}],listing:{seats:[{id:"s4",position:4,status:"available"},{id:"s3",position:3,status:"occupied"},{id:"s2",position:2,status:"occupied"},{id:"s1",position:1,status:"available"}]} } as ClientShareSeatContext;
  const groups=clientSeatGroups(context);
  assert.deepEqual(groups.mine.map(s=>s.id),["mine"]);
  assert.deepEqual(groups.available.map(s=>s.id),["s1","s4"]);
  assert.deepEqual(groups.occupied.map(s=>s.id),["s3"]);
  assert.deepEqual(groups.history.map(s=>s.id),["old"]);
  assert.equal(new URL(clientShareHref("share", "seat", "sub"),"https://router.test").pathname,"/");
});
test("seat actions distinguish occupancy and gate approval on the same service prerequisites as the market", () => {
  const seat = { status: "available", readOnly: false, canRent: false, rentPrerequisitesMet: true,
    eligibility: { allowed: false, status: "access_required" } } as ShareMarketSeat;
  assert.equal(clientSeatAction(seat, false), "login");
  assert.equal(clientSeatAction(seat, true), "approval");
  assert.equal(clientSeatAction({ ...seat, rentPrerequisitesMet: false }, true), "unavailable");
  assert.equal(clientSeatAction({ ...seat, canRent: true }, true), "rent");
  for (const authenticated of [false, true]) {
    assert.equal(clientSeatAction({ ...seat, status: "occupied" }, authenticated), "occupied");
    assert.equal(clientSeatAction({ ...seat, readOnly: true }, authenticated), "unavailable");
  }
});
test("homepage integration batches cards, preserves missing-resource contexts and uses shared financial dialogs", () => {
  const source=(file:string)=>readFileSync(new URL(file,import.meta.url),"utf8");
  const provider=source("../components/dashboard/client-share-market.tsx");
  assert.match(provider,/useShareSeatCheckout\(/);
  assert.match(provider,/useShareMarketRentalActions\(/);
  assert.match(provider,/ShareRentalDetail subscription=\{sub\}/);
  assert.match(provider,/requested.slice\(i \* 100, \(i \+ 1\) \* 100\)/);
  assert.match(provider,/document.visibilityState !== "hidden"/);
  assert.match(provider,/key=\{`\$\{userId\}:\$\{actor\}`\}/);
  assert.match(provider,/!ownLive && !detailError && \["login", "rent", "approval"\]/);
  assert.match(provider,/event.stopPropagation\(\)/);
  assert.match(source("../components/dashboard/client-board.tsx"),/ClientShareRecoveryCards knownShareIds=\{mineShares.map/);
  assert.match(source("../components/dashboard/share-card.tsx"),/ClientShareMarketEntryButton shareId=\{share.shareId\}/);
});
test("both management entrances reuse the same manager with cancellation and no implicit writes", () => {
  const source = (file: string) => readFileSync(new URL(file, import.meta.url), "utf8");
  const provider = source("../components/dashboard/client-share-market.tsx");
  const owner = source("../components/dashboard/client-share-owner-management.tsx");
  const workspace = source("../components/dashboard/share-market/owner-workspace.tsx");
  assert.match(provider, /entries.get\(selection.shareId\)\?\.entryKind === "manage"/);
  assert.match(provider, /!shareId \|\| ownerMode \|\| waitingForEntry/);
  assert.match(provider, /key=\{`\$\{actor\}:\$\{selection.shareId\}`\}/);
  assert.match(owner, /getClientOwnerSeatContext\(shareId, controller.signal\)/);
  assert.match(owner, /request.current\?\.abort\(\)/);
  assert.match(owner, /actor === valueReadinessActor\(\)/);
  assert.match(owner, /setContext\(null\)/);
  assert.match(owner, /<ShareMarketOwnerSeatManager/);
  assert.match(workspace, /<ShareMarketOwnerSeatManager key=\{actor\}/);
  assert.match(workspace, /ownedSeatSections\(selected\)/);
  assert.match(workspace, /<details key=\{selected.id\}/);
  assert.match(owner, /<CompactSelect/);
  assert.doesNotMatch(owner, /localStorage|sessionStorage|createShareMarketListing|addShareMarketSeat|updateShareMarketSeat/);
});
