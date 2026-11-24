"use client";

import * as React from "react";
import { Button, Checkbox } from "@heroui/react";
import { SlidersHorizontal } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { HOST_SORT_COLUMN_LABELS, HOST_SORT_KEYS, type HostSortPrefs } from "./host-utils";

export function HostHealthFilters({ onlyHealthy, arch, cpu, memory, sort, onChange, onSort }: {
  onlyHealthy: boolean; arch: string; cpu: string; memory: string; sort: HostSortPrefs;
  onChange: (value: { onlyHealthy: boolean; arch: string; cpu: string; memory: string }) => void;
  onSort: (value: HostSortPrefs) => void;
}) {
  const { t } = useLocaleText();
  const panel = React.useRef<HTMLDetailsElement>(null);
  const update = (value: Partial<{ onlyHealthy: boolean; arch: string; cpu: string; memory: string }>) => onChange({ onlyHealthy, arch, cpu, memory, ...value });
  const count = [arch, cpu, memory].filter(Boolean).length;
  const choices = HOST_SORT_KEYS.flatMap(key => (["asc", "desc"] as const).map(dir => ({
    value: `${key}:${dir}`, label: `${t(HOST_SORT_COLUMN_LABELS[key])} · ${t(dir === "asc" ? "clientMarket.sortAsc" : "clientMarket.sortDesc")}`, key, dir,
  })));
  return <div className="host-health-toolbar">
    <Checkbox isSelected={onlyHealthy} onChange={value => update({ onlyHealthy: value })} className="host-health-only-normal">
      <Checkbox.Control className="!h-4 !w-4 !rounded border border-muted-foreground/70 bg-card shadow-none"><Checkbox.Indicator /></Checkbox.Control><span>{t("hostHealth.onlyHealthy")}</span>
    </Checkbox>
    <div className="host-health-toolbar-actions">
      <CompactSelect value={sort.key ? `${sort.key}:${sort.dir}` : ""} options={[{ value: "", label: t("hostHealth.defaultSort") }, ...choices]}
        ariaLabel={t("hostHealth.sortBy")} triggerClassName="!min-h-11 !shadow-none" className="host-health-sort" onChange={value => {
          const choice = choices.find(item => item.value === value);
          onSort(choice ? { key: choice.key, dir: choice.dir } : { key: null, dir: "asc" });
        }} />
      <details className="host-health-filter-menu" ref={panel} onKeyDown={event => { if (event.key === "Escape" && !event.defaultPrevented && panel.current) panel.current.open = false; }}>
        <summary className="host-health-filter-trigger"><SlidersHorizontal className="h-4 w-4" aria-hidden="true" /><span>{t("hostHealth.filters")}</span>{count ? <span className="host-health-filter-count">{count}</span> : null}</summary>
        <div className="host-health-filter-panel" aria-label={t("hostHealth.filterTitle")}>
          <label className="host-health-filter-field"><span>{t("hostHealth.arch")}</span>
            <CompactSelect value={arch} options={[{ value: "", label: t("hostHealth.allArch") }, { value: "aarch64", label: "ARM64" }, { value: "x86_64", label: "x86_64" }]} onChange={value => update({ arch: value })} ariaLabel={t("hostHealth.arch")} triggerClassName="!min-h-11 !shadow-none" />
          </label>
          <div className="host-health-capacity-fields">
            <label className="host-health-filter-field"><span>{t("hostHealth.minCpu")}</span><input type="number" min="0" step="0.5" className="host-health-input" value={cpu} onChange={event => update({ cpu: event.target.value })} /></label>
            <label className="host-health-filter-field"><span>{t("hostHealth.minMemory")}</span><input type="number" min="0" step="0.5" className="host-health-input" value={memory} onChange={event => update({ memory: event.target.value })} /></label>
          </div>
          <div className="flex items-center justify-between gap-3 border-t border-border pt-3">
            <Button variant="ghost" className="min-h-11" onPress={() => onChange({ onlyHealthy: false, arch: "", cpu: "", memory: "" })}>{t("dashboard.clearFilters")}</Button>
            <Button variant="primary" className="min-h-11" onPress={() => { if (panel.current) panel.current.open = false; }}>{t("hostHealth.closeFilters")}</Button>
          </div>
        </div>
      </details>
    </div>
    {count ? <span className="host-health-active-filters">{t("hostHealth.appliedFilters", { count })}</span> : null}
  </div>;
}
