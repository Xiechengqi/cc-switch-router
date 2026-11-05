"use client";

import * as React from "react";
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core";
import {
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { ChevronDown, ChevronUp, GripVertical, Plus, X } from "lucide-react";
import { CompactSelect } from "@/components/common/compact-select";
import { cn } from "@/lib/utils";
import type { MessageKey } from "@/lib/i18n";
import { MAX_MODEL_ROUTE_CANDIDATES, modelRouteDragAnnouncements } from "@/lib/model-routing";

type CandidateListProps = {
  routeClientId: string;
  primaryShareId: string;
  candidates: string[];
  shareOptions: { value: string; label: string; description?: string; content?: React.ReactNode }[];
  disabled: boolean;
  t: (key: MessageKey, params?: Record<string, string | number>) => string;
  onAdd: (shareId: string) => void;
  onRemove: (index: number) => void;
  onMove: (fromIndex: number, toIndex: number) => void;
};

function SortableCandidateRow({
  id,
  index,
  shareId,
  options,
  disabled,
  t,
  onRemove,
  onMoveUp,
  onMoveDown,
  isFirst,
  isLast,
}: {
  id: string;
  index: number;
  shareId: string;
  options: CandidateListProps["shareOptions"];
  disabled: boolean;
  t: CandidateListProps["t"];
  onRemove: () => void;
  onMoveUp: () => void;
  onMoveDown: () => void;
  isFirst: boolean;
  isLast: boolean;
}) {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id, disabled });

  const style: React.CSSProperties = {
    transform: CSS.Transform.toString(transform),
    transition,
    zIndex: isDragging ? 10 : undefined,
    opacity: isDragging ? 0.85 : undefined,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(
        "model-route-account model-route-sortable grid gap-1 rounded-md bg-white p-1",
        isDragging && "shadow-lg ring-1 ring-primary/30",
      )}
    >
      <div className="flex min-w-0 items-center gap-1">
        <button
          type="button"
          className="inline-flex h-11 w-11 touch-none shrink-0 cursor-grab items-center justify-center rounded text-muted-foreground hover:bg-secondary active:cursor-grabbing focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
          disabled={disabled}
          aria-label={t("modelHub.dragCandidate")}
          {...attributes}
          {...listeners}
        >
          <GripVertical className="h-3.5 w-3.5" />
        </button>
        <span className="min-w-0 flex-1 text-xs font-medium tabular-nums text-muted-foreground">
          {index + 2}
        </span>
        <div className="flex shrink-0 items-center gap-0.5">
          <button
            type="button"
            disabled={disabled || isFirst}
            className="inline-flex h-11 w-11 items-center justify-center rounded text-muted-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t("modelHub.moveUp")}
            aria-label={t("modelHub.moveUp")}
            onClick={onMoveUp}
          >
            <ChevronUp className="h-3 w-3" />
          </button>
          <button
            type="button"
            disabled={disabled || isLast}
            className="inline-flex h-11 w-11 items-center justify-center rounded text-muted-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t("modelHub.moveDown")}
            aria-label={t("modelHub.moveDown")}
            onClick={onMoveDown}
          >
            <ChevronDown className="h-3 w-3" />
          </button>
          <button
            type="button"
            disabled={disabled}
            className="inline-flex h-11 w-11 items-center justify-center rounded text-muted-foreground hover:text-destructive active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
            title={t("modelHub.removeCandidate")}
            aria-label={t("modelHub.removeCandidate")}
            onClick={onRemove}
          >
            <X className="h-3 w-3" />
          </button>
        </div>
      </div>
      <div className="min-w-0 truncate px-1 text-xs">{options.find(option => option.value === shareId)?.label || shareId}</div>
    </div>
  );
}

export function CandidateList({
  routeClientId,
  primaryShareId,
  candidates,
  shareOptions,
  disabled,
  t,
  onAdd,
  onRemove,
  onMove,
}: CandidateListProps) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const itemIds = React.useMemo(
    () => candidates.map(shareId => JSON.stringify([routeClientId, shareId])),
    [candidates, routeClientId],
  );

  const handleDragEnd = React.useCallback(
    (event: DragEndEvent) => {
      if (disabled) return;
      const { active, over } = event;
      if (!over || active.id === over.id) return;
      const fromIndex = itemIds.indexOf(String(active.id));
      const toIndex = itemIds.indexOf(String(over.id));
      if (fromIndex >= 0 && toIndex >= 0) onMove(fromIndex, toIndex);
    },
    [disabled, itemIds, onMove],
  );

  const availableOptions = shareOptions.filter(
    (option) => option.value !== primaryShareId && !candidates.includes(option.value),
  );

  const [pickerValue, setPickerValue] = React.useState("");
  const validPickerValue = availableOptions.some(option => option.value === pickerValue)
    ? pickerValue : availableOptions[0]?.value || "";
  const [open, setOpen] = React.useState(false);

  return (
    <div className="grid min-w-0 gap-1.5">
      <button
        type="button"
        className="inline-flex min-h-8 w-fit items-center gap-1 rounded px-1 text-[11px] font-medium text-slate-500 hover:bg-secondary focus-visible:outline-2 focus-visible:outline-ring"
        aria-expanded={open}
        onClick={() => setOpen(current => !current)}
      >
        <ChevronDown className={cn("h-3 w-3 shrink-0 transition-transform", open && "rotate-180")} />
        {t("modelHub.candidates", { count: candidates.length })}
      </button>
      {open && candidates.length > 0 ? (
        <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}
          accessibility={{
            screenReaderInstructions: { draggable: t("modelHub.dragInstructions") },
            announcements: modelRouteDragAnnouncements(itemIds, candidates.map(id => shareOptions.find(option => option.value === id)?.label || id), 2, t)
          }}>
          <SortableContext items={itemIds} strategy={verticalListSortingStrategy}>
            <div className="grid min-w-0 gap-1">
              {candidates.map((shareId, index) => {
                const options = shareOptions.filter(
                  (option) =>
                    option.value === shareId ||
                    (option.value !== primaryShareId && !candidates.includes(option.value)),
                );
                return (
                  <SortableCandidateRow
                    key={itemIds[index]}
                    id={itemIds[index]}
                    index={index}
                    shareId={shareId}
                    options={options}
                    disabled={disabled}
                    t={t}
                    onRemove={() => onRemove(index)}
                    onMoveUp={() => onMove(index, index - 1)}
                    onMoveDown={() => onMove(index, index + 1)}
                    isFirst={index === 0}
                    isLast={index === candidates.length - 1}
                  />
                );
              })}
            </div>
          </SortableContext>
        </DndContext>
      ) : null}
      {open && candidates.length < MAX_MODEL_ROUTE_CANDIDATES && availableOptions.length > 0 ? (
        <div className="flex min-w-0 items-center gap-1.5">
          <div className="min-w-0 flex-1">
            <CompactSelect
              value={validPickerValue}
              options={availableOptions}
              onChange={setPickerValue}
              ariaLabel={t("modelHub.addCandidate")}
              disabled={disabled}
              className="w-full"
              triggerClassName="min-h-8 w-full py-0.5"
            />
          </div>
          <button
            type="button"
            disabled={disabled || !validPickerValue}
            className="inline-flex min-h-11 shrink-0 items-center gap-1 whitespace-nowrap rounded-md border border-border bg-white px-2 text-xs font-medium text-muted-foreground hover:bg-secondary active:bg-secondary focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-not-allowed disabled:opacity-40"
            onClick={() => {
              if (!validPickerValue) return;
              onAdd(validPickerValue);
              setPickerValue("");
            }}
          >
            <Plus className="h-3 w-3" />
            {t("modelHub.addCandidate")}
          </button>
        </div>
      ) : null}
    </div>
  );
}
