"use client";

import * as React from "react";
import { DndContext, KeyboardSensor, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, sortableKeyboardCoordinates, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { ChevronDown, ChevronUp, GripVertical, Plus, X } from "lucide-react";
import { RoutingShareDetails, RoutingShareSummary, type RoutingShareSummaryProps } from "./model-hub-share-summary";
import { MAX_MODEL_ROUTE_TARGETS, modelRouteDragAnnouncements, moveModelRouteTarget, sharesForProtocol } from "@/lib/model-routing";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import { cn } from "@/lib/utils";

const controlClass = "inline-flex h-11 w-11 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40";

function TargetRow({ id, index, total, disabled, onMove, onRemove, handleRef, ...summary }: RoutingShareSummaryProps & {
  id: string; index: number; total: number; disabled: boolean;
  handleRef: (node: HTMLButtonElement | null) => void;
  onMove: (from: number, to: number) => void; onRemove: () => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id, disabled });
  return (
    <li ref={setNodeRef} style={{ transform: CSS.Transform.toString(transform), transition }}
      className={cn("model-route-account model-route-sortable grid gap-1 rounded-lg", isDragging && "relative z-10 bg-card outline-2 outline-primary")}>
      <div className="flex min-w-0 items-center">
        <button ref={handleRef} {...attributes} {...listeners} type="button" disabled={disabled}
          aria-label={summary.t("modelHub.dragCandidate")} className={cn(controlClass, "touch-none cursor-grab active:cursor-grabbing")}>
          <GripVertical className="h-4 w-4" aria-hidden />
        </button>
        <span className="min-w-0 flex-1 truncate text-xs font-medium tabular-nums">
          {index === 0 ? summary.t("modelHub.candidatePrimary") : summary.t("modelHub.candidateOrder", { order: index + 1 })}
        </span>
        <button type="button" disabled={disabled || index === 0} onClick={() => onMove(index, index - 1)}
          className={controlClass} aria-label={summary.t("modelHub.moveUp")}><ChevronUp className="h-4 w-4" aria-hidden /></button>
        <button type="button" disabled={disabled || index === total - 1} onClick={() => onMove(index, index + 1)}
          className={controlClass} aria-label={summary.t("modelHub.moveDown")}><ChevronDown className="h-4 w-4" aria-hidden /></button>
        <button type="button" disabled={disabled} onClick={onRemove} className={cn(controlClass, "hover:text-destructive")}
          aria-label={summary.t("modelHub.removeCandidate")}><X className="h-4 w-4" aria-hidden /></button>
      </div>
      <RoutingShareSummary {...summary} />
      <RoutingShareDetails {...summary} />
    </li>
  );
}

export function SharePoolPicker({ shares, selected, app, routeKey, disabled, addBlockedReason, onChange, locale, t }: {
  shares: UserModelRoutingShare[]; selected: string[]; app: ModelRoutingApp; routeKey: string;
  disabled: boolean; addBlockedReason?: string; onChange: (ids: string[]) => void;
} & Pick<RoutingShareSummaryProps, "locale" | "t">) {
  const [query, setQuery] = React.useState("");
  const [announcement, setAnnouncement] = React.useState("");
  const [dragging, setDragging] = React.useState(false);
  const handles = React.useRef(new Map<string, HTMLButtonElement>());
  const searchRef = React.useRef<HTMLInputElement>(null);
  const pendingFocus = React.useRef<string | null>(null);
  React.useLayoutEffect(() => {
    if (pendingFocus.current === null) return;
    (handles.current.get(pendingFocus.current) || searchRef.current)?.focus();
    pendingFocus.current = null;
  }, [selected]);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }));
  const eligible = sharesForProtocol(shares, app);
  const byId = new Map(eligible.map(share => [share.shareId, share]));
  const remaining = eligible.filter(share => !selected.includes(share.shareId));
  const needle = query.trim().toLocaleLowerCase();
  const available = remaining.filter(share => [
    share.subdomain, share.shareName,
    share.appCapabilities?.find(cap => cap.app === app)?.providerName || "",
  ].join(" ").toLocaleLowerCase().includes(needle));
  const full = selected.length >= MAX_MODEL_ROUTE_TARGETS;
  const ids = selected.map(id => JSON.stringify([routeKey, id]));
  const move = (from: number, to: number) => {
    if (disabled) return;
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
    <div className="grid min-w-0 gap-4 md:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]" data-model-route-picker>
      <section className="grid min-w-0 content-start gap-2" aria-label={t("modelHub.availableShares")}>
        <h4 className="text-xs font-semibold text-foreground">{t("modelHub.availableShares")} · {remaining.length}</h4>
        <input ref={searchRef} value={query} onChange={event => setQuery(event.target.value)} type="search"
          className="model-route-account h-11 rounded-md border border-input bg-card px-3 text-xs focus-visible:outline-2 focus-visible:outline-ring"
          aria-label={t("modelHub.searchShares")} placeholder={t("modelHub.searchShares")} />
        {full ? <p className="text-xs text-muted-foreground">{t("modelHub.queueFull")}</p> : null}
        {addBlockedReason ? <p role="status" className="text-xs text-muted-foreground">{addBlockedReason}</p> : null}
        <ul className="grid max-h-[32rem] min-w-0 content-start gap-3 overflow-y-auto overscroll-contain p-1">
          {available.map(share => (
            <li key={share.shareId} className="model-route-account grid gap-1">
              <button type="button" disabled={disabled || full || dragging || !!addBlockedReason} aria-label={t("modelHub.addShare", { name: share.subdomain || share.shareName })}
                className="grid min-w-0 gap-1 rounded-md text-left hover:outline hover:outline-border active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed"
                onClick={() => {
                  if (disabled || full || dragging || addBlockedReason || selected.includes(share.shareId)) return;
                  pendingFocus.current = share.shareId;
                  onChange([...selected, share.shareId]);
                  setAnnouncement(t("modelHub.added", { name: share.subdomain || share.shareName }));
                }}>
                <span className={cn("flex h-11 items-center gap-1 px-2 text-xs font-medium", disabled || full || !!addBlockedReason ? "text-muted-foreground" : "text-primary")}><Plus className="h-4 w-4" aria-hidden />{t("common.add")}</span>
                <RoutingShareSummary share={share} shareId={share.shareId} app={app} locale={locale} t={t} />
              </button>
              <RoutingShareDetails share={share} shareId={share.shareId} app={app} locale={locale} t={t} />
            </li>
          ))}
        </ul>
        {!available.length ? <p className="text-xs text-muted-foreground">{t(!eligible.length ? "modelHub.noEligibleShares" : !remaining.length ? "modelHub.allSharesSelected" : "modelHub.noSearchResults")}</p> : null}
      </section>
      <section className="grid min-w-0 content-start gap-2" aria-label={t("modelHub.selectedShares")}>
        <h4 className="text-xs font-semibold text-foreground">{t("modelHub.selectedShares")} · {selected.length}/{MAX_MODEL_ROUTE_TARGETS}</h4>
        <p className="model-route-account min-h-11 text-xs leading-5 text-muted-foreground">{t("modelHub.queueHint")}</p>
        <DndContext sensors={sensors} collisionDetection={closestCenter}
          onDragStart={() => setDragging(true)} onDragCancel={() => setDragging(false)} onDragEnd={dragEnd}
          accessibility={{
            screenReaderInstructions: { draggable: t("modelHub.dragInstructions") },
            announcements: modelRouteDragAnnouncements(ids, selected.map(id => byId.get(id)?.subdomain || id), 1, t)
          }}>
          <SortableContext items={ids} strategy={verticalListSortingStrategy}>
            <ol className="grid max-h-[32rem] min-w-0 content-start gap-3 overflow-y-auto overscroll-contain p-1">
              {selected.map((shareId, index) => <TargetRow key={ids[index]} id={ids[index]} index={index} total={selected.length}
                share={byId.get(shareId)} shareId={shareId} app={app} locale={locale} t={t}
                handleRef={node => { if (node) handles.current.set(shareId, node); else handles.current.delete(shareId); }}
                disabled={disabled} onMove={move} onRemove={() => {
                  if (disabled || dragging) return;
                  pendingFocus.current = selected[index + 1] || selected[index - 1] || "";
                  onChange(selected.filter(id => id !== shareId));
                  setAnnouncement(t("modelHub.removed", { name: byId.get(shareId)?.subdomain || shareId }));
                }} />)}
            </ol>
          </SortableContext>
        </DndContext>
        {!selected.length ? <p className="text-xs text-muted-foreground">{t("modelHub.emptyQueue")}</p> : null}
      </section>
      <span className="sr-only" role="status">{announcement}</span>
    </div>
  );
}
