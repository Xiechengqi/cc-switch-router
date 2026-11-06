import assert from "node:assert/strict";
import test from "node:test";
import { ModelRoutingSession, routingDraftIsDirty } from "./model-routing-session";
import type { UserModelRoutingResponse, UserApiTokenResetResponse } from "./types";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

function profile(revision = 1): UserModelRoutingResponse {
  return {
    enabled: true, revision, apiBaseUrl: "https://api.example.test",
    routes: [{ id: "route", appType: "codex", requestedModel: "*", targetShareId: "share", candidates: [], createdAt: "", updatedAt: "" }],
    eligibleShares: [{ shareId: "share", shareName: "Private Share", subdomain: "private-sub", apps: ["codex"], access: "owner", directApiUrl: "", freeAccess: false, isOnline: true }],
  };
}

function fixture() {
  const calls = { routing: 0, token: 0, replace: 0, reset: 0 };
  const token = { prefix: "fixture", scopes: ["shares:use"], createdAt: "" };
  const services: ConstructorParameters<typeof ModelRoutingSession>[0] = {
    getRouting: async () => { calls.routing++; return profile(); },
    getToken: async () => { calls.token++; return { token, apiToken: "fixture-only-secret" }; },
    replaceRouting: async input => {
      calls.replace++;
      return { ...profile(input.expectedRevision + 1), routes: input.routes.map((route, index) => ({ ...route, id: `saved-${index}`, createdAt: "", updatedAt: "" })) };
    },
    resetToken: async () => { calls.reset++; return { token, apiToken: "fixture-reset-secret" }; },
  };
  const session = new ModelRoutingSession(services);
  session.start();
  const edit = () => session.edit(routes => routes.map(route => ({ ...route, requestedModel: "edited-model" })));
  return { session, services, calls, edit, token };
}

test("initial load fetches the key once; metadata refresh never fetches it or overlaps", async () => {
  const f = fixture();
  assert.equal(await f.session.load(), true);
  assert.equal(f.session.getSnapshot().rawToken, "fixture-only-secret");
  assert.equal(f.session.getSnapshot().showToken, false);
  await f.session.refresh();
  await f.session.load();
  assert.deepEqual(f.calls, { routing: 3, token: 1, replace: 0, reset: 0 });
  const pending = deferred<UserModelRoutingResponse>();
  let reads = 0;
  f.services.getRouting = () => { reads++; return pending.promise; };
  const first = f.session.refresh();
  await f.session.refresh();
  assert.equal(reads, 1);
  pending.resolve(profile());
  await first;
});

test("background metadata and manual reload preserve dirty drafts and their baseline", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const draft = f.session.getSnapshot().routes;
  f.services.getRouting = async () => ({ ...profile(), eligibleShares: [{ ...profile().eligibleShares[0], isOnline: false }] });
  await f.session.refresh();
  await f.session.load();
  assert.equal(f.session.getSnapshot().routes, draft);
  assert.equal(f.session.getSnapshot().profile!.routes[0].requestedModel, "*");
  assert.equal(f.session.getSnapshot().profile!.eligibleShares[0].isOnline, false);
  assert.equal(routingDraftIsDirty(f.session.getSnapshot()), true);
  assert.equal(f.session.getSnapshot().conflict, false);
});

test("a changed remote revision flags conflict without rebasing; only explicit discard reloads", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  f.services.getRouting = async () => profile(2);
  await f.session.refresh();
  assert.equal(f.session.getSnapshot().conflict, true);
  assert.equal(f.session.getSnapshot().profile!.revision, 1);
  assert.equal(f.session.getSnapshot().routes[0].requestedModel, "edited-model");
  await f.session.save();
  assert.equal(f.calls.replace, 0);
  assert.equal(await f.session.load(true), true);
  assert.equal(f.session.getSnapshot().conflict, false);
  assert.equal(f.session.getSnapshot().profile!.revision, 2);
  assert.equal(routingDraftIsDirty(f.session.getSnapshot()), false);
});

test("clean drafts follow remote changes; losing Share access leaves a removable missing target", async () => {
  const f = fixture();
  await f.session.load();
  f.services.getRouting = async () => ({ ...profile(3), eligibleShares: [] });
  await f.session.refresh();
  assert.equal(f.session.getSnapshot().profile!.revision, 3);
  assert.equal(f.session.getSnapshot().routes[0].targetShareId, "share");
  f.edit();
  await f.session.save();
  assert.equal(f.session.getSnapshot().validation, "target_unavailable");
  assert.equal(f.calls.replace, 0);
  f.session.edit(() => []);
  await f.session.save();
  assert.equal(f.calls.replace, 1);
});

test("saved unavailable targets do not block unrelated repairs, but new targets and promotions do", async () => {
  const f = fixture();
  f.services.getRouting = async () => ({ ...profile(), routes: [{ ...profile().routes[0], candidates: ["revoked"] }] });
  await f.session.load();
  f.session.edit(routes => [...routes, { ...routes[0], clientId: "new", requestedModel: "another-model", candidates: [] }]);
  await f.session.save();
  assert.equal(f.calls.replace, 1, "preserved stale backup does not block the other route");
  f.session.edit(routes => routes.map(route => ({ ...route, candidates: [...(route.candidates || []), "missing"] })));
  await f.session.save();
  assert.equal(f.calls.replace, 1);
  assert.equal(f.session.getSnapshot().validation, "target_unavailable");
  f.session.edit(routes => routes.map(route => ({ ...route, targetShareId: "revoked", candidates: [] })));
  await f.session.save();
  assert.equal(f.calls.replace, 1, "a revoked backup cannot be promoted to primary");
});

test("an in-flight refresh cannot overwrite a later save, while loading and saving lock edits", async () => {
  const f = fixture();
  const loading = deferred<UserModelRoutingResponse>();
  f.services.getRouting = () => loading.promise;
  const initial = f.session.load();
  f.edit();
  assert.deepEqual(f.session.getSnapshot().routes, []);
  loading.resolve(profile());
  await initial;
  f.edit();
  const stale = deferred<UserModelRoutingResponse>();
  f.services.getRouting = () => stale.promise;
  const refresh = f.session.refresh();
  const saved = deferred<UserModelRoutingResponse>();
  f.services.replaceRouting = input => {
    assert.equal(input.expectedRevision, 1);
    assert.equal(input.routes[0].requestedModel, "edited-model");
    return saved.promise;
  };
  const save = f.session.save();
  f.session.edit(() => []);
  assert.equal(f.session.getSnapshot().routes.length, 1);
  const update = profile(2);
  update.routes[0].requestedModel = "edited-model";
  saved.resolve(update);
  await save;
  stale.resolve(profile());
  await refresh;
  assert.equal(f.session.getSnapshot().profile!.revision, 2);
  assert.equal(f.session.getSnapshot().routes[0].requestedModel, "edited-model");
  assert.equal(routingDraftIsDirty(f.session.getSnapshot()), false);
  assert.equal(f.session.getSnapshot().busy, false);
});

test("network failures keep drafts; 409 keeps drafts and disables stale saves", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const draft = f.session.getSnapshot().routes;
  f.services.getRouting = async () => { throw new Error("offline"); };
  await f.session.refresh();
  assert.equal(f.session.getSnapshot().routes, draft);
  assert.equal(f.session.getSnapshot().refreshError, "offline");
  f.services.replaceRouting = async () => { throw Object.assign(new Error("conflict"), { status: 409 }); };
  await f.session.save();
  assert.equal(f.session.getSnapshot().routes, draft);
  assert.equal(f.session.getSnapshot().conflict, true);
  assert.equal(f.session.getSnapshot().busy, false);
});

test("an App capability conflict is not misreported as a remote route revision conflict", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  f.services.replaceRouting = async () => {
    throw Object.assign(new Error("App unavailable"), { status: 409, code: "user_model_route_app_unavailable" });
  };
  await f.session.save();
  assert.equal(f.session.getSnapshot().conflict, false);
  assert.equal(f.session.getSnapshot().error, "App unavailable");
  assert.equal(routingDraftIsDirty(f.session.getSnapshot()), true);
});

test("401/403 discard cached accounts, routes and keys; explicit reload can recover", async () => {
  for (const status of [401, 403]) {
    for (const operation of ["refresh", "save", "resetToken"] as const) {
      const f = fixture();
      await f.session.load();
      f.edit();
      f.session.setShowToken(true);
      const failed = async () => { throw Object.assign(new Error("denied"), { status }); };
      f.services.getRouting = failed;
      f.services.replaceRouting = failed;
      f.services.resetToken = failed;
      await f.session[operation]();
      const state = f.session.getSnapshot();
      assert.equal(state.profile, null);
      assert.deepEqual(state.routes, []);
      assert.equal(state.rawToken, "");
      assert.equal(state.showToken, false);
      assert.equal(state.busy, false);
      assert.equal(state.error, "denied");
      assert.equal(state.refreshError, "", "permission loss must not claim the draft was retained");
      f.services.getRouting = async () => profile();
      assert.equal(await f.session.load(), true);
      await f.session.refresh();
      assert.equal(f.session.getSnapshot().profile!.revision, 1);
    }
  }
});

test("stopping an actor clears everything and rejects late initial loads and token resets", async () => {
  const f = fixture();
  const pending = deferred<UserModelRoutingResponse>();
  f.services.getRouting = () => pending.promise;
  const load = f.session.load();
  f.session.stop();
  pending.resolve(profile());
  assert.equal(await load, false);
  assert.equal(f.session.getSnapshot().profile, null);
  assert.equal(f.session.getSnapshot().rawToken, "");
  assert.equal(await f.session.resetToken(), false);
  f.session.start();
  await f.session.load();
  const reset = deferred<UserApiTokenResetResponse>();
  f.services.resetToken = () => reset.promise;
  const resetting = f.session.resetToken();
  f.session.stop();
  reset.resolve({ token: f.token, apiToken: "old-actor-secret" });
  assert.equal(await resetting, false);
  assert.equal(f.session.getSnapshot().rawToken, "");
  assert.deepEqual(f.session.getSnapshot().routes, []);
});

test("late saves from an old generation never update a new actor's profile or key", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const pending = deferred<UserModelRoutingResponse>();
  f.services.replaceRouting = () => pending.promise;
  const save = f.session.save();
  f.session.stop();
  f.session.start();
  f.services.getRouting = async () => ({ ...profile(50), routes: [], eligibleShares: [] });
  f.services.getToken = async () => ({ token: f.token, apiToken: "next-actor-key" });
  await f.session.load();
  pending.resolve(profile(2));
  await save;
  assert.equal(f.session.getSnapshot().profile!.revision, 50);
  assert.deepEqual(f.session.getSnapshot().routes, []);
  assert.equal(f.session.getSnapshot().rawToken, "next-actor-key");
});

test("invalid drafts never reach the server and subscribers see immutable snapshots", async () => {
  const f = fixture();
  let updates = 0;
  const unsubscribe = f.session.subscribe(() => { updates++; });
  const empty = f.session.getSnapshot();
  await f.session.load();
  assert.equal(empty.profile, null);
  assert.ok(updates > 0);
  f.session.edit(routes => routes.map(route => ({ ...route, candidates: [route.targetShareId] })));
  await f.session.save();
  assert.equal(f.session.getSnapshot().validation, "duplicate_candidate");
  assert.equal(f.calls.replace, 0);
  unsubscribe();
  const count = updates;
  f.edit();
  assert.equal(updates, count);
});

test("suspending an unfinished initial load allows resume without a stuck loading flag", async () => {
  const f = fixture();
  const old = deferred<UserModelRoutingResponse>();
  f.services.getRouting = () => old.promise;
  const loading = f.session.load();
  f.session.suspend();
  assert.equal(f.session.getSnapshot().loading, false);
  f.services.getRouting = async () => profile(2);
  await f.session.resume();
  old.resolve(profile(1));
  await loading;
  assert.equal(f.session.getSnapshot().profile?.revision, 2);
  assert.equal(f.session.getSnapshot().loading, false);
});

test("suspend retains dirty draft, clears plaintext and resume refetches credentials", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const routes = f.session.getSnapshot().routes;
  f.session.suspend();
  assert.equal(f.session.getSnapshot().rawToken, "");
  assert.equal(f.session.getSnapshot().routes, routes);
  await f.session.resume();
  assert.equal(f.session.getSnapshot().routes, routes);
  assert.equal(f.session.getSnapshot().rawToken, "fixture-only-secret");
  assert.equal(f.calls.token, 2);
  assert.equal(f.calls.replace, 0);
});

test("saving does not invalidate an in-progress credential read", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  f.session.suspend();
  const key = deferred<{ token: typeof f.token; apiToken: string }>();
  f.services.getToken = () => key.promise;
  const resuming = f.session.resume();
  await f.session.save();
  key.resolve({ token: f.token, apiToken: "resumed-secret" });
  await resuming;
  assert.equal(f.session.getSnapshot().rawToken, "resumed-secret");
  assert.equal(f.calls.replace, 1);
});

test("saving across suspend commits configuration without exposing credentials", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const result = deferred<UserModelRoutingResponse>();
  f.services.replaceRouting = () => result.promise;
  const saving = f.session.save();
  f.session.suspend();
  result.resolve(profile(2));
  await saving;
  assert.equal(f.session.getSnapshot().profile?.revision, 2);
  assert.equal(f.session.getSnapshot().busy, false);
  assert.equal(f.session.getSnapshot().rawToken, "");
});

test("authorization failure suspends automatic reads until explicit recovery", async () => {
  const f = fixture();
  await f.session.load();
  let reads = 0;
  f.services.getRouting = async () => { reads++; throw Object.assign(new Error("expired"), { status: 401 }); };
  await f.session.refresh();
  for (let index = 0; index < 3; index++) { await f.session.refresh(); await f.session.resume(); }
  assert.equal(reads, 1);
  f.services.getRouting = async () => { reads++; return profile(3); };
  assert.equal(await f.session.load(), true);
  assert.equal(reads, 2);
});

test("explicit discard supersedes background reads and their late finally", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const old = deferred<UserModelRoutingResponse>();
  f.services.getRouting = () => old.promise;
  const refreshing = f.session.refresh();
  f.services.getRouting = async () => profile(3);
  assert.equal(await f.session.load(true), true);
  old.resolve(profile(2));
  await refreshing;
  assert.equal(f.session.getSnapshot().profile?.revision, 3);
  assert.equal(routingDraftIsDirty(f.session.getSnapshot()), false);
  assert.equal(f.session.getSnapshot().loading, false);
});

test("authentication recovery remains explicit after hiding and showing the panel", async () => {
  for (const status of [401, 403]) {
    const f = fixture();
    await f.session.load();
    let reads = 0;
    f.services.getRouting = async () => { reads++; throw Object.assign(new Error("expired"), { status }); };
    await f.session.refresh();
    f.session.suspend();
    await f.session.resume();
    await f.session.refresh();
    assert.equal(reads, 1);
    assert.equal(await f.session.load(), false);
    await f.session.resume();
    assert.equal(reads, 2, "failed explicit recovery must not re-enable automatic reads");
    f.services.getRouting = async () => { reads++; return profile(3); };
    assert.equal(await f.session.load(), true);
    assert.equal(reads, 3);
  }
});

test("dirty drafts retain their baseline while capabilities track the current server", async () => {
  const f = fixture();
  const capabilities = { savedRoute: true, revisionCheck: true, totalTimeoutMs: 30000, maxAttempts: 4, responsePreviewBytes: 16384 };
  f.services.getRouting = async () => ({ ...profile(), testCapabilities: capabilities });
  await f.session.load();
  f.edit();
  const draft = f.session.getSnapshot().routes;
  f.services.getRouting = async () => profile();
  await f.session.refresh();
  assert.equal(f.session.getSnapshot().profile?.testCapabilities, undefined);
  assert.equal(f.session.getSnapshot().routes, draft);
  f.services.getRouting = async () => ({ ...profile(2), testCapabilities: capabilities });
  await f.session.refresh();
  assert.equal(f.session.getSnapshot().profile?.testCapabilities, capabilities);
  assert.equal(f.session.getSnapshot().profile?.revision, 1);
  assert.equal(f.session.getSnapshot().conflict, true);
  assert.equal(f.session.getSnapshot().routes, draft);
});

test("a failed save across suspend retains drafts and resume coalesces the deferred refresh", async () => {
  const f = fixture();
  await f.session.load();
  f.edit();
  const draft = f.session.getSnapshot().routes;
  const pending = deferred<UserModelRoutingResponse>();
  f.services.replaceRouting = () => pending.promise;
  const saving = f.session.save();
  f.session.suspend();
  await f.session.resume();
  await f.session.resume();
  assert.equal(f.calls.routing, 1);
  pending.reject(new Error("offline"));
  await saving;
  await Promise.resolve();
  assert.equal(f.session.getSnapshot().routes, draft);
  assert.equal(f.session.getSnapshot().error, "offline");
  assert.equal(f.session.getSnapshot().busy, false);
  assert.equal(f.calls.routing, 2);
  assert.equal(f.calls.token, 2);
});

test("resetting the key fences old credential reads across visibility changes", async () => {
  const f = fixture();
  await f.session.load();
  f.session.suspend();
  const old = deferred<{ token: typeof f.token; apiToken: string }>();
  f.services.getToken = () => old.promise;
  const resuming = f.session.resume();
  const reset = deferred<UserApiTokenResetResponse>();
  f.services.resetToken = () => reset.promise;
  const resetting = f.session.resetToken();
  f.session.suspend();
  old.resolve({ token: f.token, apiToken: "obsolete-secret" });
  reset.resolve({ token: f.token, apiToken: "reset-secret" });
  await resuming;
  assert.equal(await resetting, true);
  assert.equal(f.session.getSnapshot().rawToken, "");
  assert.equal(f.session.getSnapshot().showToken, false);
  f.services.getToken = async () => ({ token: f.token, apiToken: "reset-secret" });
  await f.session.resume();
  assert.equal(f.session.getSnapshot().rawToken, "reset-secret");
});
