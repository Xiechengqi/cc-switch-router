import type {
  ReplaceUserModelRoutingRequest, UserApiTokenResponse, UserApiTokenResetResponse,
  UserApiTokenStatus, UserModelRoutingResponse,
} from "@/lib/types";
import {
  canonicalModelRoutes, hasNewUnavailableRouteTarget, validateModelRoutes,
  type DraftModelRoute, type ModelRouteValidationError,
} from "@/lib/model-routing";

type Services = {
  getRouting: () => Promise<UserModelRoutingResponse>;
  getToken: () => Promise<UserApiTokenResponse>;
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
};

function emptyState(): ModelRoutingSessionState {
  return {
    profile: null, routes: [], token: null, rawToken: "", showToken: false,
    loading: false, busy: false, error: "", refreshError: "", conflict: false, validation: null
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
  private epoch = 0;
  private sequence = 0;
  private reading = false;

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
  start() { this.active = true; this.epoch++; }
  stop() {
    this.active = false;
    this.epoch++;
    this.sequence++;
    this.reading = false;
    this.patch(emptyState());
  }
  private current(epoch: number, sequence?: number) {
    return this.active && this.epoch === epoch && (sequence == null || sequence === this.sequence);
  }
  private failure(error: unknown, background = false) {
    const status = (error as { status?: number } | null)?.status;
    const message = error instanceof Error ? error.message : String(error);
    if (status === 401 || status === 403) {
      // Permission loss invalidates cached identity and account details too.
      this.sequence++;
      this.reading = false;
      this.patch({ ...emptyState(), error: message });
      return;
    }
    this.patch({ [background ? "refreshError" : "error"]: message });
  }
  private receive(profile: UserModelRoutingResponse, discard: boolean) {
    if (!discard && routingDraftIsDirty(this.state) && this.state.profile) {
      this.patch({
        profile: { ...this.state.profile, eligibleShares: profile.eligibleShares, apiBaseUrl: profile.apiBaseUrl },
        conflict: this.state.conflict || profile.revision !== this.state.profile.revision,
        refreshError: "",
      });
    } else {
      this.patch({ profile, routes: routingDrafts(profile), conflict: false, validation: null, refreshError: "" });
    }
  }
  load = async (discard = false): Promise<boolean> => {
    if (!this.active || this.state.busy || this.state.loading) return false;
    const epoch = this.epoch;
    const sequence = ++this.sequence;
    this.reading = true;
    this.patch({ loading: true, error: "" });
    try {
      const [profile, token] = await Promise.all([
        this.services.getRouting(),
        this.state.token ? Promise.resolve(null) : this.services.getToken(),
      ]);
      if (!this.current(epoch, sequence)) return false;
      this.receive(profile, discard);
      if (token) this.patch({ token: token.token, rawToken: token.apiToken || "", showToken: false });
      return true;
    } catch (error) {
      if (this.current(epoch, sequence)) this.failure(error);
      return false;
    } finally {
      if (this.current(epoch, sequence)) {
        this.reading = false;
        this.patch({ loading: false });
      }
    }
  };
  refresh = async (): Promise<void> => {
    if (!this.active || !this.state.profile || this.state.busy || this.state.loading || this.reading) return;
    const epoch = this.epoch;
    const sequence = ++this.sequence;
    this.reading = true;
    try {
      const profile = await this.services.getRouting();
      if (this.current(epoch, sequence)) this.receive(profile, false);
    } catch (error) {
      if (this.current(epoch, sequence)) this.failure(error, true);
    } finally {
      if (this.current(epoch, sequence)) this.reading = false;
    }
  };
  edit = (update: (routes: DraftModelRoute[]) => DraftModelRoute[]) => {
    if (!this.active || !this.state.profile || this.state.busy || this.state.loading) return;
    this.patch({ routes: update(this.state.routes), error: "", validation: null });
  };
  setShowToken = (value: boolean | ((value: boolean) => boolean)) => {
    if (!this.active || !this.state.rawToken) return;
    this.patch({ showToken: typeof value === "function" ? value(this.state.showToken) : value });
  };
  save = async (): Promise<void> => {
    const { profile, routes } = this.state;
    if (!this.active || !profile || this.state.busy || this.state.loading || this.state.conflict) return;
    const validation = validateModelRoutes(routes) || (hasNewUnavailableRouteTarget(routes, profile) ? "target_unavailable" : null);
    if (validation) { this.patch({ validation }); return; }
    const epoch = this.epoch;
    ++this.sequence;
    this.reading = false;
    this.patch({ busy: true, error: "", validation: null });
    try {
      const updated = await this.services.replaceRouting({ expectedRevision: profile.revision, routes: canonicalModelRoutes(routes) });
      if (this.current(epoch)) this.receive(updated, true);
    } catch (error) {
      if (this.current(epoch)) {
        this.failure(error);
        const apiError = error as { status?: number; code?: string } | null;
        if (apiError?.status === 409 && (!apiError.code || apiError.code === "model_routing_revision_conflict")) {
          this.patch({ conflict: true });
        }
      }
    } finally {
      if (this.current(epoch)) this.patch({ busy: false });
    }
  };
  resetToken = async (): Promise<boolean> => {
    if (!this.active || !this.state.profile || this.state.busy || this.state.loading) return false;
    const epoch = this.epoch;
    ++this.sequence;
    this.reading = false;
    this.patch({ busy: true, error: "" });
    try {
      const updated = await this.services.resetToken();
      if (!this.current(epoch)) return false;
      this.patch({ token: updated.token, rawToken: updated.apiToken, showToken: true });
      return true;
    } catch (error) {
      if (this.current(epoch)) this.failure(error);
      return false;
    } finally {
      if (this.current(epoch)) this.patch({ busy: false });
    }
  };
}
