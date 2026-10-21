"use client";

/* Owner case desk: open reconciliation cases with translated codes, critical
 * badges and a reason-coded resolve dialog. Top-up cases route to recovery. */

import * as React from "react";
import { Alert, Button, Modal } from "@heroui/react";
import { Loader2 } from "lucide-react";
import { WalletBadge } from "@/components/market/wallet-primitives";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { ApiError } from "@/lib/api";
import {
  getMarketFinanceCases,
  isCriticalMarketFinanceCase,
  resolveMarketFinanceCase,
  validMarketReasonCode,
  type MarketFinanceCase,
} from "@/lib/market-wallet-api";
import { codeLabel, OWNER_DIALOG_CLASS, OwnerField, ownerDateText, ownerErrorText } from "@/components/settings/market-finance-shared";

export const TOPUP_RECOVERY_SECTION_ID = "market-finance-topup-recovery";

export function focusTopupRecovery() {
  const section = document.getElementById(TOPUP_RECOVERY_SECTION_ID);
  if (!section) return;
  section.scrollIntoView({ behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth", block: "start" });
  section.focus({ preventScroll: true });
}

export function MarketFinanceCaseDesk({ refreshKey, onChanged }: { refreshKey: number; onChanged: () => Promise<unknown> }) {
  const { locale, t } = useLocaleText();
  const [cases, setCases] = React.useState<MarketFinanceCase[]>([]);
  const [nextCursor, setNextCursor] = React.useState<string | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [loadingMore, setLoadingMore] = React.useState(false);
  const [selected, setSelected] = React.useState<MarketFinanceCase | null>(null);
  const [reason, setReason] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [error, setError] = React.useState("");
  const [dialogError, setDialogError] = React.useState<{ text: string; topup: boolean } | null>(null);
  const [notice, setNotice] = React.useState("");
  const version = React.useRef(0);

  const load = React.useCallback(async () => {
    const current = ++version.current;
    try {
      const page = await getMarketFinanceCases();
      if (current !== version.current) return;
      setCases(page.items);
      setNextCursor(page.nextCursor);
      setError("");
    } catch (cause) {
      if (current === version.current) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (current === version.current) setLoading(false);
    }
  }, []);

  React.useEffect(() => { void load(); }, [load, refreshKey]);

  const loadMore = async () => {
    if (!nextCursor || loadingMore) return;
    const current = version.current;
    setLoadingMore(true);
    try {
      const page = await getMarketFinanceCases(nextCursor);
      if (current !== version.current) return;
      setCases((items) => {
        const seen = new Set(items.map((item) => item.id));
        return [...items, ...page.items.filter((item) => !seen.has(item.id))];
      });
      setNextCursor(page.nextCursor);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setLoadingMore(false);
    }
  };

  const openResolve = (item: MarketFinanceCase) => {
    setSelected(item);
    setReason("");
    setDialogError(null);
    setNotice("");
  };

  const reasonValid = validMarketReasonCode(reason);
  const submit = async (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (!selected || busy || !reasonValid) return;
    setBusy(true);
    setDialogError(null);
    try {
      await resolveMarketFinanceCase(selected.id, reason.trim());
      setSelected(null);
      setNotice(t("settings.marketFinance.cases.resolved"));
      await Promise.all([load(), onChanged()]).catch(() => {});
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 403) {
        setDialogError({ text: t(isCriticalMarketFinanceCase(selected.code) ? "settings.marketFinance.cases.ownerOnlyCritical" : "settings.marketFinance.owner.ownerOnly"), topup: false });
      } else if (cause instanceof ApiError && cause.status === 409 && selected.objectKind === "topup") {
        setDialogError({ text: t("settings.marketFinance.cases.topupConflict"), topup: true });
      } else {
        setDialogError({ text: ownerErrorText(t, cause), topup: false });
      }
    } finally {
      setBusy(false);
    }
  };

  const goToRecovery = () => {
    setSelected(null);
    window.setTimeout(focusTopupRecovery, 0);
  };

  return (
    <section className="grid min-w-0 gap-3" aria-labelledby="market-finance-case-desk-title">
      <div className="grid gap-1">
        <h4 id="market-finance-case-desk-title" className="text-sm font-semibold">
          {t("settings.marketFinance.cases.title")} <span className="font-normal text-muted-foreground tabular-nums">({cases.length}{nextCursor ? "+" : ""})</span>
        </h4>
        <p className="text-xs text-muted-foreground">{t("settings.marketFinance.cases.note")}</p>
      </div>
      {loading ? <p className="flex items-center gap-2 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" aria-hidden="true" />{t("common.loading")}</p> : null}
      {error ? <Alert status="danger" role="alert">{error}</Alert> : null}
      <div aria-live="polite">{notice ? <Alert status="success">{notice}</Alert> : null}</div>
      {!loading && !cases.length ? <p className="text-sm text-muted-foreground">{t("settings.marketFinance.cases.empty")}</p> : null}
      {cases.length ? (
        <ul className="grid gap-2">
          {cases.map((item) => {
            const critical = isCriticalMarketFinanceCase(item.code);
            return (
              <li key={item.id} className={`grid min-w-0 gap-2 rounded-xl border bg-card p-3 text-sm shadow-sm sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center ${critical ? "border-red-200" : "border-border"}`}>
                <div className="grid min-w-0 gap-1">
                  <div className="flex min-w-0 flex-wrap items-center gap-2">
                    <span className="font-medium">{codeLabel(t, "settings.marketFinance.case.", item.code)}</span>
                    {critical ? <WalletBadge tone="danger">{t("settings.marketFinance.cases.critical")}</WalletBadge> : null}
                  </div>
                  {item.detailCode ? <span className="text-xs text-muted-foreground">{codeLabel(t, "settings.marketFinance.caseDetail.", item.detailCode)}</span> : null}
                  <span className="break-all font-mono text-[11px] text-muted-foreground">{item.code}{item.detailCode ? ` / ${item.detailCode}` : ""} · {item.objectKind}:{item.objectId}</span>
                  <span className="text-xs text-muted-foreground">{t("settings.marketFinance.cases.opened", { time: ownerDateText(item.createdAt, locale) })}</span>
                </div>
                {item.objectKind === "topup" ? (
                  <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" onPress={focusTopupRecovery}>
                    {t("settings.marketFinance.cases.openRecovery")}
                  </Button>
                ) : (
                  <Button size="sm" variant="outline" className="min-h-11 whitespace-nowrap" isDisabled={busy} onPress={() => openResolve(item)}>
                    {t("settings.marketFinance.cases.resolve")}
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      ) : null}
      {nextCursor ? (
        <Button size="sm" variant="outline" className="min-h-11 justify-self-start" isDisabled={loadingMore} onPress={() => void loadMore()}>
          {loadingMore ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
          {t("settings.marketFinance.casesLoadMore")}
        </Button>
      ) : null}

      <Modal.Backdrop isOpen={!!selected} onOpenChange={(open) => { if (!open && !busy) setSelected(null); }}>
        <Modal.Container placement="center">
          <Modal.Dialog className={OWNER_DIALOG_CLASS}>
            <Modal.CloseTrigger />
            <Modal.Header><Modal.Heading>{t("settings.marketFinance.cases.resolveTitle")}</Modal.Heading></Modal.Header>
            <form onSubmit={(event) => void submit(event)}>
              <Modal.Body className="grid gap-3 text-sm">
                {selected ? <div className="grid gap-1">
                  <p className="font-medium">{codeLabel(t, "settings.marketFinance.case.", selected.code)}</p>
                  <p className="break-all font-mono text-xs text-muted-foreground">{selected.objectKind}:{selected.objectId}</p>
                </div> : null}
                <p className="text-xs text-muted-foreground">{t("settings.marketFinance.cases.resolveNote")}</p>
                <OwnerField
                  label={t("settings.marketFinance.owner.reasonCode")}
                  help={t("settings.marketFinance.owner.reasonCodeHelp")}
                  value={reason}
                  onChange={(event) => setReason(event.target.value)}
                  invalid={!!reason && !reasonValid}
                  maxLength={64}
                  placeholder="verified_no_impact"
                  autoFocus
                  inputClassName="font-mono"
                />
                {dialogError ? <Alert status="danger" role="alert">
                  <div className="grid gap-2">
                    <span>{dialogError.text}</span>
                    {dialogError.topup ? <Button size="sm" variant="outline" className="min-h-11 justify-self-start" onPress={goToRecovery}>{t("settings.marketFinance.cases.openRecovery")}</Button> : null}
                  </div>
                </Alert> : null}
              </Modal.Body>
              <Modal.Footer>
                <Button type="button" variant="outline" isDisabled={busy} onPress={() => setSelected(null)}>{t("common.cancel")}</Button>
                <Button type="submit" isDisabled={busy || !reasonValid}>
                  {busy ? <Loader2 className="size-4 animate-spin" aria-hidden="true" /> : null}
                  {t("settings.marketFinance.cases.resolve")}
                </Button>
              </Modal.Footer>
            </form>
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </section>
  );
}
