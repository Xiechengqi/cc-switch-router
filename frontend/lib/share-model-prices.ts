import type { shareModelPriceMessagesEn } from "./share-model-price-messages";
import type { ShareModelPriceRates, ShareUserUsageBreakdownResponse } from "./types";

type ModelPriceMessageKey = keyof typeof shareModelPriceMessagesEn;

export const PRICE_RATE_FIELDS = ["input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h"] as const;
export type PriceRateField = typeof PRICE_RATE_FIELDS[number];
export type PriceDraft = Record<PriceRateField, string>;
export const EMPTY_PRICE_DRAFT: PriceDraft = { input: "", output: "", cacheRead: "", cacheWrite5m: "", cacheWrite1h: "" };

const WIRE_FIELD = {
  input: "inputUsdPer1m",
  output: "outputUsdPer1m",
  cacheRead: "cacheReadUsdPer1m",
  cacheWrite5m: "cacheWrite5mUsdPer1m",
  cacheWrite1h: "cacheWrite1hUsdPer1m",
} as const satisfies Record<PriceRateField, keyof ShareModelPriceRates>;

/** Fills the editor from wire rates (an owner price or a suggestion), exactly as sent. */
export function priceDraftFromRates(rates: ShareModelPriceRates | undefined): PriceDraft {
  const draft = { ...EMPTY_PRICE_DRAFT };
  if (!rates) return draft;
  for (const field of PRICE_RATE_FIELDS) {
    const value = rates[WIRE_FIELD[field]];
    if (typeof value === "string") draft[field] = value;
  }
  return draft;
}

export type PriceRateError = "required" | "format" | "below5m";
const RATE_PATTERN = /^(\d*)(?:\.(\d{0,6}))?$/;

/**
 * A rate as the Router's decimal parser accepts it: non-negative, at most six
 * decimals, no sign or exponent. Tolerates a typed `$` and `.5`/`5.`; never
 * goes through floating point.
 */
export function normalizePriceRate(input: string): { ok: true; value: string } | { ok: false; error: PriceRateError } {
  let text = input.trim();
  if (text.startsWith("$")) text = text.slice(1).trim();
  if (!text) return { ok: false, error: "required" };
  const match = RATE_PATTERN.exec(text);
  if (!match || (!match[1] && !match[2])) return { ok: false, error: "format" };
  const whole = (match[1] || "0").replace(/^0+(?=\d)/, "");
  const fraction = (match[2] ?? "").replace(/0+$/, "");
  return { ok: true, value: fraction ? `${whole}.${fraction}` : whole };
}

const isPositive = (value: string) => /[1-9]/.test(value);

/** Compares two normalized rates exactly, as decimals; never via floating point. */
export function comparePriceRates(left: string, right: string): number {
  const [leftWhole, leftFraction = ""] = left.split(".");
  const [rightWhole, rightFraction = ""] = right.split(".");
  if (leftWhole.length !== rightWhole.length) return leftWhole.length < rightWhole.length ? -1 : 1;
  const a = leftWhole + leftFraction.padEnd(6, "0");
  const b = rightWhole + rightFraction.padEnd(6, "0");
  return a === b ? 0 : a < b ? -1 : 1;
}

export type PriceDraftValidation = {
  rates?: ShareModelPriceRates;
  errors: Partial<Record<PriceRateField, PriceRateError>>;
  /** Input and output are both zero: such a price would make the model free. */
  allZero: boolean;
};

/**
 * Every required rate valid, the optional 1h rate valid when given and not
 * below the 5m rate (a 1h write never costs less), and input or output above zero.
 */
export function validatePriceDraft(draft: PriceDraft): PriceDraftValidation {
  const errors: PriceDraftValidation["errors"] = {};
  const values: Partial<Record<PriceRateField, string>> = {};
  for (const field of PRICE_RATE_FIELDS) {
    if (field === "cacheWrite1h" && !draft[field].trim()) continue;
    const parsed = normalizePriceRate(draft[field]);
    if (parsed.ok) values[field] = parsed.value;
    else errors[field] = parsed.error;
  }
  if (values.cacheWrite1h != null && values.cacheWrite5m != null && comparePriceRates(values.cacheWrite1h, values.cacheWrite5m) < 0) {
    errors.cacheWrite1h = "below5m";
  }
  const allZero = values.input != null && values.output != null && !isPositive(values.input) && !isPositive(values.output);
  if (Object.keys(errors).length || allZero) return { errors, allZero };
  return {
    errors,
    allZero,
    rates: {
      inputUsdPer1m: values.input!,
      outputUsdPer1m: values.output!,
      cacheReadUsdPer1m: values.cacheRead!,
      cacheWrite5mUsdPer1m: values.cacheWrite5m!,
      ...(values.cacheWrite1h != null ? { cacheWrite1hUsdPer1m: values.cacheWrite1h } : {}),
    },
  };
}

export type SharePriceAccess = { client: boolean; global: boolean };

/**
 * Who may price a model from a Share. Routers that predate the
 * `canSetPrice`/`canSetGlobalPrice` fields fall back to Share management and
 * the admin session.
 */
export function sharePriceAccess(
  capabilities: Pick<ShareUserUsageBreakdownResponse, "canSetPrice" | "canSetGlobalPrice"> | undefined,
  fallback: { canManage?: boolean; isAdmin?: boolean },
): SharePriceAccess {
  const legacy = capabilities?.canSetPrice == null && capabilities?.canSetGlobalPrice == null;
  return legacy
    ? { client: !!fallback.canManage, global: !!fallback.isAdmin }
    : { client: !!capabilities?.canSetPrice, global: !!capabilities?.canSetGlobalPrice };
}

export type PriceScope = "client" | "global";

/** One translated sentence per save failure; {@link modelPriceErrorMessage} refines validation failures. */
export function modelPriceErrorKey(error: unknown, scope: PriceScope): ModelPriceMessageKey {
  const candidate = error as { status?: number; code?: string } | null;
  const status = candidate?.status, code = candidate?.code;
  if (status === 409 || code === "MODEL_PRICE_VERSION_CONFLICT") return scope === "client" ? "dashboard.modelPrice.conflict.client" : "dashboard.modelPrice.conflict.global";
  if (code === "MODEL_PRICE_INVALID" || status === 400 || status === 422) return "dashboard.modelPrice.error.invalid";
  if (code === "SHARE_FORBIDDEN" || status === 403) return "dashboard.modelPrice.error.forbidden";
  if (code === "SHARE_NOT_FOUND") return "dashboard.modelPrice.error.notFound";
  // A client-scope 404/405 without a Share code is a Router without the endpoint.
  if (scope === "client" && (status === 404 || status === 405)) return "dashboard.modelPrice.error.unsupported";
  return "dashboard.modelPrice.error.failed";
}

const RATE_FIELD_BY_WIRE = Object.fromEntries(
  PRICE_RATE_FIELDS.map((field) => [WIRE_FIELD[field], field]),
) as Record<string, PriceRateField>;

const REASON_KEY: Partial<Record<string, ModelPriceMessageKey>> = {
  rate_above_catalog_max: "dashboard.modelPrice.reason.rateAboveMax",
  rate_negative: "dashboard.modelPrice.reason.rateNegative",
  rate_required: "dashboard.modelPrice.reason.rateRequired",
  rate_invalid: "dashboard.modelPrice.reason.rateInvalid",
  too_many_prices: "dashboard.modelPrice.reason.tooMany",
  cache_write_1h_below_5m: "dashboard.modelPrice.reason.below5m",
  catalog_unavailable: "dashboard.modelPrice.reason.catalogUnavailable",
  catalog_priced: "dashboard.modelPrice.reason.catalogPriced",
  model_key_invalid: "dashboard.modelPrice.reason.modelKeyInvalid",
  display_name_invalid: "dashboard.modelPrice.reason.displayNameInvalid",
};

export type ModelPriceErrorMessage = {
  key: ModelPriceMessageKey;
  params?: Record<string, string | number>;
  /** The rate the Router named; its label fills `{field}`. */
  field?: PriceRateField;
  /** The Router's own words, shown only when its reason has no translation. */
  detail?: string;
};

/**
 * The save failure as a translated sentence: `MODEL_PRICE_INVALID` reasons
 * name the rate and the allowed maximum; anything unknown falls back to the
 * generic sentence with the Router's message as a detail.
 */
export function modelPriceErrorMessage(error: unknown, scope: PriceScope): ModelPriceErrorMessage {
  const key = modelPriceErrorKey(error, scope);
  if (key !== "dashboard.modelPrice.error.invalid") return { key };
  const candidate = error as { message?: unknown; details?: Record<string, unknown> } | null;
  const details = candidate?.details ?? {};
  const reason = typeof details.reason === "string" ? details.reason : "";
  const field = typeof details.field === "string" ? RATE_FIELD_BY_WIRE[details.field] : undefined;
  const reasonKey = REASON_KEY[reason];
  const max = typeof details.maxUsdPer1m === "string" ? details.maxUsdPer1m
    : typeof details.max === "number" ? details.max : undefined;
  // Sentences with a placeholder are only used when the Router filled it.
  const complete = reason === "rate_above_catalog_max" ? !!field && max != null
    : reason === "rate_negative" ? !!field
      : reason === "too_many_prices" ? max != null
        : true;
  if (reasonKey && complete) return { key: reasonKey, params: max != null ? { max } : undefined, field };
  const message = typeof details.detail === "string" && details.detail.trim()
    ? details.detail
    : typeof candidate?.message === "string" ? candidate.message : "";
  return { key, detail: message.trim() ? message.trim().slice(0, 300) : undefined };
}
