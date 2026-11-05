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
  } = useSortable({ id });

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
        "flex min-w-0 items-center gap-1.5 rounded-md border border-border bg-white px-2 py-1",
        isDragging && "shadow-lg ring-1 ring-primary/30",
      )}
    >
      <button
        type="button"
        className="inline-flex h-6 w-5 shrink-0 cursor-grab items-center justify-center text-slate-400 hover:text-slate-600 active:cursor-grabbing disabled:opacity-30"
        disabled={disabled}
        aria-label={t("modelHub.dragCandidate")}
        {...attributes}
        {...listeners}
      >
        <GripVertical className="h-3.5 w-3.5" />
      </button>
      <span className="w-4 shrink-0 text-center text-[10px] font-medium text-slate-400">
        {index + 1}
      </span>
      <div className="min-w-0 flex-1">
        <CompactSelect
          value={shareId}
          options={options}
          onChange={() => {}}
          ariaLabel={t("modelHub.candidateOrder", { order: index + 1 })}
          disabled
          className="w-full"
          triggerClassName="min-h-8 w-full py-0.5 border-transparent bg-transparent shadow-none"
        />
      </div>
      <div className="flex shrink-0 items-center gap-0.5">
        <button
          type="button"
          disabled={disabled || isFirst}
          className="inline-flex h-6 w-5 items-center justify-center rounded text-slate-400 hover:bg-slate-100 hover:text-slate-700 disabled:opacity-30"
          title={t("modelHub.candidateOrder", { order: index })}
          aria-label={t("modelHub.candidateOrder", { order: index })}
          onClick={onMoveUp}
        >
          <ChevronUp className="h-3 w-3" />
        </button>
        <button
          type="button"
          disabled={disabled || isLast}
          className="inline-flex h-6 w-5 items-center justify-center rounded text-slate-400 hover:bg-slate-100 hover:text-slate-700 disabled:opacity-30"
          title={t("modelHub.candidateOrder", { order: index + 2 })}
          aria-label={t("modelHub.candidateOrder", { order: index + 2 })}
          onClick={onMoveDown}
        >
          <ChevronDown className="h-3 w-3" />
        </button>
        <button
          type="button"
          disabled={disabled}
          className="inline-flex h-6 w-5 items-center justify-center rounded text-slate-400 hover:bg-rose-50 hover:text-rose-600 disabled:opacity-30"
          title={t("modelHub.removeCandidate")}
          aria-label={t("modelHub.removeCandidate")}
          onClick={onRemove}
        >
          <X className="h-3 w-3" />
        </button>
      </div>
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
    () => candidates.map((_, index) => `${routeClientId}-candidate-${index}`),
    [candidates, routeClientId],
  );

  const handleDragEnd = React.useCallback(
    (event: DragEndEvent) => {
      const { active, over } = event;
      if (!over || active.id === over.id) return;
      const fromIndex = itemIds.indexOf(String(active.id));
      const toIndex = itemIds.indexOf(String(over.id));
      if (fromIndex >= 0 && toIndex >= 0) onMove(fromIndex, toIndex);
    },
    [itemIds, onMove],
  );

  const availableOptions = shareOptions.filter(
    (option) => option.value !== primaryShareId && !candidates.includes(option.value),
  );

  const [pickerValue, setPickerValue] = React.useState("");

  return (
    <div className="grid min-w-0 gap-1.5">
      <span className="text-[11px] font-medium text-slate-500">{t("modelHub.candidatesList")}</span>
      {candidates.length > 0 ? (
        <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={handleDragEnd}>
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
      {candidates.length < 5 && availableOptions.length > 0 ? (
        <div className="flex min-w-0 items-center gap-1.5">
          <div className="min-w-0 flex-1">
            <CompactSelect
              value={pickerValue}
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
            disabled={disabled || !pickerValue}
            className="inline-flex h-7 shrink-0 items-center gap-1 rounded-md border border-border bg-white px-2 text-[11px] font-medium text-slate-600 hover:bg-slate-50 disabled:opacity-40"
            onClick={() => {
              if (!pickerValue) return;
              onAdd(pickerValue);
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
