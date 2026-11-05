"use client";

import * as React from "react";
import { DndContext, KeyboardSensor, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, sortableKeyboardCoordinates, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, ChevronDown, ChevronUp, GripVertical } from "lucide-react";
import { RoutingShareDetails, RoutingShareSummary, type RoutingShareSummaryProps } from "./model-hub-share-summary";
import { MAX_MODEL_ROUTE_TARGETS, modelRouteDragAnnouncements, modelRouteTransferSelection, moveModelRouteTarget, sharesForProtocol, transferModelRouteTargets } from "@/lib/model-routing";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import { cn } from "@/lib/utils";

// Hallmark · component: Transfer · existing Router tokens · keyboard/native checkboxes
// Pre-emit critique: P4 H4 E4 S5 R5 V4. No page-level redesign or added animation.
const controlClass = "inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40";
const selectionClass = "flex min-h-11 min-w-0 cursor-pointer items-start gap-2 rounded-md p-2 hover:bg-secondary active:bg-secondary has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-ring has-[:disabled]:cursor-not-allowed";

function TransferCheckbox({ checked, mixed = false, disabled, label, onChange, inputRef }: {
  checked: boolean; mixed?: boolean; disabled: boolean; label: string;
  onChange: (checked: boolean) => void; inputRef?: (node: HTMLInputElement | null) => void;
}) {
  return <input type="checkbox" checked={checked} disabled={disabled} aria-label={label}
    aria-checked={mixed ? "mixed" : checked}
    ref={node => { if (node) node.indeterminate = mixed; inputRef?.(node); }}
    className="mt-0.5 size-4 shrink-0 accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed"
    onChange={event => onChange(event.target.checked)} />;
}

function TargetRow({ id, index, total, disabled, selectionDisabled, sortDisabled, checked, onCheck, onMove, inputRef, ...summary }: RoutingShareSummaryProps & {
  id: string; index: number; total: number; disabled: boolean;
  selectionDisabled: boolean; sortDisabled: boolean;
  checked: boolean; onCheck: (checked: boolean) => void;
  inputRef: (node: HTMLInputElement | null) => void;
  onMove: (from: number, to: number) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id, disabled: disabled || sortDisabled });
  return (
    <li ref={setNodeRef} style={{ transform: CSS.Transform.toString(transform), transition }}
      className={cn("model-route-sortable grid min-w-0 gap-1 rounded-md", checked && "bg-secondary", isDragging && "relative z-10 bg-card outline-2 outline-primary")}>
      <div className="flex min-w-0 items-center">
        <button {...attributes} {...listeners} type="button" disabled={disabled || sortDisabled}
          aria-label={summary.t("modelHub.dragCandidate")} className={cn(controlClass, "touch-none cursor-grab active:cursor-grabbing")}>
          <GripVertical className="h-4 w-4" aria-hidden />
        </button>
        <span className="min-w-0 flex-1 truncate text-xs font-medium tabular-nums">
          {index === 0 ? summary.t("modelHub.candidatePrimary") : summary.t("modelHub.candidateOrder", { order: index + 1 })}
        </span>
        <button type="button" disabled={selectionDisabled || sortDisabled || index === 0} onClick={() => onMove(index, index - 1)}
          className={controlClass} aria-label={summary.t("modelHub.moveUp")}><ChevronUp className="h-4 w-4" aria-hidden /></button>
        <button type="button" disabled={selectionDisabled || sortDisabled || index === total - 1} onClick={() => onMove(index, index + 1)}
          className={controlClass} aria-label={summary.t("modelHub.moveDown")}><ChevronDown className="h-4 w-4" aria-hidden /></button>
      </div>
      <label className={selectionClass}>
        <TransferCheckbox checked={checked} disabled={selectionDisabled} onChange={onCheck} inputRef={inputRef}
          label={summary.t("modelHub.checkShare", { name: summary.share?.subdomain || summary.shareId })} />
        <RoutingShareSummary {...summary} />
      </label>
      <div className="min-w-0 px-2"><RoutingShareDetails {...summary} /></div>
    </li>
  );
}

export function SharePoolPicker({ shares, selected, app, routeKey, disabled, addBlockedReason, onChange, locale, t }: {
  shares: UserModelRoutingShare[]; selected: string[]; app: ModelRoutingApp; routeKey: string;
  disabled: boolean; addBlockedReason?: string; onChange: (ids: string[]) => void;
} & Pick<RoutingShareSummaryProps, "locale" | "t">) {
  const [leftQuery, setLeftQuery] = React.useState("");
  const [rightQuery, setRightQuery] = React.useState("");
  const [leftChecked, setLeftChecked] = React.useState<string[]>([]);
  const [rightChecked, setRightChecked] = React.useState<string[]>([]);
  const [announcement, setAnnouncement] = React.useState("");
  const [dragging, setDragging] = React.useState(false);
  const inputs = React.useRef(new Map<string, HTMLInputElement>());
  const leftSearchRef = React.useRef<HTMLInputElement>(null);
  const rightSearchRef = React.useRef<HTMLInputElement>(null);
  const pendingFocus = React.useRef<{ side: "left" | "right"; id: string } | null>(null);
  React.useLayoutEffect(() => {
    const pending = pendingFocus.current;
    if (!pending) return;
    const moved = inputs.current.get(`${pending.side}:${pending.id}`);
    if (moved) {
      moved.focus();
      pendingFocus.current = null;
      return;
    }
    // The moved row's checkbox commits after this effect. Retry once on the
    // next frame before falling back to a search box that may be unmounted.
    const frame = requestAnimationFrame(() => {
      const node = inputs.current.get(`${pending.side}:${pending.id}`)
        || (pending.side === "left" ? leftSearchRef.current : rightSearchRef.current);
      node?.focus();
      if (pendingFocus.current === pending) pendingFocus.current = null;
    });
    return () => cancelAnimationFrame(frame);
  }, [selected]);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }));
  const eligible = sharesForProtocol(shares, app);
  const byId = new Map(eligible.map(share => [share.shareId, share]));
  const remaining = eligible.filter(share => !selected.includes(share.shareId));
  const matches = (id: string, query: string) => {
    const share = byId.get(id);
    return [id, share?.subdomain, share?.shareName, share?.appCapabilities?.find(cap => cap.app === app)?.providerName]
      .filter(Boolean).join(" ").toLocaleLowerCase().includes(query.trim().toLocaleLowerCase());
  };
  const available = remaining.filter(share => matches(share.shareId, leftQuery));
  const availableIds = available.map(share => share.shareId);
  const visibleSelected = selected.filter(id => matches(id, rightQuery));
  const checkedIn = modelRouteTransferSelection(availableIds, leftChecked);
  const checkedOut = modelRouteTransferSelection(visibleSelected, rightChecked);
  // Drop disappeared selections so a later metadata refresh cannot silently
  // re-select a revoked Share. Filtering also resets only that side's checks.
  const leftMembership = JSON.stringify(availableIds);
  const rightMembership = JSON.stringify(visibleSelected);
  React.useEffect(() => {
    setLeftChecked(current => {
      const next = modelRouteTransferSelection(JSON.parse(leftMembership), current);
      return JSON.stringify(next) === JSON.stringify(current) ? current : next;
    });
    setRightChecked(current => {
      const next = modelRouteTransferSelection(JSON.parse(rightMembership), current);
      return JSON.stringify(next) === JSON.stringify(current) ? current : next;
    });
  }, [leftMembership, rightMembership]);
  const searchable = eligible.length > MAX_MODEL_ROUTE_TARGETS;
  const full = selected.length >= MAX_MODEL_ROUTE_TARGETS;
  const remainingSlots = Math.max(0, MAX_MODEL_ROUTE_TARGETS - selected.length);
  const overCapacity = checkedIn.length > remainingSlots;
  const statusText = addBlockedReason
    || (overCapacity ? t("modelHub.transferCapacity", { count: checkedIn.length, remaining: remainingSlots }) : "")
    || (full ? t("modelHub.queueFull") : "")
    || (rightQuery.trim() ? t("modelHub.sortSearchHint") : "");
  const locked = disabled || dragging;
  const setChecked = (side: "left" | "right", id: string, checked: boolean) => {
    if (locked) return;
    (side === "left" ? setLeftChecked : setRightChecked)(current => checked ? [...new Set([...current, id])] : current.filter(value => value !== id));
  };
  const moveChecked = (direction: "in" | "out") => {
    if (locked || (direction === "in" && (!!addBlockedReason || overCapacity))) return;
    const checked = direction === "in" ? checkedIn : checkedOut;
    const next = transferModelRouteTargets(selected, availableIds, checked, direction);
    if (next === selected) return;
    const side = direction === "in" ? "right" : "left";
    pendingFocus.current = { side, id: checked[0] };
    if (direction === "in") { setLeftChecked([]); setRightQuery(""); setRightChecked(checked); }
    else { setRightChecked([]); setLeftQuery(""); setLeftChecked(checked); }
    onChange(next);
    setAnnouncement(t(direction === "in" ? "modelHub.transferredIn" : "modelHub.transferredOut", { count: checked.length }));
  };
  const ids = selected.map(id => JSON.stringify([routeKey, id]));
  const move = (from: number, to: number) => {
    if (disabled || rightQuery.trim()) return;
    const next = moveModelRouteTarget(selected, from, to);
    if (next !== selected) {
      onChange(next);
      setAnnouncement(t("modelHub.moved", { name: byId.get(selected[from])?.subdomain || selected[from], order: to + 1 }));
    }
  };
  const dragEnd = ({ active, over }: DragEndEvent) => {
    setDragging(false);
    if (over) move(ids.indexOf(String(active.id)), ids.indexOf(String(over.id)));
  };

  return (
    <div className="model-route-transfer grid min-w-0 gap-2" data-model-route-picker>
      <div className="model-route-transfer-grid">
      <section className="model-route-transfer-pane" aria-label={t("modelHub.availableShares")}>
        <div className="grid gap-2 border-b border-border p-2">
          <h5 className="px-2 text-xs font-semibold text-foreground">{t("modelHub.availableShares")} · {remaining.length}</h5>
          <label className={cn(selectionClass, "items-center text-xs text-muted-foreground")}>
            <TransferCheckbox checked={!!availableIds.length && checkedIn.length === availableIds.length} mixed={checkedIn.length > 0 && checkedIn.length < availableIds.length}
              disabled={locked || !availableIds.length} label={t("modelHub.checkVisible", { list: t("modelHub.availableShares") })}
              onChange={checked => { if (!locked) setLeftChecked(checked ? availableIds : []); }} />
            {t("modelHub.checkedCount", { count: checkedIn.length })}
          </label>
          {searchable || leftQuery ? (
            <input ref={leftSearchRef} value={leftQuery} onChange={event => { setLeftQuery(event.target.value); setLeftChecked([]); }} type="search"
              className="h-11 min-w-0 w-full rounded-md border border-input bg-card px-3 text-xs focus-visible:outline-2 focus-visible:outline-ring"
              aria-label={t("modelHub.searchList", { list: t("modelHub.availableShares") })} placeholder={t("modelHub.searchShares")} />
          ) : null}
        </div>
        <ul className="model-route-transfer-list" aria-label={t("modelHub.availableShares")}>
          {available.map(share => (
            <li key={share.shareId} className={cn("grid min-w-0 gap-1 rounded-md", checkedIn.includes(share.shareId) && "bg-secondary")}>
              <label className={selectionClass}>
                <TransferCheckbox checked={checkedIn.includes(share.shareId)} disabled={locked}
                  label={t("modelHub.checkShare", { name: share.subdomain || share.shareName })}
                  inputRef={node => { const key = `left:${share.shareId}`; if (node) inputs.current.set(key, node); else inputs.current.delete(key); }}
                  onChange={checked => setChecked("left", share.shareId, checked)} />
                <RoutingShareSummary share={share} shareId={share.shareId} app={app} locale={locale} t={t} />
              </label>
              <div className="min-w-0 px-2"><RoutingShareDetails share={share} shareId={share.shareId} app={app} locale={locale} t={t} /></div>
            </li>
          ))}
          {!available.length ? <li className="px-2 py-4 text-xs text-muted-foreground">{t(!eligible.length ? "modelHub.noEligibleShares" : !remaining.length ? "modelHub.allSharesSelected" : "modelHub.noSearchResults")}</li> : null}
        </ul>
      </section>
      <div className="model-route-transfer-actions" role="group" aria-label={t("modelHub.transferHint")}>
        <button type="button" disabled={locked || !checkedIn.length || overCapacity || !!addBlockedReason}
          className="inline-flex h-11 items-center justify-center gap-1 whitespace-nowrap rounded-md border border-input bg-card px-3 text-xs text-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
          onClick={() => moveChecked("in")} aria-label={t("modelHub.transferInLabel", { count: checkedIn.length })}>
          <ArrowRight className="model-route-transfer-horizontal size-4" aria-hidden /><ArrowDown className="model-route-transfer-vertical size-4" aria-hidden />{t("modelHub.transferIn")}
        </button>
        <button type="button" disabled={locked || !checkedOut.length}
          className="inline-flex h-11 items-center justify-center gap-1 whitespace-nowrap rounded-md border border-input bg-card px-3 text-xs text-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
          onClick={() => moveChecked("out")} aria-label={t("modelHub.transferOutLabel", { count: checkedOut.length })}>
          <ArrowLeft className="model-route-transfer-horizontal size-4" aria-hidden /><ArrowUp className="model-route-transfer-vertical size-4" aria-hidden />{t("modelHub.transferOut")}
        </button>
      </div>
      <section className="model-route-transfer-pane" aria-label={t("modelHub.selectedShares")}>
        <div className="grid gap-2 border-b border-border p-2">
          <h5 className="px-2 text-xs font-semibold text-foreground">{t("modelHub.selectedShares")} · {selected.length}/{MAX_MODEL_ROUTE_TARGETS}</h5>
          <label className={cn(selectionClass, "items-center text-xs text-muted-foreground")}>
            <TransferCheckbox checked={!!visibleSelected.length && checkedOut.length === visibleSelected.length} mixed={checkedOut.length > 0 && checkedOut.length < visibleSelected.length}
              disabled={locked || !visibleSelected.length} label={t("modelHub.checkVisible", { list: t("modelHub.selectedShares") })}
              onChange={checked => { if (!locked) setRightChecked(checked ? visibleSelected : []); }} />
            {t("modelHub.checkedCount", { count: checkedOut.length })}
          </label>
          {searchable || rightQuery ? (
            <input ref={rightSearchRef} value={rightQuery} onChange={event => { setRightQuery(event.target.value); setRightChecked([]); }} type="search"
              className="h-11 min-w-0 w-full rounded-md border border-input bg-card px-3 text-xs focus-visible:outline-2 focus-visible:outline-ring"
              aria-label={t("modelHub.searchList", { list: t("modelHub.selectedShares") })} placeholder={t("modelHub.searchShares")} />
          ) : null}
        </div>
        <DndContext sensors={sensors} collisionDetection={closestCenter}
          onDragStart={() => setDragging(true)} onDragCancel={() => setDragging(false)} onDragEnd={dragEnd}
          accessibility={{
            screenReaderInstructions: { draggable: t("modelHub.dragInstructions") },
            announcements: modelRouteDragAnnouncements(ids, selected.map(id => byId.get(id)?.subdomain || id), 1, t)
          }}>
          <SortableContext items={ids} strategy={verticalListSortingStrategy}>
            <ol className="model-route-transfer-list" aria-label={t("modelHub.selectedShares")}>
              {visibleSelected.map(shareId => { const index = selected.indexOf(shareId); return <TargetRow key={ids[index]} id={ids[index]} index={index} total={selected.length}
                share={byId.get(shareId)} shareId={shareId} app={app} locale={locale} t={t}
                inputRef={node => { const key = `right:${shareId}`; if (node) inputs.current.set(key, node); else inputs.current.delete(key); }}
                checked={checkedOut.includes(shareId)} onCheck={checked => setChecked("right", shareId, checked)}
                disabled={disabled} selectionDisabled={locked} sortDisabled={!!rightQuery.trim()} onMove={move} />; })}
              {!visibleSelected.length ? <li className="px-2 py-4 text-xs text-muted-foreground">{t(!selected.length ? "modelHub.emptyQueue" : "modelHub.noSearchResults")}</li> : null}
            </ol>
          </SortableContext>
        </DndContext>
      </section>
      </div>
      {statusText ? <div className="text-xs text-muted-foreground" role="status">{statusText}</div> : null}
      <span className="sr-only" role="status">{announcement}</span>
    </div>
  );
}
