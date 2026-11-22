/**
 * Bridge between the signed fetch layer and the password confirmation dialog.
 *
 * Risky wallet operations answer 428 with a one-time challenge. The fetch
 * layer asks the mounted dialog host for the login password, which is used for
 * exactly that challenge and never stored. Without a mounted host the request
 * fails instead of silently skipping confirmation.
 */

export type FinancialConfirmRisk = "confirm" | "immediate";

export type FinancialPasswordRequest = {
  risk: FinancialConfirmRisk;
  /** Localized error from the previous attempt, e.g. a wrong password. */
  error?: string;
};

export type FinancialConfirmHost = {
  /** Resolves with the password, or `null` when the person cancels. */
  askPassword: (request: FinancialPasswordRequest) => Promise<string | null>;
  /** Opens the "set login password" flow for accounts without a password. */
  requirePassword: () => void;
};

let host: FinancialConfirmHost | null = null;

export function registerFinancialConfirmHost(next: FinancialConfirmHost) {
  host = next;
  return () => {
    if (host === next) host = null;
  };
}

export class FinancialConfirmCancelled extends Error {
  readonly code = "FINANCIAL_CONFIRM_CANCELLED";
  constructor() {
    super("Confirmation cancelled.");
    this.name = "FinancialConfirmCancelled";
  }
}

export async function askFinancialPassword(request: FinancialPasswordRequest): Promise<string> {
  if (!host) throw new FinancialConfirmCancelled();
  const password = await host.askPassword(request);
  if (password == null) throw new FinancialConfirmCancelled();
  return password;
}

export function requireFinancialPassword() {
  host?.requirePassword();
}
