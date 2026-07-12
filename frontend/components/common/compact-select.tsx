"use client";

import { ListBox, Select } from "@heroui/react";
import { cn } from "@/lib/utils";

type CompactSelectOption = {
  value: string;
  label: string;
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
      <Select.Trigger className={cn("min-h-9 rounded-lg border bg-white px-3 text-xs shadow-sm", triggerClassName)}>
        <Select.Value className="min-w-0 truncate pr-2 text-xs font-medium text-foreground">{selected?.label || value}</Select.Value>
        <Select.Indicator className="text-muted-foreground" />
      </Select.Trigger>
      <Select.Popover className="min-w-[var(--trigger-width)] bg-white text-foreground">
        <ListBox aria-label={ariaLabel}>
          {options.map((option) => (
            <ListBox.Item key={option.value || EMPTY_KEY} id={option.value || EMPTY_KEY} textValue={option.label}>
              {option.label}
            </ListBox.Item>
          ))}
        </ListBox>
      </Select.Popover>
    </Select>
  );
}
