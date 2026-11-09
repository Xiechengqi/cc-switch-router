"use client";

import * as React from "react";
import { DndContext, KeyboardSensor, PointerSensor, closestCenter, useSensor, useSensors, type DragEndEvent } from "@dnd-kit/core";
import { SortableContext, sortableKeyboardCoordinates, useSortable, verticalListSortingStrategy } from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { LayoutGroup, MotionConfig, motion } from "motion/react";
import { ArrowDown, ArrowLeft, ArrowRight, ArrowUp, ChevronDown, ChevronUp, GripVertical } from "lucide-react";
import { RoutingShareSummary, type RoutingShareSummaryProps } from "./model-hub-share-summary";
import { QueueRowTraffic } from "./model-hub-traffic";
import type { ShareRouteTraffic } from "@/lib/model-routing-traffic";
import { MAX_MODEL_ROUTE_TARGETS, modelRouteDragAnnouncements, moveModelRouteTarget, sharesForProtocol, transferModelRouteTargets } from "@/lib/model-routing";
import type { ModelRoutingApp, UserModelRoutingShare } from "@/lib/types";
import { cn } from "@/lib/utils";

// Each row carries its own arrow toward the other pane; there is no separate
// selection step. A moved row keeps its layoutId, so it travels across panes.
const LAYOUT_TRANSITION = { layout: { duration: 0.42, ease: [0.16, 1, 0.3, 1] as const } };
const focusRing = "focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring";
const iconButton = cn("inline-flex size-9 shrink-0 items-center justify-center rounded-full text-muted-foreground transition-colors duration-200 hover:bg-secondary hover:text-foreground active:bg-secondary disabled:pointer-events-none disabled:opacity-30 pointer-coarse:size-11", focusRing);
const arrowButton = cn("inline-flex size-9 shrink-0 items-center justify-center rounded-full border bg-card shadow-[0_1px_3px_rgba(0,0,0,0.06)] transition-all duration-200 ease-out hover:-translate-y-0.5 active:scale-95 disabled:pointer-events-none disabled:opacity-30 motion-reduce:transform-none pointer-coarse:size-11", focusRing);
const addButton = cn(arrowButton, "border-border text-primary group-hover/row:border-primary/40 hover:border-transparent hover:bg-gradient-to-r hover:from-accent hover:to-accent-secondary hover:text-primary-foreground hover:shadow-[0_4px_14px_rgba(0,82,255,0.25)]");
const removeButton = cn(arrowButton, "border-border text-muted-foreground hover:border-red-200 hover:bg-red-50 hover:text-red-600");
/** Rows a pane shows before it scrolls. */
const VISIBLE_ROWS = 3;

/** Caps a list at `rows` rows by measuring them, so the cap follows whatever a
 * row holds (a traffic line, touch-sized buttons, a larger font). Offsets are
 * layout positions, so drag and layout-animation transforms do not move it.
 * Work happens only when a row is added, removed or resized. */
function useVisibleRows<T extends HTMLElement>(rows: number) {
  const ref = React.useRef<T>(null);
  React.useLayoutEffect(() => {
    const list = ref.current;
    if (!list || typeof ResizeObserver === "undefined") return;
    const fit = () => {
      const last = list.children[rows - 1] as HTMLElement | undefined;
      // "none", not "": an empty value would fall back to the stylesheet's pre-measure cap.
      if (list.children.length <= rows || !last) { list.style.maxHeight = "none"; return; }
      const bottom = parseFloat(getComputedStyle(list).paddingBottom) || 0;
      list.style.maxHeight = `${last.offsetTop + last.offsetHeight + bottom}px`;
    };
    const resize = new ResizeObserver(fit);
    const observeRows = () => {
      resize.disconnect();
      resize.observe(list);
      for (const child of Array.from(list.children)) resize.observe(child);
    };
    const mutation = new MutationObserver(() => { observeRows(); fit(); });
    mutation.observe(list, { childList: true });
    observeRows();
    fit();
    return () => { resize.disconnect(); mutation.disconnect(); };
  }, [rows]);
  return ref;
}

function DirectionIcon({ toward }: { toward: "queue" | "pool" }) {
  // Side by side the arrow points across; stacked (narrow) it points up/down.
  return toward === "queue"
    ? <><ArrowRight className="model-route-transfer-horizontal size-4" aria-hidden /><ArrowDown className="model-route-transfer-vertical size-4" aria-hidden /></>
    : <><ArrowLeft className="model-route-transfer-horizontal size-4" aria-hidden /><ArrowUp className="model-route-transfer-vertical size-4" aria-hidden /></>;
}

function RankBadge({ index, t }: { index: number; t: RoutingShareSummaryProps["t"] }) {
  if (index === 0) {
    return <span className="shrink-0 rounded-full bg-gradient-to-r from-accent to-accent-secondary px-2 py-0.5 text-[11px] font-medium text-primary-foreground shadow-[0_2px_8px_rgba(0,82,255,0.25)]">
      {t("modelHub.candidatePrimary")}
    </span>;
  }
  return <span className="inline-flex size-6 shrink-0 items-center justify-center rounded-full bg-secondary font-mono text-[11px] tabular-nums text-muted-foreground">
    <span aria-hidden>{index + 1}</span><span className="sr-only">{t("modelHub.candidateOrder", { order: index + 1 })}</span>
  </span>;
}

function QueueRow({ id, index, total, disabled, locked, layoutKey, traffic, onRemove, onMove, buttonRef, ...summary }: RoutingShareSummaryProps & {
  /** Undefined hides the counters (no live data, or the rule is an unsaved draft). */
  traffic?: { stats?: ShareRouteTraffic };
  /** `disabled` freezes the whole row; `locked` (also true mid-drag) only stops clicks. */
  id: string; index: number; total: number; disabled: boolean; locked: boolean; layoutKey: string;
  onRemove: () => void; onMove: (from: number, to: number) => void;
  buttonRef: (node: HTMLButtonElement | null) => void;
}) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id, disabled });
  const name = summary.share?.subdomain || summary.shareId;
  return (
    <li ref={setNodeRef} style={{ transform: CSS.Transform.toString(transform), transition }}
      className={cn("model-route-sortable min-w-0", isDragging && "relative z-10")}>
      {/* Position only: animating size would stretch the text mid-flight. */}
      <motion.div layoutId={summary.shareId} layout="position" layoutDependency={layoutKey} transition={LAYOUT_TRANSITION}
        className={cn("flex min-w-0 items-center gap-2 rounded-lg border px-2 py-2 transition-[border-color,background-color,box-shadow] duration-200",
          !summary.share ? "border-dashed border-red-200 bg-red-50/40"
            : index === 0 ? "border-primary/25 bg-primary/[0.03]" : "border-border bg-card",
          isDragging && "shadow-[0_20px_25px_rgba(0,0,0,0.1)] ring-2 ring-primary/30")}>
        <button ref={buttonRef} type="button" disabled={locked} onClick={onRemove} data-model-route-target={summary.shareId}
          className={removeButton} aria-label={summary.t("modelHub.removeFromQueue", { name })} title={summary.t("modelHub.removeFromQueue", { name })}>
          <DirectionIcon toward="pool" />
        </button>
        <RankBadge index={index} t={summary.t} />
        <div className="grid min-w-0 flex-1 gap-0.5">
          <RoutingShareSummary {...summary} />
          {traffic ? <QueueRowTraffic traffic={traffic.stats} t={summary.t} /> : null}
        </div>
        <span className="flex shrink-0 items-center">
          <button {...attributes} {...listeners} type="button" disabled={disabled}
            aria-label={summary.t("modelHub.dragCandidate")} className={cn(iconButton, "touch-none cursor-grab active:cursor-grabbing")}>
            <GripVertical className="size-4" aria-hidden />
          </button>
          <button type="button" disabled={locked || index === 0} onClick={() => onMove(index, index - 1)}
            className={iconButton} aria-label={summary.t("modelHub.moveUp")}><ChevronUp className="size-4" aria-hidden /></button>
          <button type="button" disabled={locked || index === total - 1} onClick={() => onMove(index, index + 1)}
            className={iconButton} aria-label={summary.t("modelHub.moveDown")}><ChevronDown className="size-4" aria-hidden /></button>
        </span>
      </motion.div>
    </li>
  );
}

export type PickerAction = { kind: "append_targets" | "remove_targets"; shareIds: string[] }
  | { kind: "move_target"; from: number; to: number };

export function SharePoolPicker({ shares, selected, app, routeKey, disabled, addBlockedReason, traffic, onChange, onAction, locale, t }: {
  shares: UserModelRoutingShare[]; selected: string[]; app: ModelRoutingApp; routeKey: string;
  /** Per-Share counters for this saved rule (plan P5-1); absent when not shown. */
  traffic?: Map<string, ShareRouteTraffic>;
  disabled: boolean; addBlockedReason?: string; onChange: (ids: string[]) => void;
  /** Returns whether the draft accepted the change; a rejection is reported by the panel's error banner. */
  onAction?: (action: PickerAction) => boolean;
} & Pick<RoutingShareSummaryProps, "locale" | "t">) {
  const [query, setQuery] = React.useState("");
  const [announcement, setAnnouncement] = React.useState("");
  const [dragging, setDragging] = React.useState(false);
  const poolList = useVisibleRows<HTMLUListElement>(VISIBLE_ROWS);
  const queueList = useVisibleRows<HTMLOListElement>(VISIBLE_ROWS);
  const buttons = React.useRef(new Map<string, HTMLButtonElement>());
  const pendingFocus = React.useRef<string[] | null>(null);
  React.useLayoutEffect(() => {
    const pending = pendingFocus.current;
    if (!pending) return;
    // Adding the sixth Share disables the pool, so skip to the moved row.
    const focus = () => pending.map(key => buttons.current.get(key)).find(node => node && !node.disabled)?.focus();
    // The destination row commits after this effect on the first pass; retry once.
    const frame = requestAnimationFrame(() => { focus(); pendingFocus.current = null; });
    return () => cancelAnimationFrame(frame);
  }, [selected]);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 8 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }));
  const eligible = sharesForProtocol(shares, app);
  const byId = new Map(eligible.map(share => [share.shareId, share]));
  const nameOf = (id: string) => byId.get(id)?.subdomain || id;
  const remaining = eligible.filter(share => !selected.includes(share.shareId));
  const needle = query.trim().toLocaleLowerCase();
  const available = remaining.filter(share => [share.shareId, share.subdomain, share.shareName,
    share.appCapabilities?.find(cap => cap.app === app)?.providerName].filter(Boolean).join(" ").toLocaleLowerCase().includes(needle));
  const availableIds = available.map(share => share.shareId);
  const searchable = eligible.length > MAX_MODEL_ROUTE_TARGETS;
  const full = selected.length >= MAX_MODEL_ROUTE_TARGETS;
  const locked = disabled || dragging;
  const addLocked = locked || full || !!addBlockedReason;
  const statusText = addBlockedReason || (full ? t("modelHub.queueFull") : "");
  // Membership and order are the only things that should move a row; dnd-kit
  // transforms during a drag must not trigger a second layout animation.
  const layoutKey = `${selected.join("|")}#${query}`;
  const buttonRef = (key: string) => (node: HTMLButtonElement | null) => {
    if (node) buttons.current.set(key, node); else buttons.current.delete(key);
  };

  const add = (shareId: string) => {
    if (addLocked) return;
    const index = availableIds.indexOf(shareId);
    const neighbor = availableIds[index + 1] || availableIds[index - 1];
    const accepted = onAction ? onAction({ kind: "append_targets", shareIds: [shareId] })
      : (onChange(transferModelRouteTargets(selected, availableIds, [shareId], "in")), true);
    if (!accepted) return;
    // Keep the keyboard in the pool so several Shares can be added in a row.
    pendingFocus.current = [...(neighbor ? [`pool:${neighbor}`] : []), `queue:${shareId}`];
    setAnnouncement(t("modelHub.addedToQueue", { name: nameOf(shareId), order: selected.length + 1 }));
  };
  const remove = (shareId: string) => {
    if (locked) return;
    const index = selected.indexOf(shareId);
    const neighbor = selected[index + 1] || selected[index - 1];
    const accepted = onAction ? onAction({ kind: "remove_targets", shareIds: [shareId] })
      : (onChange(transferModelRouteTargets(selected, [], [shareId], "out")), true);
    if (!accepted) return;
    pendingFocus.current = [...(neighbor ? [`queue:${neighbor}`] : []), `pool:${shareId}`];
    setAnnouncement(t("modelHub.removedFromQueue", { name: nameOf(shareId) }));
  };
  const ids = selected.map(id => JSON.stringify([routeKey, id]));
  const move = (from: number, to: number) => {
    if (disabled || from < 0 || to < 0 || from >= selected.length || to >= selected.length || from === to) return;
    const accepted = onAction ? onAction({ kind: "move_target", from, to })
      : (onChange(moveModelRouteTarget(selected, from, to)), true);
    if (accepted) setAnnouncement(t("modelHub.moved", { name: nameOf(selected[from]), order: to + 1 }));
  };
  const dragEnd = ({ active, over }: DragEndEvent) => {
    setDragging(false);
    if (over) move(ids.indexOf(String(active.id)), ids.indexOf(String(over.id)));
  };

  const pane = "min-w-0 overflow-hidden rounded-xl border border-border bg-card shadow-[0_1px_3px_rgba(0,0,0,0.06)]";
  const paneHeader = "flex min-h-11 min-w-0 items-center justify-between gap-2 border-b border-border px-3.5 py-2";
  const empty = "grid min-h-20 place-items-center rounded-lg border border-dashed border-border px-3 py-4 text-center text-xs leading-relaxed text-muted-foreground";

  return (
    <MotionConfig reducedMotion="user">
      <LayoutGroup id={routeKey}>
        <div className="model-route-transfer grid min-w-0 gap-2" data-model-route-picker>
          <div className="model-route-transfer-grid">
            <div className={pane}>
              <div className={paneHeader}>
                <h5 className="text-xs font-semibold text-foreground">{t("modelHub.availableShares")}</h5>
                <span className="font-mono text-[11px] tabular-nums text-muted-foreground">{remaining.length}</span>
              </div>
              {searchable || query ? (
                <div className="border-b border-border p-2">
                  <input value={query} onChange={event => setQuery(event.target.value)} type="search"
                    className={cn("h-10 w-full min-w-0 rounded-lg border border-input bg-card px-3 text-xs placeholder:text-muted-foreground/60", focusRing)}
                    aria-label={t("modelHub.searchList", { list: t("modelHub.availableShares") })} placeholder={t("modelHub.searchShares")} />
                </div>
              ) : null}
              <ul ref={poolList} className="model-route-transfer-list" aria-label={t("modelHub.availableShares")}>
                {available.map(share => {
                  const name = share.subdomain || share.shareName;
                  return (
                    <motion.li key={share.shareId} layoutId={share.shareId} layout="position" layoutDependency={layoutKey} transition={LAYOUT_TRANSITION}
                      className="group/row flex min-w-0 items-center gap-2 rounded-lg border border-transparent px-2.5 py-2 transition-colors duration-200 hover:border-border hover:bg-secondary/60">
                      <div className="min-w-0 flex-1"><RoutingShareSummary share={share} shareId={share.shareId} app={app} locale={locale} t={t} /></div>
                      <button ref={buttonRef(`pool:${share.shareId}`)} type="button" disabled={addLocked} onClick={() => add(share.shareId)}
                        className={addButton} aria-label={t("modelHub.addToQueue", { name })} title={statusText || t("modelHub.addToQueue", { name })}>
                        <DirectionIcon toward="queue" />
                      </button>
                    </motion.li>
                  );
                })}
                {!available.length ? <li className={empty}>
                  {t(!eligible.length ? "modelHub.noEligibleShares" : !remaining.length ? "modelHub.allSharesSelected" : "modelHub.noSearchResults")}
                </li> : null}
              </ul>
            </div>
            <div className={pane}>
              <div className={paneHeader}>
                <h5 className="text-xs font-semibold text-foreground">{t("modelHub.selectedShares")}</h5>
                <span className="flex items-center gap-2">
                  <span className="flex items-center gap-0.5" aria-hidden>
                    {Array.from({ length: MAX_MODEL_ROUTE_TARGETS }, (_, slot) => <span key={slot}
                      className={cn("h-1.5 w-2.5 rounded-full transition-colors duration-300", slot < selected.length ? "bg-gradient-to-r from-accent to-accent-secondary" : "bg-slate-200")} />)}
                  </span>
                  <span className="font-mono text-[11px] tabular-nums text-muted-foreground">{selected.length}/{MAX_MODEL_ROUTE_TARGETS}</span>
                </span>
              </div>
              <DndContext sensors={sensors} collisionDetection={closestCenter}
                onDragStart={() => setDragging(true)} onDragCancel={() => setDragging(false)} onDragEnd={dragEnd}
                accessibility={{
                  screenReaderInstructions: { draggable: t("modelHub.dragInstructions") },
                  announcements: modelRouteDragAnnouncements(ids, selected.map(nameOf), 1, t)
                }}>
                <SortableContext items={ids} strategy={verticalListSortingStrategy}>
                  <ol ref={queueList} className="model-route-transfer-list" aria-label={t("modelHub.selectedShares")}>
                    {selected.map((shareId, index) => <QueueRow key={ids[index]} id={ids[index]} index={index} total={selected.length}
                      share={byId.get(shareId)} shareId={shareId} app={app} locale={locale} t={t} layoutKey={layoutKey}
                      traffic={traffic ? { stats: traffic.get(shareId) } : undefined}
                      buttonRef={buttonRef(`queue:${shareId}`)} disabled={disabled} locked={locked} onRemove={() => remove(shareId)} onMove={move} />)}
                    {!selected.length ? <li className={empty}>{t("modelHub.emptyQueue")}</li> : null}
                  </ol>
                </SortableContext>
              </DndContext>
            </div>
          </div>
          {statusText ? <div className="text-xs text-muted-foreground" role="status">{statusText}</div> : null}
          <span className="sr-only" role="status">{announcement}</span>
        </div>
      </LayoutGroup>
    </MotionConfig>
  );
}
