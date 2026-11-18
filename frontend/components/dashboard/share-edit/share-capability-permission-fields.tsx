"use client";
import { useLocaleText } from "@/components/i18n/locale-provider";
import {
  capabilityPermissionMode,
  capabilityPermissionChange,
  OPERATION_LABELS,
  type ShareOperation,
  type CapabilityPermissionMode,
} from "@/lib/share-capabilities";

export function ShareCapabilityPermissionFields({
  value,
  available,
  disabled,
  onChange,
}: {
  value?: ShareOperation[];
  available: readonly ShareOperation[];
  disabled?: boolean;
  onChange: (operations: ShareOperation[] | undefined) => void;
}) {
  const { locale } = useLocaleText();
  const lang = locale.startsWith("zh") ? 0 : 1;
  const mode = capabilityPermissionMode(value);
  return (
    <fieldset disabled={disabled} className="grid min-w-0 gap-2 sm:col-span-2">
      <legend className="mb-2 text-xs font-medium text-muted-foreground">
        {lang === 0 ? "扩展能力" : "Extended capabilities"}
      </legend>
      <select
        aria-label={lang === 0 ? "扩展能力权限" : "Extension permission"}
        className="min-h-10 max-w-full rounded-lg border border-input bg-background px-3 text-sm"
        value={mode}
        onChange={(event) =>
          onChange(
            capabilityPermissionChange(
              event.target.value as CapabilityPermissionMode,
              value,
              available,
            ),
          )
        }
      >
        <option value="inherit">
          {lang === 0
            ? "跟随 Share 设置（默认）"
            : "Follow Share settings (default)"}
        </option>
        <option value="custom" disabled={!available.length}>
          {lang === 0 ? "只允许指定能力" : "Allow selected extensions"}
        </option>
        <option value="disabled">
          {lang === 0 ? "不允许扩展能力" : "Disable extensions"}
        </option>
      </select>
      {mode === "custom" ? (
        <div className="flex flex-wrap gap-x-4 gap-y-2 text-sm">
          {available.map((operation) => (
            <label className="flex items-center gap-2" key={operation}>
              <input
                type="checkbox"
                checked={value?.includes(operation) ?? false}
                onChange={(event) =>
                  onChange(
                    event.target.checked
                      ? [...(value ?? []), operation]
                      : (value?.filter((op) => op !== operation) ?? []),
                  )
                }
              />
              {OPERATION_LABELS[operation][lang]}
            </label>
          ))}
        </div>
      ) : null}
      <p className="text-xs text-muted-foreground">
        {mode === "inherit"
          ? lang === 0
            ? "Share 以后开放或关闭能力时，此用户一起变化。"
            : "This user follows future changes to Share extensions."
          : mode === "disabled"
            ? lang === 0
              ? "仍可使用基础模型交互和理解输入，按原有权限与额度执行。"
              : "Basic model interaction and understanding inputs remain subject to existing access and quotas."
            : lang === 0
              ? "仅允许所选能力，且不能超出 Share 的开放范围。"
              : "Only selected extensions are allowed, within the Share permissions."}
      </p>
    </fieldset>
  );
}
