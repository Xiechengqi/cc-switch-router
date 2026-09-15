"use client";

import { History, Loader2, Pencil, Plus, RefreshCw, RotateCcw, Search } from "lucide-react";
import { Alert, Button, Card, Chip, Input, Modal } from "@heroui/react";
import * as React from "react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  getAdminModelPriceHistory,
  getAdminModelPrices,
  restoreAdminModelPrice,
  upsertAdminModelPrice,
} from "@/lib/api";
import type {
  AdminModelPriceHistoryItem,
  AdminModelPriceList,
  AdminModelPriceListItem,
  AdminModelPriceRates,
} from "@/lib/types";

const PAGE_SIZE = 25;
const EMPTY_DRAFT = { input: "", output: "", cacheRead: "", cacheWrite5m: "", cacheWrite1h: "" };

function decimalFromMicros(value?: number) {
  if (value == null) return "";
  return (value / 1_000_000).toFixed(6).replace(/\.?0+$/, "");
}

function draftFromRates(rates?: AdminModelPriceRates) {
  return {
    input: decimalFromMicros(rates?.input),
    output: decimalFromMicros(rates?.output),
    cacheRead: decimalFromMicros(rates?.cacheRead),
    cacheWrite5m: decimalFromMicros(rates?.cacheWrite5m),
    cacheWrite1h: decimalFromMicros(rates?.cacheWrite1h),
  };
}

function formatRate(value: number | undefined, locale: string) {
  if (value == null) return "—";
  return new Intl.NumberFormat(locale, { maximumFractionDigits: 6 }).format(value / 1_000_000);
}

export function ModelPricesPanel() {
  const { locale, t } = useLocaleText();
  const [data, setData] = React.useState<AdminModelPriceList | null>(null);
  const [loading, setLoading] = React.useState(true);
  const [error, setError] = React.useState("");
  const [query, setQuery] = React.useState("");
  const [filter, setFilter] = React.useState("all");
  const [page, setPage] = React.useState(0);
  const [editing, setEditing] = React.useState<AdminModelPriceListItem | null>(null);
  const [draft, setDraft] = React.useState(EMPTY_DRAFT);
  const [displayName, setDisplayName] = React.useState("");
  const [modelKey, setModelKey] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [dialogError, setDialogError] = React.useState("");
  const [historyModel, setHistoryModel] = React.useState<AdminModelPriceListItem | null>(null);
  const [history, setHistory] = React.useState<AdminModelPriceHistoryItem[]>([]);
  const [historyLoading, setHistoryLoading] = React.useState(false);

  const load = React.useCallback(async () => {
    setLoading(true);
    setError("");
    try { setData(await getAdminModelPrices()); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setLoading(false); }
  }, []);

  React.useEffect(() => { void load(); }, [load]);
  React.useEffect(() => { setPage(0); }, [query, filter]);

  const filtered = React.useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return (data?.models || []).filter((model) => {
      const matchesQuery = !needle || model.modelKey.includes(needle) || model.displayName.toLocaleLowerCase().includes(needle);
      const matchesFilter = filter === "all"
        || (filter === "unpriced" && !model.priced)
        || (filter === "admin" && model.source === "admin")
        || (filter === "derived" && model.source === "derived");
      return matchesQuery && matchesFilter;
    });
  }, [data, filter, query]);
  const pageCount = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  React.useEffect(() => {
    setPage((current) => Math.min(current, pageCount - 1));
  }, [pageCount]);
  const visible = filtered.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE);

  const openEditor = (model: AdminModelPriceListItem) => {
    setEditing(model);
    setModelKey(model.modelKey);
    setDisplayName(model.displayName || model.modelKey);
    setDraft(draftFromRates(model.ratesMicrosPer1m));
    setDialogError("");
  };

  const save = async () => {
    if (!editing) return;
    setBusy(true); setDialogError("");
    try {
      await upsertAdminModelPrice(modelKey, {
        displayName,
        ...(editing.effectiveFrom != null ? { expectedEffectiveFrom: editing.effectiveFrom } : {}),
        ...(!editing.priced ? { expectUnpriced: true } : {}),
        rates: {
          inputUsdPer1m: draft.input,
          outputUsdPer1m: draft.output,
          cacheReadUsdPer1m: draft.cacheRead,
          cacheWrite5mUsdPer1m: draft.cacheWrite5m,
          ...(draft.cacheWrite1h.trim() ? { cacheWrite1hUsdPer1m: draft.cacheWrite1h } : {}),
        },
      });
      setEditing(null);
      await load();
    } catch (cause) { setDialogError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };

  const openHistory = async (model: AdminModelPriceListItem) => {
    setHistoryModel(model); setHistory([]); setHistoryLoading(true);
    try { setHistory(await getAdminModelPriceHistory(model.modelKey)); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); setHistoryModel(null); }
    finally { setHistoryLoading(false); }
  };

  const restore = async (model: AdminModelPriceListItem) => {
    if (model.source !== "admin" || model.effectiveFrom == null) return;
    if (!window.confirm(t("settings.modelPrices.restoreConfirm", { model: model.modelKey }))) return;
    setBusy(true); setError("");
    try { await restoreAdminModelPrice(model.modelKey, model.effectiveFrom); await load(); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  };

  return (
    <section className="grid gap-5">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div><h2 className="font-display text-2xl">{t("settings.modelPrices.title")}</h2><p className="mt-1 text-sm text-muted-foreground">{t("settings.modelPrices.description")}</p></div>
        <div className="flex gap-2"><Button variant="outline" onClick={() => { const model = { modelKey: "", displayName: "", priced: false, discoveredFrom: [] } satisfies AdminModelPriceListItem; openEditor(model); }}><Plus className="h-4 w-4" />{t("settings.modelPrices.add")}</Button><Button variant="outline" onClick={() => void load()} isDisabled={loading || busy}>{loading ? <Loader2 className="h-4 w-4 animate-spin" /> : <RefreshCw className="h-4 w-4" />}{t("common.reload")}</Button></div>
      </div>
      {error ? <Alert status="danger">{error}</Alert> : null}
      <div className="grid grid-cols-2 overflow-hidden rounded-md border bg-background sm:grid-cols-4">
        {([["total", data?.total], ["priced", data?.priced], ["unpriced", data?.unpriced], ["overrides", data?.adminOverrides]] as const).map(([key, value]) => <div key={key} className="border-r p-4 last:border-r-0"><div className="text-2xl font-semibold tabular-nums">{value ?? "—"}</div><div className="text-xs text-muted-foreground">{t(`settings.modelPrices.stat.${key}`)}</div></div>)}
      </div>
      <div className="grid gap-2 sm:grid-cols-[minmax(0,1fr)_220px]">
        <div className="relative"><Search className="pointer-events-none absolute left-3 top-1/2 z-10 h-4 w-4 -translate-y-1/2 text-muted-foreground" /><Input value={query} onChange={(event) => setQuery(event.target.value)} aria-label={t("settings.modelPrices.search")} placeholder={t("settings.modelPrices.search")} className="pl-9" /></div>
        <CompactSelect value={filter} onChange={setFilter} ariaLabel={t("settings.modelPrices.filter")} options={(["all", "unpriced", "admin", "derived"] as const).map((value) => ({ value, label: t(`settings.modelPrices.filter.${value}`) }))} />
      </div>
      <Card className="overflow-x-auto p-0">
        <table className="w-full min-w-[1050px] text-left text-xs">
          <thead className="border-b bg-muted/40 text-muted-foreground"><tr>{(["model", "status", "input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h", "effective", "actions"] as const).map((key) => <th key={key} className="px-3 py-2 font-medium">{t(`settings.modelPrices.column.${key}`)}</th>)}</tr></thead>
          <tbody>{visible.map((model) => <tr key={model.modelKey} className="border-b last:border-0">
            <td className="max-w-64 px-3 py-2"><div className="font-medium">{model.displayName}</div><div className="break-all font-mono text-[10px] text-muted-foreground">{model.modelKey}</div></td>
            <td className="px-3 py-2"><Chip size="sm" color={!model.priced ? "warning" : model.source === "admin" ? "accent" : "default"}>{t(`settings.modelPrices.source.${model.source || "unpriced"}`)}</Chip></td>
            {(["input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h"] as const).map((key) => <td key={key} className="px-3 py-2 tabular-nums">{formatRate(model.ratesMicrosPer1m?.[key], locale)}</td>)}
            <td className="px-3 py-2 whitespace-nowrap">{model.effectiveFrom == null ? "—" : new Date(model.effectiveFrom * 1000).toLocaleString(locale)}</td>
            <td className="px-3 py-2"><div className="flex gap-1"><Button isIconOnly size="sm" variant="ghost" aria-label={t("common.edit")} onClick={() => openEditor(model)}><Pencil className="h-3.5 w-3.5" /></Button>{model.priced ? <Button isIconOnly size="sm" variant="ghost" aria-label={t("settings.modelPrices.history")} onClick={() => void openHistory(model)}><History className="h-3.5 w-3.5" /></Button> : null}{model.source === "admin" ? <Button isIconOnly size="sm" variant="ghost" aria-label={t("settings.modelPrices.restore")} isDisabled={busy} onClick={() => void restore(model)}><RotateCcw className="h-3.5 w-3.5" /></Button> : null}</div></td>
          </tr>)}</tbody>
        </table>
        {!loading && !visible.length ? <div className="p-10 text-center text-sm text-muted-foreground">{t("settings.modelPrices.empty")}</div> : null}
      </Card>
      <div className="flex items-center justify-between text-xs text-muted-foreground"><span>{t("settings.modelPrices.resultCount", { count: filtered.length })}</span><div className="flex items-center gap-2"><Button size="sm" variant="outline" isDisabled={page === 0} onClick={() => setPage((value) => Math.max(0, value - 1))}>{t("common.previous")}</Button><span>{page + 1} / {pageCount}</span><Button size="sm" variant="outline" isDisabled={page + 1 >= pageCount} onClick={() => setPage((value) => Math.min(pageCount - 1, value + 1))}>{t("common.next")}</Button></div></div>
      <div className="break-all text-[10px] text-muted-foreground">{t("settings.modelPrices.revision")}: {data?.pricingRevision || "—"}</div>

      <Modal.Backdrop isOpen={!!editing} onOpenChange={(open) => !open && !busy && setEditing(null)}><Modal.Container placement="center"><Modal.Dialog className="light w-[min(580px,calc(100vw-2rem))] !bg-white !text-slate-900"><Modal.CloseTrigger /><Modal.Header><Modal.Heading>{t("settings.modelPrices.editorTitle")}</Modal.Heading></Modal.Header><Modal.Body className="grid gap-3"><p className="text-xs text-amber-700">{t("dashboard.userLimit.price.globalHint")}</p><label className="grid gap-1 text-xs"><span>{t("settings.modelPrices.displayName")}</span><Input value={displayName} onChange={(event) => setDisplayName(event.target.value)} /></label><label className="grid gap-1 text-xs"><span>{t("settings.modelPrices.column.model")}</span><Input value={modelKey} disabled={!!editing?.modelKey} onChange={(event) => setModelKey(event.target.value)} /></label>{(["input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h"] as const).map((field) => <label key={field} className="grid gap-1 text-xs"><span>{t(`dashboard.userLimit.price.${field}`)}</span><Input inputMode="decimal" value={draft[field]} placeholder="0" onChange={(event) => setDraft((current) => ({ ...current, [field]: event.target.value }))} /></label>)}{dialogError ? <p className="text-xs text-red-600">{dialogError}</p> : null}</Modal.Body><Modal.Footer><Button variant="outline" isDisabled={busy} onClick={() => setEditing(null)}>{t("common.cancel")}</Button><Button variant="primary" isDisabled={busy || !modelKey.trim() || !displayName.trim() || !draft.input || !draft.output || !draft.cacheRead || !draft.cacheWrite5m} onClick={() => void save()}>{busy ? <Loader2 className="h-4 w-4 animate-spin" /> : null}{t("common.save")}</Button></Modal.Footer></Modal.Dialog></Modal.Container></Modal.Backdrop>

      <Modal.Backdrop isOpen={!!historyModel} onOpenChange={(open) => !open && setHistoryModel(null)}><Modal.Container placement="center"><Modal.Dialog className="light w-[min(780px,calc(100vw-2rem))] !bg-white !text-slate-900"><Modal.CloseTrigger /><Modal.Header><Modal.Heading>{t("settings.modelPrices.historyTitle", { model: historyModel?.modelKey || "" })}</Modal.Heading></Modal.Header><Modal.Body className="max-h-[65vh] overflow-auto">{historyLoading ? <Loader2 className="mx-auto h-5 w-5 animate-spin" /> : <div className="grid gap-2">{history.map((item) => <div key={`${item.source}:${item.effectiveFrom}`} className="rounded-md border p-3 text-xs"><div className="flex flex-wrap justify-between gap-2"><div className="flex gap-2"><Chip size="sm">{t(`settings.modelPrices.source.${item.source}`)}</Chip>{item.current ? <Chip size="sm" color="success">{t("settings.modelPrices.current")}</Chip> : null}</div><span>{new Date(item.effectiveFrom * 1000).toLocaleString(locale)} → {item.effectiveTo ? new Date(item.effectiveTo * 1000).toLocaleString(locale) : "—"}</span></div><div className="mt-2 grid grid-cols-2 gap-2 sm:grid-cols-5">{(["input", "output", "cacheRead", "cacheWrite5m", "cacheWrite1h"] as const).map((key) => <div key={key}><div className="text-muted-foreground">{t(`settings.modelPrices.column.${key}`)}</div><div className="tabular-nums">{formatRate(item.ratesMicrosPer1m[key], locale)}</div></div>)}</div></div>)}</div>}</Modal.Body><Modal.Footer><Button variant="outline" onClick={() => setHistoryModel(null)}>{t("common.close")}</Button></Modal.Footer></Modal.Dialog></Modal.Container></Modal.Backdrop>
    </section>
  );
}
