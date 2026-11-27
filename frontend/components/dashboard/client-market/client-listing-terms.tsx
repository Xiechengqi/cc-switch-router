"use client";
/*
 * Rental terms a seller sets on a Client listing: a public title, free or
 * monthly, and the free-day limit or trial. Shared by "add listing" and
 * "change terms"; changed terms apply to the next order only.
 */
import * as React from "react";
import { Button, Modal, toast } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { parseNonNegativeUsdMinor } from "@/lib/market-wallet-money";
import {
  onlineMarketError,
  updateOnlineListing,
  validListingTitle,
  type ListingTermsPayload,
  type OnlineListing,
} from "@/lib/online-client-market";
import { cn } from "@/lib/utils";

export type ListingTermsDraft = {
  mode: "free" | "monthly";
  price: string;
  freeDays: string | null;
  trialDays: string | null;
  title: string;
};

export const EMPTY_LISTING_TERMS: ListingTermsDraft = { mode: "free", price: "", freeDays: null, trialDays: null, title: "" };

export function draftFromListing(
  listing: Pick<OnlineListing, "cyclePriceMinor" | "freeDays" | "trialDays" | "title">,
): ListingTermsDraft {
  const paid = listing.cyclePriceMinor > 0;
  return {
    mode: paid ? "monthly" : "free",
    price: paid ? (listing.cyclePriceMinor / 100).toFixed(2) : "",
    freeDays: !paid && listing.freeDays ? String(listing.freeDays) : null,
    trialDays: paid && listing.trialDays > 0 ? String(listing.trialDays) : null,
    title: listing.title ?? "",
  };
}

/** Validate the draft. `payload` is present only when every field is valid. */
export function listingTermsFrom(draft: ListingTermsDraft) {
  const paid = draft.mode === "monthly";
  const days = paid ? draft.trialDays : draft.freeDays;
  const maxDays = paid ? 30 : 365;
  const validDays = days === null || (/^\d+$/.test(days) && Number(days) >= 1 && Number(days) <= maxDays);
  const minor = paid ? parseNonNegativeUsdMinor(draft.price.trim().replace(/^0+(?=\d)/, "")) : 0;
  const validPrice = !paid || (minor !== null && minor > 0 && minor <= 100_000_000);
  const validTitle = validListingTitle(draft.title);
  const payload: ListingTermsPayload | null =
    validDays && validPrice && validTitle && minor !== null
      ? {
          cyclePriceMinor: minor,
          freeDays: !paid && draft.freeDays !== null ? Number(draft.freeDays) : null,
          trialDays: paid && draft.trialDays !== null ? Number(draft.trialDays) : 0,
          title: draft.title.trim() || null,
        }
      : null;
  return { paid, days, maxDays, validDays, validPrice, validTitle, payload };
}

const numberField =
  "flex h-11 min-w-0 items-center gap-2 rounded-lg border border-border bg-background px-3 focus-within:border-accent focus-within:ring-2 focus-within:ring-accent focus-within:ring-offset-2";
const textAction =
  "min-h-10 shrink-0 rounded-md px-1 text-sm font-medium text-primary hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent focus-visible:ring-offset-2";

export function ListingTermsFields({
  draft,
  onChange,
  disabled,
  idPrefix,
  showPricing = true,
}: {
  draft: ListingTermsDraft;
  onChange: (next: ListingTermsDraft) => void;
  disabled: boolean;
  idPrefix: string;
  /** Hidden while a paid-listing requirement is being resolved. */
  showPricing?: boolean;
}) {
  const { t } = useLocaleText();
  const { paid, days, maxDays, validDays, validPrice, validTitle } = listingTermsFrom(draft);
  const set = (patch: Partial<ListingTermsDraft>) => onChange({ ...draft, ...patch });
  const setDays = (value: string | null) => set(paid ? { trialDays: value } : { freeDays: value });
  return (
    <div className="grid gap-4">
      <label className="grid gap-1.5 text-sm font-medium">
        <span className="flex items-baseline justify-between gap-2">
          {t("clientMarket.listing.title")}
          <span className="text-xs font-normal tabular-nums text-muted-foreground">{[...draft.title.trim()].length}/30</span>
        </span>
        <input
          className={cn(numberField, "font-normal outline-none placeholder:text-muted-foreground/50")}
          value={draft.title}
          maxLength={60}
          disabled={disabled}
          placeholder={t("clientMarket.listing.titlePlaceholder")}
          aria-invalid={!validTitle}
          aria-describedby={`${idPrefix}-title-help`}
          onChange={(event) => set({ title: event.target.value })}
        />
        <span id={`${idPrefix}-title-help`} className={cn("text-xs font-normal", validTitle ? "text-muted-foreground" : "text-danger")}>
          {t(validTitle ? "clientMarket.listing.titleHelp" : "clientMarket.listing.titleError")}
        </span>
      </label>
      <fieldset disabled={disabled}>
        <legend className="mb-2 text-sm font-medium">{t("clientMarket.listing.listingMode")}</legend>
        <div className="grid grid-cols-2 gap-1 rounded-lg bg-secondary p-1">
          {(["free", "monthly"] as const).map((value) => (
            <label key={value} className="min-w-0 cursor-pointer">
              <input
                className="peer sr-only"
                type="radio"
                name={`${idPrefix}-rent-mode`}
                value={value}
                checked={draft.mode === value}
                onChange={() => set({ mode: value })}
              />
              <span className="flex min-h-10 items-center justify-center rounded-md text-sm font-medium text-muted-foreground peer-checked:bg-card peer-checked:text-foreground peer-checked:shadow-sm peer-focus-visible:ring-2 peer-focus-visible:ring-accent peer-disabled:opacity-60">
                {t(value === "free" ? "online.free" : "clientMarket.listing.monthly")}
              </span>
            </label>
          ))}
        </div>
      </fieldset>
      {showPricing ? (
        <>
          {paid ? (
            <label className="grid gap-1.5 text-sm font-medium">
              {t("clientMarket.listing.amount")}
              <span className={numberField}>
                <input
                  className="w-0 flex-1 bg-transparent font-normal tabular-nums outline-none"
                  required
                  inputMode="decimal"
                  placeholder="0.00"
                  value={draft.price}
                  disabled={disabled}
                  aria-describedby={draft.price && !validPrice ? `${idPrefix}-price-error` : undefined}
                  aria-invalid={!!draft.price && !validPrice}
                  onChange={(event) => set({ price: event.target.value })}
                />
                <span className="text-xs text-muted-foreground">{t("clientMarket.listing.amountUnit")}</span>
              </span>
              {!!draft.price && !validPrice ? (
                <span id={`${idPrefix}-price-error`} className="text-xs font-normal text-danger">
                  {t("clientMarket.listing.priceError")}
                </span>
              ) : null}
            </label>
          ) : null}
          <div className="grid gap-1.5">
            <div className="flex min-h-10 flex-wrap items-center gap-x-3">
              <span id={`${idPrefix}-term`} className="text-sm font-medium">
                {t(paid ? "clientMarket.listing.trial" : "clientMarket.listing.freeDays")}
              </span>
              <span className="ml-auto text-sm text-muted-foreground">
                {days === null && t(paid ? "clientMarket.listing.noTrial" : "clientMarket.listing.unlimited")}
              </span>
              <button
                type="button"
                className={textAction}
                aria-expanded={days !== null}
                disabled={disabled}
                onClick={() => setDays(days === null ? "" : null)}
              >
                {t(
                  days === null
                    ? paid
                      ? "clientMarket.listing.addTrial"
                      : "clientMarket.listing.setLimit"
                    : paid
                      ? "clientMarket.listing.removeTrial"
                      : "clientMarket.listing.removeLimit",
                )}
              </button>
            </div>
            {days !== null ? (
              <>
                <div className={numberField}>
                  <input
                    required
                    aria-labelledby={`${idPrefix}-term`}
                    inputMode="numeric"
                    placeholder={`1–${maxDays}`}
                    className="w-0 flex-1 bg-transparent text-sm tabular-nums outline-none"
                    value={days}
                    disabled={disabled}
                    aria-describedby={days && !validDays ? `${idPrefix}-days-error` : undefined}
                    aria-invalid={!!days && !validDays}
                    onChange={(event) => setDays(event.target.value)}
                  />
                  <span className="text-xs text-muted-foreground">{t("clientMarket.listing.days")}</span>
                </div>
                {!!days && !validDays ? (
                  <p id={`${idPrefix}-days-error`} className="text-xs text-danger">
                    {t("clientMarket.listing.daysError", { max: maxDays })}
                  </p>
                ) : null}
              </>
            ) : null}
          </div>
        </>
      ) : null}
    </div>
  );
}

/** Change the title and terms of a listed Client. Signed rentals keep theirs. */
export function ClientListingEditDialog({
  listing,
  onClose,
  onSaved,
}: {
  listing: OnlineListing | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const { t } = useLocaleText();
  const idPrefix = React.useId();
  const [draft, setDraft] = React.useState<ListingTermsDraft>(EMPTY_LISTING_TERMS);
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  React.useEffect(() => {
    if (listing) setDraft(draftFromListing(listing));
    setError("");
    setBusy(false);
  }, [listing?.installationId]);
  const { payload } = listingTermsFrom(draft);
  async function save() {
    if (!listing || !payload || busy) return;
    setBusy(true);
    setError("");
    try {
      const result = await updateOnlineListing(listing.installationId, payload);
      toast.success(t(result?.termsChanged ? "clientMarket.edit.savedTerms" : "clientMarket.edit.saved"));
      onSaved();
      onClose();
    } catch (cause) {
      setError(onlineMarketError(cause, t));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Modal.Backdrop isOpen={!!listing} isDismissable={!busy} onOpenChange={(open) => !open && !busy && onClose()}>
      <Modal.Container placement="center" scroll="inside" className="w-full max-w-[32rem] p-3">
        <Modal.Dialog className="flex max-h-full min-h-0 w-full flex-col overflow-hidden rounded-2xl! border border-border bg-card! p-0 text-card-foreground">
          <Modal.Header className="shrink-0 px-5 pt-5 pb-2 sm:px-6">
            <Modal.Heading className="text-lg font-semibold tracking-tight">{t("clientMarket.edit.title")}</Modal.Heading>
            <p className="mt-1 text-xs text-muted-foreground">{t("clientMarket.edit.hint")}</p>
          </Modal.Header>
          <Modal.Body className="min-h-0 flex-1 overflow-y-auto px-5 pb-5 sm:px-6">
            <form
              id={`${idPrefix}-form`}
              onSubmit={(event) => {
                event.preventDefault();
                void save();
              }}
            >
              <ListingTermsFields draft={draft} onChange={setDraft} disabled={busy} idPrefix={idPrefix} />
            </form>
            {error ? <p role="alert" className="mt-3 text-sm text-danger">{error}</p> : null}
          </Modal.Body>
          <Modal.Footer className="mt-0! flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3 sm:px-6">
            <Button variant="ghost" isDisabled={busy} onPress={onClose}>{t("common.cancel")}</Button>
            <Button type="submit" form={`${idPrefix}-form`} variant="primary" isDisabled={busy || !payload}>
              {busy ? <Loader2 className="size-4 animate-spin motion-reduce:animate-none" aria-hidden /> : null}
              {t("common.save")}
            </Button>
          </Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
