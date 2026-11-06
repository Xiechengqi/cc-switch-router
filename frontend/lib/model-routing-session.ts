import type {
  ReplaceUserModelRoutingRequest, UserApiTokenResponse, UserApiTokenResetResponse,
  UserApiTokenStatus, UserModelRoutingResponse,
} from "@/lib/types";
import {
  canonicalModelRoutes, hasNewUnavailableRouteTarget, validateModelRoutes,
  applyModelRouteCommand, type ModelRouteCommand, type ModelRouteCommandError,
  type DraftModelRoute, type ModelRouteValidationError,
} from "@/lib/model-routing";

type Services = {
  getRouting: (signal?: AbortSignal) => Promise<UserModelRoutingResponse>;
  getToken: (signal?: AbortSignal) => Promise<UserApiTokenResponse>;
  replaceRouting: (input: ReplaceUserModelRoutingRequest) => Promise<UserModelRoutingResponse>;
  resetToken: () => Promise<UserApiTokenResetResponse>;
};

export type ModelRoutingSessionState = {
  profile: UserModelRoutingResponse | null;
  routes: DraftModelRoute[];
  token: UserApiTokenStatus | null;
  rawToken: string;
  showToken: boolean;
  loading: boolean;
  busy: boolean;
  error: string;
  refreshError: string;
  conflict: boolean;
  validation: ModelRouteValidationError | "target_unavailable" | null;
  commandError: ModelRouteCommandError | null;
};

function emptyState(): ModelRoutingSessionState {
  return {
    profile: null, routes: [], token: null, rawToken: "", showToken: false,
    loading: false, busy: false, error: "", refreshError: "", conflict: false, validation: null, commandError: null,
  };
}

export function routingDrafts(profile: UserModelRoutingResponse): DraftModelRoute[] {
  return profile.routes.map(route => ({
    clientId: route.id, appType: route.appType, requestedModel: route.requestedModel,
    targetShareId: route.targetShareId, candidates: route.candidates || [],
  }));
}

export function routingDraftIsDirty(state: Pick<ModelRoutingSessionState, "profile" | "routes">) {
  return !!state.profile && JSON.stringify(canonicalModelRoutes(state.routes))
    !== JSON.stringify(canonicalModelRoutes(state.profile.routes));
}

/** One instance per signed-in actor. Async replies may only update their own
 * generation; background reads never overwrite edits or fetch the API key. */
export class ModelRoutingSession {
  private state = emptyState();
  private listeners = new Set<() => void>();
  private active = false;
  private visible = false;
  private epoch = 0;
  private visibilityEpoch = 0;
  private sequence = 0;
  private credentialSequence = 0;
  private writeSequence = 0;
  private reading = false;
  private credentialRead: Promise<void> | null = null;
  private routeAbort: AbortController | null = null;
  private credentialAbort: AbortController | null = null;
  private pendingRefresh = false;
  private authInvalid = false;

  constructor(private services: Services) { }

  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => { this.listeners.delete(listener); };
  };
  private patch(change: Partial<ModelRoutingSessionState>) {
    this.state = { ...this.state, ...change };
    this.listeners.forEach(listener => listener());
  }
  start() {
    if (this.active) return;
    this.authInvalid = false;
    this.active = true;
    this.visible = true;
    this.epoch++;
  }
  private cancelRouteRead() {
    this.sequence++;
    this.routeAbort?.abort();
    this.routeAbort = null;
    this.reading = false;
  }
  private cancelReads() {
    this.cancelRouteRead();
    this.credentialSequence++;
    this.credentialAbort?.abort();
    this.credentialAbort = null;
    this.credentialRead = null;
  }
  suspend() {
    if (!this.active) return;
    this.visible = false;
    this.visibilityEpoch++;
    this.pendingRefresh = false;
    this.cancelReads();
    this.patch({ loading: false, rawToken: "", showToken: false });
  }
  resume = async (): Promise<void> => {
    if (!this.active) return;
    if (!this.visible) { this.visible = true; this.visibilityEpoch++; }
    if (this.authInvalid) return;
    if (this.state.busy) { this.pendingRefresh = true; return; }
    if (!this.state.profile) await this.load();
    else await Promise.all([this.refresh(), this.ensureKey()]);
  };
  stop() {
    this.active = false;
    this.visible = false;
    this.epoch++;
    this.visibilityEpoch++;
    this.writeSequence++;
    this.pendingRefresh = false;
    this.cancelReads();
    this.patch(emptyState());
  }
  private current(epoch: number, sequence?: number) {
    return this.active && this.epoch === epoch
      && (sequence == null || this.visible && sequence === this.sequence);
  }
  private failure(error: unknown, background = false) {
    const status = (error as { status?: number } | null)?.status;
    const message = error instanceof Error ? error.message : String(error);
    if (status === 401 || status === 403) {
      // Permission loss invalidates cached identity and account details too.
      this.authInvalid = true;
      this.epoch++;
      this.writeSequence++;
      this.cancelReads();
      this.pendingRefresh = false;
      this.patch({ ...emptyState(), error: message });
      return;
    }
    this.patch({ [background ? "refreshError" : "error"]: message });
  }
  private receive(profile: UserModelRoutingResponse, discard: boolean) {
    if (!discard && routingDraftIsDirty(this.state) && this.state.profile) {
      this.patch({
        profile: { ...profile, revision: this.state.profile.revision, routes: this.state.profile.routes },
        conflict: this.state.conflict || profile.revision !== this.state.profile.revision,
        refreshError: "",
      });
    } else {
      this.patch({ profile, routes: routingDrafts(profile), conflict: false, validation: null, commandError: null, refreshError: "" });
    }
  }
  private ensureKey = async (): Promise<void> => {
    if (!this.active || !this.visible || this.authInvalid || this.state.busy || this.state.rawToken) return;
    if (this.credentialRead) return this.credentialRead;
    const epoch = this.epoch;
    const visibility = this.visibilityEpoch;
    const sequence = ++this.credentialSequence;
    const abort = new AbortController();
    this.credentialAbort = abort;
    const current = () => this.current(epoch) && this.visible
      && visibility === this.visibilityEpoch && sequence === this.credentialSequence;
    const read = (async () => {
      try {
        const token = await this.services.getToken(abort.signal);
        if (current()) this.patch({ token: token.token, rawToken: token.apiToken || "", showToken: false });
      } catch (error) {
        if (current()) this.failure(error);
      } finally {
        if (current()) { this.credentialRead = null; this.credentialAbort = null; }
      }
    })();
    this.credentialRead = read;
    return read;
  };
  load = async (discard = false): Promise<boolean> => {
    if (!this.active || !this.visible || this.state.busy) return false;
    if (discard) { this.cancelRouteRead(); this.patch({ loading: false }); }
    if (this.state.loading || this.reading) return false;
    this.authInvalid = false;
    const epoch = this.epoch;
    const sequence = ++this.sequence;
    this.reading = true;
    const abort = new AbortController();
    this.routeAbort = abort;
    this.patch({ loading: true, error: "" });
    try {
      const [profile] = await Promise.all([
        this.services.getRouting(abort.signal), this.ensureKey(),
      ]);
      if (!this.current(epoch, sequence)) return false;
      this.receive(profile, discard);
      return true;
    } catch (error) {
      if (this.current(epoch, sequence)) this.failure(error);
      return false;
    } finally {
      if (this.current(epoch, sequence)) {
        this.reading = false;
        this.routeAbort = null;
        this.patch({ loading: false });
      }
    }
  };
  refresh = async (): Promise<void> => {
    if (!this.active || !this.visible || this.authInvalid) return;
    if (this.state.busy) { this.pendingRefresh = true; return; }
    if (!this.state.profile) { await this.load(); return; }
    if (this.state.loading || this.reading) return;
    const epoch = this.epoch;
    const sequence = ++this.sequence;
    this.reading = true;
    const abort = new AbortController();
    this.routeAbort = abort;
    try {
      const profile = await this.services.getRouting(abort.signal);
      if (this.current(epoch, sequence)) this.receive(profile, false);
    } catch (error) {
      if (this.current(epoch, sequence)) this.failure(error, true);
    } finally {
      if (this.current(epoch, sequence)) { this.reading = false; this.routeAbort = null; }
    }
  };
  edit = (update: (routes: DraftModelRoute[]) => DraftModelRoute[]) => {
    if (!this.active || !this.visible || !this.state.profile || this.state.busy || this.state.loading) return;
    this.patch({ routes: update(this.state.routes), error: "", validation: null });
  };
  command = (command: ModelRouteCommand) => {
    const { profile, routes } = this.state;
    if (!this.active || !this.visible || !profile || this.state.busy || this.state.loading) return;
    if (command.kind === "restore_change" && this.state.conflict) {
      this.patch({ commandError: "stale_diff" }); return;
    }
    const result = applyModelRouteCommand(routes, command, {
      eligibleShares: profile.eligibleShares, savedRoutes: routingDrafts(profile), baseRevision: profile.revision,
    });
    this.patch({ routes: result.routes, commandError: result.error?.code || null, error: "", validation: null });
    return result;
  };
  setShowToken = (value: boolean | ((value: boolean) => boolean)) => {
    if (!this.active || !this.visible || !this.state.rawToken) return;
    this.patch({ showToken: typeof value === "function" ? value(this.state.showToken) : value });
  };
  save = async (): Promise<void> => {
    const { profile, routes } = this.state;
    if (!this.active || !this.visible || !profile || this.state.busy || this.state.loading || this.state.conflict) return;
    const validation = validateModelRoutes(routes) || (hasNewUnavailableRouteTarget(routes, profile) ? "target_unavailable" : null);
    if (validation) { this.patch({ validation }); return; }
    const epoch = this.epoch;
    const write = ++this.writeSequence;
    const input = { expectedRevision: profile.revision, routes: canonicalModelRoutes(routes) };
    this.cancelRouteRead();
    const current = () => this.current(epoch) && write === this.writeSequence;
    this.patch({ busy: true, error: "", validation: null });
    try {
      const updated = await this.services.replaceRouting(input);
      if (current()) this.receive(updated, true);
    } catch (error) {
      if (current()) {
        this.failure(error);
        const apiError = error as { status?: number; code?: string } | null;
        if (apiError?.status === 409 && (!apiError.code || apiError.code === "model_routing_revision_conflict")) {
          this.patch({ conflict: true });
        }
      }
    } finally {
      if (current()) this.finishWrite();
    }
  };
  resetToken = async (): Promise<boolean> => {
    if (!this.active || !this.visible || !this.state.profile || this.state.busy || this.state.loading) return false;
    const epoch = this.epoch;
    const visibility = this.visibilityEpoch;
    const write = ++this.writeSequence;
    this.cancelReads();
    const current = () => this.current(epoch) && write === this.writeSequence;
    this.patch({ busy: true, error: "" });
    try {
      const updated = await this.services.resetToken();
      if (!current()) return false;
      const mayReveal = this.visible && visibility === this.visibilityEpoch;
      this.patch({ token: updated.token, rawToken: mayReveal ? updated.apiToken : "", showToken: mayReveal });
      return true;
    } catch (error) {
      if (current()) this.failure(error);
      return false;
    } finally {
      if (current()) this.finishWrite();
    }
  };
  private finishWrite() {
    this.patch({ busy: false });
    const refresh = this.pendingRefresh;
    this.pendingRefresh = false;
    if (this.visible && !this.authInvalid) {
      if (refresh) void this.resume();
      else void this.ensureKey();
    }
  }
}
