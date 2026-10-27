"use client";

import * as React from "react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { getMarketTrialPolicy } from "@/lib/market-trial-policy";

type ClientTrialInputProps = {
  value: string;
  onChange: (value: string) => void;
  disabled: boolean;
  defaultHours?: number;
};

export function ClientTrialInput({ value, onChange, disabled, defaultHours }: ClientTrialInputProps) {
  const { t } = useLocaleText();
  const inputId = React.useId();
  const [routerDefault, setRouterDefault] = React.useState(defaultHours);
  React.useEffect(() => {
    let active = true;
    getMarketTrialPolicy()
      .then((policy) => {
        if (active) setRouterDefault(policy.clientHours);
      })
      .catch(() => {
        // The server still resolves an inherited value. A display-only lookup
        // failure must not silently freeze a guessed default into the offer.
      });
    return () => { active = false; };
  }, []);
  return (
    <div className="grid gap-1 text-sm">
      <label htmlFor={inputId} className="text-muted-foreground">{t("clientMarket.trialHours")}</label>
      <input
        id={inputId}
        aria-describedby={`${inputId}-help`}
        type="number"
        min={0}
        max={720}
        step={1}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
        placeholder={routerDefault == null ? t("clientMarket.trialInherit") : t("clientMarket.trialDefault", { hours: routerDefault })}
        className="h-10 w-full rounded-md border px-3"
      />
      <p id={`${inputId}-help`} className="text-xs text-muted-foreground">{t("clientMarket.trialHelp")}</p>
    </div>
  );
}
