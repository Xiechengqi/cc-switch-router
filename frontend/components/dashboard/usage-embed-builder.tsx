"use client";

import * as React from "react";
import { Button } from "@heroui/react";
import { Copy } from "lucide-react";
import { useLocaleText } from "@/components/i18n/locale-provider";
import { cn } from "@/lib/utils";
import { USAGE_PERIODS } from "@/components/dashboard/account-usage-shared";
import type { UsageEmbedOptions } from "@/lib/usage-card-preferences";

function buildEmbedQuery(options: UsageEmbedOptions, stamp?: number) {
  const params = new URLSearchParams();
  params.set("period", options.period);
  params.set("theme", options.theme);
  params.set("models", String(options.models));
  params.set("showBreakdown", options.showBreakdown ? "1" : "0");
  params.set("showModels", options.showModels ? "1" : "0");
  params.set("compact", options.compact ? "1" : "0");
  params.set("format", options.format);
  if (stamp != null) params.set("t", String(stamp));
  return params.toString();
}

export function buildUsageEmbedUrl(
  host: string,
  userId: string,
  options: UsageEmbedOptions,
  stamp?: number,
) {
  const query = buildEmbedQuery(options, stamp);
  return `https://${host}/v1/public/embed/usage/${encodeURIComponent(userId)}.svg?${query}`;
}

export function buildUsageEmbedMarkdown(host: string, userId: string, options: UsageEmbedOptions) {
  const src = buildUsageEmbedUrl(host, userId, options);
  return `[![TokenSwitch Usage](${src})](${src})`;
}

export function buildUsageEmbedHtml(host: string, userId: string, options: UsageEmbedOptions) {
  const src = buildUsageEmbedUrl(host, userId, options);
  return `<object data="${src}" type="image/svg+xml" width="680" aria-label="TokenSwitch Usage"><img src="${src}" alt="TokenSwitch Usage" width="680" /></object>`;
}

type Translate = ReturnType<typeof useLocaleText>["t"];

export function UsageEmbedBuilder({
  host,
  userId,
  enabled,
  options,
  onChange,
  t,
}: {
  host: string;
  userId: string;
  enabled: boolean;
  options: UsageEmbedOptions;
  onChange: (next: UsageEmbedOptions) => void;
  t: Translate;
}) {
  const [copied, setCopied] = React.useState<"md" | "html" | "url" | null>(null);
  const [previewStamp, setPreviewStamp] = React.useState(() => Date.now());
  const previewRef = React.useRef<HTMLObjectElement>(null);

  React.useEffect(() => {
    setPreviewStamp(Date.now());
  }, [options, enabled, userId]);

  const patch = (partial: Partial<UsageEmbedOptions>) => {
    onChange({ ...options, ...partial });
  };

  const markdown = enabled ? buildUsageEmbedMarkdown(host, userId, options) : "";
  const html = enabled ? buildUsageEmbedHtml(host, userId, options) : "";
  const url = enabled ? buildUsageEmbedUrl(host, userId, options) : "";
  const previewSrc = enabled ? buildUsageEmbedUrl(host, userId, options, previewStamp) : "";

  const syncPeriodFromPreview = () => {
    try {
      const loadedUrl = previewRef.current?.contentDocument?.URL;
      if (!loadedUrl) return;
      const nextPeriod = new URL(loadedUrl).searchParams.get("period");
      if (
        (nextPeriod === "24h" || nextPeriod === "7d" || nextPeriod === "30d")
        && nextPeriod !== options.period
      ) {
        patch({ period: nextPeriod });
      }
    } catch {
      // Cross-origin embeds still navigate internally; only control-state syncing is unavailable.
    }
  };

  const copy = async (kind: "md" | "html" | "url", value: string) => {
    if (!value) return;
    await navigator.clipboard.writeText(value);
    setCopied(kind);
    window.setTimeout(() => setCopied(null), 1500);
  };

  if (!enabled) {
    return <p className="text-sm text-muted-foreground">{t("account.consumerUsage.embedPrivate")}</p>;
  }

  return (
    <div className="grid gap-4">
      <div className="grid gap-3 rounded-lg border border-border bg-muted/20 p-3 sm:grid-cols-2">
        <label className="grid gap-1 text-sm">
          <span className="text-muted-foreground">{t("account.consumerUsage.embedPeriod")}</span>
          <div className="flex flex-wrap gap-1">
            {USAGE_PERIODS.map((item) => (
              <button
                key={item}
                type="button"
                className={cn(
                  "rounded-md border px-2.5 py-1 text-xs transition-colors",
                  options.period === item
                    ? "border-foreground/25 bg-background font-semibold text-foreground"
                    : "border-transparent text-muted-foreground hover:border-border hover:bg-muted/40",
                )}
                onClick={() => patch({ period: item })}
              >
                {t(`account.usage.period.${item}`)}
              </button>
            ))}
          </div>
        </label>

        <label className="grid gap-1 text-sm">
          <span className="text-muted-foreground">{t("account.consumerUsage.embedTheme")}</span>
          <div className="flex flex-wrap gap-1">
            {(["dark", "light"] as const).map((item) => (
              <button
                key={item}
                type="button"
                className={cn(
                  "rounded-md border px-2.5 py-1 text-xs capitalize transition-colors",
                  options.theme === item
                    ? "border-foreground/25 bg-background font-semibold text-foreground"
                    : "border-transparent text-muted-foreground hover:border-border hover:bg-muted/40",
                )}
                onClick={() => patch({ theme: item })}
              >
                {t(`account.consumerUsage.theme.${item}`)}
              </button>
            ))}
          </div>
        </label>

        <label className="grid gap-1 text-sm">
          <span className="text-muted-foreground">{t("account.consumerUsage.embedModels")}</span>
          <input
            type="range"
            min={1}
            max={16}
            value={options.models}
            onChange={(event) => patch({ models: Number(event.target.value) })}
            className="w-full accent-[var(--accent)]"
          />
          <span className="font-mono text-xs tabular-nums text-foreground">{options.models}</span>
        </label>

        <label className="grid gap-1 text-sm">
          <span className="text-muted-foreground">{t("account.consumerUsage.embedFormat")}</span>
          <div className="flex flex-wrap gap-1">
            {(["compact", "full"] as const).map((item) => (
              <button
                key={item}
                type="button"
                className={cn(
                  "rounded-md border px-2.5 py-1 text-xs capitalize transition-colors",
                  options.format === item
                    ? "border-foreground/25 bg-background font-semibold text-foreground"
                    : "border-transparent text-muted-foreground hover:border-border hover:bg-muted/40",
                )}
                onClick={() => patch({ format: item })}
              >
                {t(`account.consumerUsage.format.${item}`)}
              </button>
            ))}
          </div>
        </label>

        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={options.showBreakdown}
            onChange={(event) => patch({ showBreakdown: event.target.checked })}
            className="h-4 w-4 accent-[var(--accent)]"
          />
          <span>{t("account.consumerUsage.embedShowBreakdown")}</span>
        </label>

        <label className="flex items-center gap-2 text-sm">
          <input
            type="checkbox"
            checked={options.showModels}
            onChange={(event) => patch({ showModels: event.target.checked })}
            className="h-4 w-4 accent-[var(--accent)]"
          />
          <span>{t("account.consumerUsage.embedShowModels")}</span>
        </label>

        <label className="flex items-center gap-2 text-sm sm:col-span-2">
          <input
            type="checkbox"
            checked={options.compact}
            onChange={(event) => patch({ compact: event.target.checked })}
            className="h-4 w-4 accent-[var(--accent)]"
          />
          <span>{t("account.consumerUsage.embedCompact")}</span>
        </label>
      </div>

      <div className="grid gap-2">
        <div className="text-sm font-semibold text-foreground">{t("account.consumerUsage.preview")}</div>
        <object
          ref={previewRef}
          key={previewSrc}
          data={previewSrc}
          type="image/svg+xml"
          aria-label={t("account.consumerUsage.preview")}
          className="w-full max-w-[680px] rounded-lg border border-border bg-muted/20"
          width={680}
          onLoad={syncPeriodFromPreview}
        >
          {t("account.consumerUsage.previewUnavailable")}
        </object>
      </div>

      <SnippetBlock
        title={t("account.consumerUsage.embedMarkdown")}
        value={markdown}
        copied={copied === "md"}
        onCopy={() => void copy("md", markdown)}
        t={t}
      />
      <SnippetBlock
        title={t("account.consumerUsage.embedHtml")}
        value={html}
        copied={copied === "html"}
        onCopy={() => void copy("html", html)}
        t={t}
      />
      <SnippetBlock
        title={t("account.consumerUsage.embedUrl")}
        value={url}
        copied={copied === "url"}
        onCopy={() => void copy("url", url)}
        t={t}
      />
    </div>
  );
}

function SnippetBlock({
  title,
  value,
  copied,
  onCopy,
  t,
}: {
  title: string;
  value: string;
  copied: boolean;
  onCopy: () => void;
  t: Translate;
}) {
  return (
    <div className="grid gap-1.5">
      <div className="text-sm font-semibold text-foreground">{title}</div>
      <div className="relative rounded-lg border border-border bg-muted/30 p-3">
        <Button variant="ghost" size="sm" className="absolute right-2 top-2 h-8" onClick={onCopy}>
          <Copy className="h-3.5 w-3.5" />
          {copied ? t("account.consumerUsage.copied") : t("account.consumerUsage.copyEmbed")}
        </Button>
        <pre className="overflow-x-auto whitespace-pre-wrap break-all pr-24 font-mono text-[11px] leading-relaxed text-foreground sm:text-xs">
          {value}
        </pre>
      </div>
    </div>
  );
}
