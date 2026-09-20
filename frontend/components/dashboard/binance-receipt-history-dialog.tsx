"use client";

import * as React from "react";
import { Button, Chip, Modal } from "@heroui/react";
import { ChevronDown, Loader2, ReceiptText } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getBinanceReceiptHistory } from "@/lib/api";
import {
  binanceReceiptBuyerEmail,
  binanceReceiptStatus,
} from "@/lib/binance-auto-settlement";
import { formatUsdMoney } from "@/lib/market-money";
import type { BinanceReceiptHistoryEntry } from "@/lib/types";

function receiptSource(source: string, t: ReturnType<typeof useLocaleText>["t"]) {
  return source === "admin_reconciliation"
    ? t("account.binanceAuto.receipts.sourceAdmin")
    : t("account.binanceAuto.receipts.sourceAuto");
}

export function BinanceReceiptHistoryDialog({
  open,
  onOpenChange,
  actorKey,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  actorKey: string;
}) {
  const { t, locale } = useLocaleText();
  const [items, setItems] = React.useState<BinanceReceiptHistoryEntry[]>([]);
  const [cursor, setCursor] = React.useState<string>();
  const [loading, setLoading] = React.useState(false);
  const [error, setError] = React.useState("");

  const load = React.useCallback(async (next?: string, signal?: AbortSignal) => {
    setLoading(true);
    setError("");
    try {
      const page = await getBinanceReceiptHistory(next, signal);
      setItems((current) => next ? [...current, ...page.items] : page.items);
      setCursor(page.nextCursor);
    } catch (cause) {
      if (!signal?.aborted) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (!signal?.aborted) setLoading(false);
    }
  }, []);

  React.useEffect(() => {
    setItems([]);
    setCursor(undefined);
    setError("");
    if (!open) return;
    const controller = new AbortController();
    void load(undefined, controller.signal);
    return () => controller.abort();
  }, [open, actorKey, load]);

  const date = (value: string) => new Date(value).toLocaleString(locale);

  return (
    <Modal.Backdrop isOpen={open} onOpenChange={onOpenChange}>
      <Modal.Container placement="center">
        <Modal.Dialog className="light flex max-h-[min(90vh,calc(100vh-1.5rem))] w-[min(860px,calc(100vw-1.5rem))] max-w-none flex-col overflow-hidden !bg-white !text-slate-900">
          <Modal.CloseTrigger />
          <Modal.Header>
            <Modal.Heading>{t("account.binanceAuto.receipts.title")}</Modal.Heading>
          </Modal.Header>
          <Modal.Body className="min-h-0 overflow-y-auto">
            <p className="mb-4 text-xs leading-5 text-muted-foreground">{t("account.binanceAuto.receipts.scope")}</p>
            {error ? <p className="rounded-md border border-rose-200 bg-rose-50 p-3 text-sm text-rose-800">{error}</p> : null}
            {!items.length && loading ? (
              <div className="flex justify-center py-12"><Loader2 className="h-5 w-5 animate-spin" /></div>
            ) : !items.length ? (
              <div className="rounded-lg border border-dashed py-12 text-center text-sm text-muted-foreground">
                <ReceiptText className="mx-auto mb-2 h-5 w-5" />
                {t("account.binanceAuto.receipts.empty")}
              </div>
            ) : (
              <div className="grid gap-3">
                {items.map((item) => (
                  <details key={item.receiptId} className="group rounded-lg border border-border bg-white">
                    <summary className="flex cursor-pointer list-none flex-wrap items-center justify-between gap-3 p-4">
                      <div className="flex min-w-0 items-center gap-2">
                        <ChevronDown className="h-4 w-4 shrink-0 -rotate-90 transition-transform group-open:rotate-0" />
                        <div>
                          <strong className="block text-sm">{item.actualAmount} {item.asset}</strong>
                          <span className="text-xs text-muted-foreground">{date(item.transactionAt)}</span>
                        </div>
                      </div>
                      <div className="text-right text-xs">
                        <div className="flex flex-wrap items-center justify-end gap-2">
                          <Chip size="sm" variant="soft">
                            {t(item.kind === "invoice_payment"
                              ? "account.binanceAuto.receipts.kindInvoice"
                              : "account.binanceAuto.receipts.kindTopup")}
                          </Chip>
                          <strong>
                            {item.kind === "invoice_payment"
                              ? `#${item.invoice.sequence}`
                              : formatUsdMoney(item.topup.creditedAmountMinor, locale)}
                          </strong>
                          <Chip size="sm" variant="soft">{receiptSource(item.source, t)}</Chip>
                        </div>
                        <span className="mt-1 block text-muted-foreground">{binanceReceiptBuyerEmail(item)}</span>
                      </div>
                    </summary>
                    <div className="grid gap-3 border-t p-4 text-xs">
                      <div className="grid gap-1 text-muted-foreground sm:grid-cols-2">
                        <span>{t("account.binanceAuto.receipts.confirmedAt")}: {date(item.confirmedAt)}</span>
                        <span>
                          {t(item.kind === "invoice_payment"
                            ? "account.binanceAuto.receipts.invoiceStatus"
                            : "account.binanceAuto.receipts.topupStatus")}: {binanceReceiptStatus(item)}
                        </span>
                        <span>{t("account.binanceAuto.receipts.expected")}: {item.expectedAmount} {item.asset}</span>
                        <span>{t("account.binanceAuto.receipts.transactionId")}: <span className="break-all font-mono">{item.transactionId}</span></span>
                        {item.orderId ? <span>{t("account.binanceAuto.receipts.orderId")}: <span className="break-all font-mono">{item.orderId}</span></span> : null}
                        {item.kind === "prepaid_topup" ? (
                          <>
                            <span>
                              {t("account.binanceAuto.receipts.creditedAmount")}: {formatUsdMoney(item.topup.creditedAmountMinor, locale)}
                            </span>
                            <span>
                              {t("account.binanceAuto.receipts.creditedAt")}: {date(item.topup.creditedAt)}
                            </span>
                            <span>
                              {t("account.binanceAuto.receipts.fundingIntent")}: <span className="break-all font-mono">{item.fundingIntentId}</span>
                            </span>
                          </>
                        ) : null}
                      </div>
                      {item.kind === "invoice_payment" && item.invoice.lines.length ? (
                        <div className="grid gap-2">
                          {item.invoice.lines.map((line, index) => (
                            <div key={`${line.serviceLabel}-${index}`} className="flex flex-wrap justify-between gap-2 rounded-md bg-slate-50 p-2">
                              <span>{line.serviceLabel}</span>
                              <span className="text-muted-foreground">{date(line.serviceStartedAt)} – {date(line.serviceEndedAt)}</span>
                            </div>
                          ))}
                        </div>
                      ) : null}
                    </div>
                  </details>
                ))}
                {cursor ? (
                  <Button variant="ghost" isDisabled={loading} onClick={() => void load(cursor)}>
                    {loading ? <Loader2 className="h-4 w-4 animate-spin" /> : null}
                    {t("account.binanceAuto.receipts.loadMore")}
                  </Button>
                ) : null}
              </div>
            )}
          </Modal.Body>
          <Modal.Footer><Button variant="outline" onClick={() => onOpenChange(false)}>{t("common.close")}</Button></Modal.Footer>
        </Modal.Dialog>
      </Modal.Container>
    </Modal.Backdrop>
  );
}
