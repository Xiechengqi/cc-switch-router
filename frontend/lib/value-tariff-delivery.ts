/** ACK is installation evidence, never proof of activation. */
export type ValueTariffHealth = {
  publishedRevision: string | null;
  effectiveFromMs: number | null;
  serverTimeMs?: number;
  publication?: {state: "pending" | "published" | "unknown"};
  delivery?: {expectedInstallations: number; acknowledgedInstallations: number; activatedInstallations: number; unconfirmedInstallations: number};
};
export type ValueTariffState = "pendingPublication" | "missing" | "pendingEffective" | "pendingAck" | "unconfirmed" | "partial" | "live" | "noServers";
export function parseValueTariffHealth(raw: unknown): ValueTariffHealth {
  if (!raw || typeof raw !== "object") throw new Error("Invalid tariff delivery response");
  const data = raw as Record<string, unknown>;
  if (!(data.publishedRevision === null || (typeof data.publishedRevision === "string" && /^sha256:[a-f0-9]{64}$/.test(data.publishedRevision)))
    || !(data.effectiveFromMs === null || (typeof data.effectiveFromMs === "number" && Number.isSafeInteger(data.effectiveFromMs) && data.effectiveFromMs >= 0))) throw new Error("Invalid tariff revision");
  if ((data.publishedRevision === null) !== (data.effectiveFromMs === null)) throw new Error("Incomplete tariff revision");
  const health: ValueTariffHealth = {publishedRevision: data.publishedRevision, effectiveFromMs: data.effectiveFromMs};
  if (data.serverTimeMs != null) {
    if (typeof data.serverTimeMs !== "number" || !Number.isSafeInteger(data.serverTimeMs) || data.serverTimeMs < 0) throw new Error("Invalid Router clock");
    health.serverTimeMs = data.serverTimeMs;
  }
  const publication = data.publication as {state?: unknown} | undefined;
  if (publication != null) {
    if (!["pending", "published", "unknown"].includes(String(publication.state))) throw new Error("Invalid publication state");
    health.publication = {state: publication.state as "pending" | "published" | "unknown"};
  }
  if (data.delivery != null) {
    if (typeof data.delivery !== "object") throw new Error("Invalid tariff counts");
    const counts = data.delivery as Record<string, unknown>;
    for (const key of ["expectedInstallations", "acknowledgedInstallations", "activatedInstallations", "unconfirmedInstallations"]) {
      if (typeof counts[key] !== "number" || !Number.isSafeInteger(counts[key]) || (counts[key] as number) < 0) throw new Error("Invalid tariff count");
    }
    const delivery = counts as ValueTariffHealth["delivery"] & {};
    if (delivery.activatedInstallations > delivery.acknowledgedInstallations || delivery.acknowledgedInstallations > delivery.expectedInstallations
      || delivery.unconfirmedInstallations !== delivery.expectedInstallations - delivery.activatedInstallations) throw new Error("Inconsistent tariff counts");
    health.delivery = delivery;
  }
  return health;
}
export function valueTariffState(health: ValueTariffHealth): ValueTariffState {
  if (health.publication?.state === "pending") return "pendingPublication";
  if (!health.publishedRevision) return "missing";
  if (health.serverTimeMs == null) return "unconfirmed";
  // Only the Router clock and a real activation receipt can prove readiness.
  if (health.serverTimeMs != null && health.effectiveFromMs != null && health.effectiveFromMs > health.serverTimeMs) return "pendingEffective";
  const delivery = health.delivery;
  if (!delivery) return "unconfirmed";
  if (delivery.expectedInstallations === 0) return "noServers";
  if (delivery.activatedInstallations === delivery.expectedInstallations) return "live";
  if (delivery.activatedInstallations > 0) return "partial";
  return delivery.acknowledgedInstallations > 0 ? "unconfirmed" : "pendingAck";
}
