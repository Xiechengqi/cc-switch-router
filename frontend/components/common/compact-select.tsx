"use client";

import * as React from "react";
import { ListBox, Select } from "@heroui/react";
import { cn } from "@/lib/utils";

type CompactSelectOption = {
  value: string;
  label: string;
  description?: string;
  content?: React.ReactNode;
};

const EMPTY_KEY = "__router_empty_select_value__";

export function CompactSelect({
  value,
  options,
  onChange,
  ariaLabel,
  disabled = false,
  className,
  triggerClassName,
}: {
  value: string;
  options: CompactSelectOption[];
  onChange: (value: string) => void;
  ariaLabel: string;
  disabled?: boolean;
  className?: string;
  triggerClassName?: string;
}) {
  const selected = options.find((option) => option.value === value) || options[0];
  return (
    <Select
      selectedKey={value === "" ? EMPTY_KEY : value}
      isDisabled={disabled}
      aria-label={ariaLabel}
      className={className}
      onSelectionChange={(key: React.Key | null) => {
        const next = String(key || "");
        if (next) onChange(next === EMPTY_KEY ? "" : next);
      }}
    >
      <Select.Trigger
        className={cn(
          "min-h-9 rounded-lg border bg-white px-3 text-xs shadow-sm",
          selected?.content ? undefined : selected?.description && "py-2",
          triggerClassName,
        )}
      >
        <Select.Value className="min-w-0 flex-1 pr-2 text-left text-xs text-foreground">
          {selected?.content ? (
            <span className="block min-w-0 truncate">{selected.content}</span>
          ) : (
            <span className="grid min-w-0 gap-0.5">
              <span className="truncate font-medium">{selected?.label || value}</span>
              {selected?.description ? (
                <span className="truncate text-[11px] font-normal text-muted-foreground">
                  {selected.description}
                </span>
              ) : null}
            </span>
          )}
        </Select.Value>
        <Select.Indicator className="text-muted-foreground" />
      </Select.Trigger>
      <Select.Popover className="min-w-[var(--trigger-width)] bg-white text-foreground">
        <ListBox aria-label={ariaLabel}>
          {options.map((option) => (
            <ListBox.Item
              key={option.value || EMPTY_KEY}
              id={option.value || EMPTY_KEY}
              textValue={[option.label, option.description].filter(Boolean).join(" ")}
              className={option.description && !option.content ? "py-2" : undefined}
            >
              {option.content ? (
                <span className="block min-w-0 truncate">{option.content}</span>
              ) : (
                <span className="grid min-w-0 gap-0.5">
                  <span className="truncate text-xs font-medium">{option.label}</span>
                  {option.description ? (
                    <span className="truncate text-[11px] text-muted-foreground">
                      {option.description}
                    </span>
                  ) : null}
                </span>
              )}
            </ListBox.Item>
          ))}
        </ListBox>
      </Select.Popover>
    </Select>
  );
}
