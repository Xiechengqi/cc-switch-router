import type { ModelRoutingApp, UserModelRoutingResponse, UserModelRoutingTestRequest, UserModelRoutingTestResponse } from "./types";

export type ModelRoutingTestState = { running: boolean; result: UserModelRoutingTestResponse | null; error: string; unknown: boolean };
/** An instance belongs to one login lifetime. No retries and no queued writes. */
export class ModelRoutingTestSession {
  private state: ModelRoutingTestState = { running: false, result: null, error: "", unknown: false };
  private listeners = new Set<() => void>();
  private sequence = 0;
  private active = true;
  private inFlight = false;
  constructor(private request: (input: UserModelRoutingTestRequest) => Promise<UserModelRoutingTestResponse>) {}
  getSnapshot = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  private patch(change: Partial<ModelRoutingTestState>) { this.state = { ...this.state, ...change }; this.listeners.forEach(listener => listener()); }
  /** A call already sent may still be metered, so dropping it is reported as unknown, never as a clean slate. */
  invalidate() {
    this.sequence++;
    this.patch(this.inFlight ? { result: null, error: "invalidated", unknown: true } : { result: null, error: "", unknown: false });
  }
  start() { this.active = true; }
  stop() { this.active = false; this.invalidate(); }
  run = async (profile: UserModelRoutingResponse, appType: ModelRoutingApp, model: string, mode: "primary" | "saved_route") => {
    if (!this.active || this.inFlight || !model.trim() || model.includes("*")) return;
    const capabilities = profile.testCapabilities;
    if (mode === "saved_route" && (!capabilities?.savedRoute || !capabilities.revisionCheck)) {
      this.patch({ error: "capability_missing", unknown: false }); return;
    }
    const sequence = ++this.sequence;
    this.inFlight = true;
    this.patch({ running: true, result: null, error: "", unknown: false });
    // Legacy requests MUST contain only these original two fields.
    const input: UserModelRoutingTestRequest = { appType, requestedModel: model.trim() };
    if (mode === "saved_route" || capabilities?.revisionCheck) {
      input.mode = mode; input.expectedRevision = profile.revision;
    }
    const current = () => this.active && sequence === this.sequence;
    try {
      const result = await this.request(input);
      if (!current()) return;
      if (result.revision != null && result.revision !== profile.revision
        || mode === "saved_route" && (result.revision !== profile.revision || result.mode !== "saved_route")) {
        this.patch({ error: "snapshot_mismatch", unknown: true }); return;
      }
      this.patch({ result });
    } catch (error) {
      if (!current()) return;
      const status = (error as { status?: number } | null)?.status;
      this.patch({ error: error instanceof Error ? error.message : String(error), unknown: status !== 409 && status !== 429 && status !== 401 && status !== 403 && status !== 422 });
    } finally {
      this.inFlight = false;
      if (this.active) this.patch({ running: false });
    }
  };
}
