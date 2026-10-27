"use client";
import * as React from "react";
import { Button, Input } from "@heroui/react";
import { Eye, EyeOff } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";

export function PasswordField({
  label,
  value,
  onChange,
  autoComplete = "new-password",
  autoFocus = false,
  disabled = false,
}: {
  label: string;
  value: string;
  onChange: (value: string) => void;
  autoComplete?: "new-password" | "current-password";
  autoFocus?: boolean;
  disabled?: boolean;
}) {
  const [visible, setVisible] = React.useState(false);
  const id = React.useId();
  const { t } = useLocaleText();
  React.useEffect(() => {
    if (!value) setVisible(false);
  }, [value]);
  return (
    <div className="grid gap-1.5">
      <label htmlFor={id} className="text-sm font-medium">
        {label}
      </label>
      <div className="flex items-center gap-1">
        <Input
          id={id}
          name={autoComplete}
          aria-label={label}
          className="min-w-0 flex-1"
          type={visible ? "text" : "password"}
          value={value}
          onChange={(event) => onChange(event.target.value)}
          autoComplete={autoComplete}
          autoFocus={autoFocus}
          disabled={disabled}
          spellCheck={false}
          autoCapitalize="none"
          maxLength={256}
        />
        <Button
          type="button"
          variant="ghost"
          isIconOnly
          aria-label={t(visible ? "auth.password.hide" : "auth.password.show")}
          aria-pressed={visible}
          onClick={() => setVisible(!visible)}
        >
          {visible ? (
            <EyeOff className="h-4 w-4" />
          ) : (
            <Eye className="h-4 w-4" />
          )}
        </Button>
      </div>
    </div>
  );
}
