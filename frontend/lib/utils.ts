import { type ClassValue, clsx } from "clsx";
import { twMerge } from "tailwind-merge";

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs));
}

export function preferredScrollBehavior(): ScrollBehavior {
  if (typeof window === "undefined") return "auto";
  return window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth";
}

function activeLocale(locale?: string) {
  if (locale) return locale;
  if (typeof document !== "undefined") return document.documentElement.lang || undefined;
  return undefined;
}

export function formatNumber(value: unknown, locale?: string) {
  const n = Number(value || 0);
  return Number.isFinite(n) ? new Intl.NumberFormat(activeLocale(locale)).format(n) : "0";
}

export function formatRelativeTime(value?: string | number | Date | null, locale?: string) {
  if (!value) return "--";
  const ts = value instanceof Date ? value.getTime() : new Date(value).getTime();
  if (!Number.isFinite(ts)) return "--";
  const diff = Date.now() - ts;
  const abs = Math.abs(diff);
  const units: Array<[Intl.RelativeTimeFormatUnit, number]> = [
    ["day", 86400000],
    ["hour", 3600000],
    ["minute", 60000],
    ["second", 1000],
  ];
  const rtf = new Intl.RelativeTimeFormat(activeLocale(locale), { numeric: "auto" });
  for (const [unit, ms] of units) {
    if (abs >= ms || unit === "second") {
      return rtf.format(Math.round(-diff / ms), unit);
    }
  }
  return "--";
}

export function formatDateTime(value?: string | number | Date | null, locale?: string) {
  if (!value) return "--";
  const date = value instanceof Date ? value : new Date(value);
  if (!Number.isFinite(date.getTime())) return "--";
  return new Intl.DateTimeFormat(activeLocale(locale), {
    dateStyle: "medium",
    timeStyle: "medium",
  }).format(date);
}

export function compactTokens(value: unknown, locale?: string) {
  const n = Number(value || 0);
  if (!Number.isFinite(n)) return "0";
  return new Intl.NumberFormat(activeLocale(locale), {
    notation: Math.abs(n) >= 1_000 ? "compact" : "standard",
    maximumFractionDigits: 1,
  }).format(n);
}

export function compactNumber(value: unknown, locale?: string) {
  const n = Number(value);
  if (!Number.isFinite(n)) return "0";
  return new Intl.NumberFormat(activeLocale(locale), {
    notation: Math.abs(n) >= 1_000 ? "compact" : "standard",
    maximumFractionDigits: Number.isInteger(n) || Math.abs(n) >= 10 ? 0 : 1,
  }).format(n);
}

export function fixed(value: unknown) {
  const n = Number(value);
  return Number.isFinite(n) ? n.toFixed(n >= 10 ? 0 : 1) : "-";
}

export function percent(value: unknown) {
  const n = Number(value);
  return Number.isFinite(n) ? `${n.toFixed(n >= 10 ? 0 : 1)}%` : "-";
}

export function formatBytes(value: unknown) {
  const n = Number(value || 0);
  if (!Number.isFinite(n) || n <= 0) return "-";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let idx = 0;
  while (v >= 1024 && idx < units.length - 1) {
    v /= 1024;
    idx += 1;
  }
  return `${v.toFixed(v >= 10 ? 0 : 1)} ${units[idx]}`;
}

export function formatUptime(value?: number | null) {
  if (!value) return "-";
  const days = Math.floor(value / 86400);
  const hours = Math.floor((value % 86400) / 3600);
  const minutes = Math.floor((value % 3600) / 60);
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${minutes}m`;
  return `${minutes}m`;
}
