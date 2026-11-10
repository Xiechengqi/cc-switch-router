"use client";
import * as React from "react";
import { Button } from "@heroui/react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getAdminResourcePrices, updateAdminResourcePrices } from "@/lib/api";
import { RESOURCE_UNITS, parseResourceRate, resourceRateInput, type ResourceDimension, type ResourcePriceState } from "@/lib/resource-prices";
import type { MessageKey } from "@/lib/i18n";

type RateRow = { id: number; model: string; dimension: ResourceDimension; qualifier: string; usd: string };
type AliasRow = { id: number; alias: string; target: string };
const inputClass = "min-w-0 rounded-md border bg-background px-2 py-1.5 text-sm";
export function ResourcePricesEditor() {
  const { t } = useLocaleText();
  const [state, setState] = React.useState<ResourcePriceState>();
  const [rates, setRates] = React.useState<RateRow[]>([]);
  const [aliases, setAliases] = React.useState<AliasRow[]>([]);
  const [error, setError] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [dirty, setDirty] = React.useState(false);
  const nextId = React.useRef(1);
  const apply = React.useCallback((data: ResourcePriceState) => {
    const rows = Object.entries(data.settings.models).flatMap(([model, values]) => Object.entries(values).map(([key, picos]) => {
      const [dimension, ...suffix] = key.split("/");
      if (!(dimension in RESOURCE_UNITS)) throw new Error("unsupported resource dimension");
      return { id: nextId.current++, model, dimension: dimension as ResourceDimension, qualifier: suffix.join("/"),
        usd: resourceRateInput(picos, RESOURCE_UNITS[dimension as ResourceDimension]) };
    }));
    setState(data); setRates(rows);
    setAliases(Object.entries(data.settings.exactAliases).map(([alias, target]) => ({ id: nextId.current++, alias, target })));
    setDirty(false);
  }, []);
  const reload = React.useCallback(async () => {
    setBusy(true); setError("");
    try { apply(await getAdminResourcePrices()); }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)); }
    finally { setBusy(false); }
  }, [apply]);
  React.useEffect(() => { void reload(); }, [reload]);
  const editRate = (id: number, patch: Partial<RateRow>) => { setDirty(true); setRates(rows => rows.map(row => row.id === id ? { ...row, ...patch } : row)); };
  const editAlias = (id: number, patch: Partial<AliasRow>) => { setDirty(true); setAliases(rows => rows.map(row => row.id === id ? { ...row, ...patch } : row)); };
  const save = async () => {
    if (!state) return;
    const settings: ResourcePriceState["settings"] = { models: Object.create(null), exactAliases: Object.create(null) };
    for (const row of rates) {
      const model = row.model.trim(), qualifier = row.qualifier.trim();
      const picos = parseResourceRate(row.usd, RESOURCE_UNITS[row.dimension]);
      if (!model || picos === undefined || qualifier && !/^[a-z0-9._/-]+$/.test(qualifier)) { setError(t("settings.resources.invalid")); return; }
      const key = row.dimension + (qualifier ? "/" + qualifier : "");
      settings.models[model] ??= Object.create(null);
      if (Object.hasOwn(settings.models[model], key)) { setError(t("settings.resources.duplicate")); return; }
      settings.models[model][key] = picos;
    }
    for (const row of aliases) {
      const alias = row.alias.trim(), target = row.target.trim();
      if (!alias || !target || Object.hasOwn(settings.exactAliases, alias)) { setError(t("settings.resources.duplicate")); return; }
      settings.exactAliases[alias] = target;
    }
    setBusy(true); setError("");
    try { apply(await updateAdminResourcePrices(state.revision, settings)); }
    catch (cause) { setError((cause instanceof Error ? cause.message : String(cause)) + " " + t("settings.resources.conflictHint")); }
    finally { setBusy(false); }
  };
  return <div className="grid gap-3 border-t pt-5">
    <div><h3 className="font-medium">{t("settings.resources.title")}</h3><p className="mt-1 text-sm text-muted-foreground">{t("settings.resources.hint")}</p></div>
    <div className="overflow-x-auto"><table className="w-full min-w-[740px] text-left text-xs">
      <thead><tr>{["model", "dimension", "variant", "price"].map(key => <th className="pb-2 pr-2" key={key}>{t(("settings.resources." + key) as MessageKey)}</th>)}<th /></tr></thead>
      <tbody>{rates.map(row => <tr key={row.id}>
        <td className="pb-2 pr-2"><input aria-label={t("settings.resources.model")} className={inputClass} value={row.model} disabled={busy} onChange={e => editRate(row.id, { model: e.target.value })} /></td>
        <td className="pb-2 pr-2"><select aria-label={t("settings.resources.dimension")} className={inputClass} value={row.dimension} disabled={busy} onChange={e => editRate(row.id, { dimension: e.target.value as ResourceDimension, usd: "" })}>
          {(Object.keys(RESOURCE_UNITS) as ResourceDimension[]).map(key => <option key={key} value={key}>{t(("settings.resources.kind." + key) as MessageKey)}</option>)}
        </select></td>
        <td className="pb-2 pr-2"><input aria-label={t("settings.resources.variant")} className={inputClass} value={row.qualifier} disabled={busy} placeholder="1024x1024/high" onChange={e => editRate(row.id, { qualifier: e.target.value })} /></td>
        <td className="pb-2 pr-2"><input aria-label={t("settings.resources.price")} className={inputClass + " w-32"} value={row.usd} disabled={busy} inputMode="decimal" placeholder="0.00" onChange={e => editRate(row.id, { usd: e.target.value })} />
          <span className="ml-1 text-muted-foreground">{t(RESOURCE_UNITS[row.dimension] === 1000000n ? "settings.resources.perMillion" : RESOURCE_UNITS[row.dimension] === 1000n ? "settings.resources.perSecond" : "settings.resources.perItem")}</span></td>
        <td className="pb-2"><Button size="sm" variant="ghost" isDisabled={busy} onClick={() => { setRates(rows => rows.filter(r => r.id !== row.id)); setDirty(true); }}>{t("common.delete")}</Button></td>
      </tr>)}</tbody>
    </table></div>
    <Button size="sm" variant="outline" className="justify-self-start" isDisabled={busy || !state} onClick={() => { setRates(rows => [...rows, { id: nextId.current++, model: "", dimension: "image.output", qualifier: "", usd: "" }]); setDirty(true); }}>{t("settings.resources.add")}</Button>
    <div><h4 className="text-sm font-medium">{t("settings.resources.aliasTitle")}</h4><p className="text-xs text-muted-foreground">{t("settings.resources.aliasHint")}</p></div>
    {aliases.map(row => <div key={row.id} className="grid gap-2 sm:grid-cols-[1fr_1fr_auto]">
      <input className={inputClass} aria-label={t("settings.resources.alias")} value={row.alias} disabled={busy} onChange={e => editAlias(row.id, { alias: e.target.value })} />
      <input className={inputClass} aria-label={t("settings.resources.target")} value={row.target} disabled={busy} onChange={e => editAlias(row.id, { target: e.target.value })} />
      <Button size="sm" variant="ghost" isDisabled={busy} onClick={() => { setAliases(rows => rows.filter(r => r.id !== row.id)); setDirty(true); }}>{t("common.delete")}</Button>
    </div>)}
    <Button size="sm" variant="outline" className="justify-self-start" isDisabled={busy || !state} onClick={() => { setAliases(rows => [...rows, { id: nextId.current++, alias: "", target: "" }]); setDirty(true); }}>{t("settings.resources.addAlias")}</Button>
    <p className="break-all text-xs text-muted-foreground">{t("settings.resources.publication", { revision: state?.pricingRevision ?? "—" })}</p>
    {error ? <p role="alert" className="text-sm text-red-600">{error}</p> : null}
    <div className="flex gap-2">
      <Button variant="primary" isDisabled={busy || !dirty || !state} onClick={() => void save()}>{t("common.save")}</Button>
      <Button variant="outline" isDisabled={busy} onClick={() => void reload()}>{t(dirty ? "settings.resources.discardReload" : "common.reload")}</Button>
    </div>
  </div>;
}
