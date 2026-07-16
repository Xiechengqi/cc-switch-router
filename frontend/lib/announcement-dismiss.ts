const TODAY_KEY = "cc_switch_router_announcement_dismiss_today_v1";
const PERMANENT_KEY = "cc_switch_router_announcement_dismiss_permanent_v1";

type TodayDismiss = {
  revision: string;
  date: string;
};

type PermanentDismiss = {
  revision: string;
};

function readJson<T>(key: string): T | null {
  if (typeof window === "undefined") return null;
  try {
    const raw = window.localStorage.getItem(key);
    if (!raw) return null;
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown) {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Ignore storage failures in private browsing or locked-down contexts.
  }
}

export function localToday(): string {
  return new Intl.DateTimeFormat("en-CA").format(new Date());
}

function isDismissedToday(revision: string): boolean {
  const stored = readJson<TodayDismiss>(TODAY_KEY);
  return stored?.revision === revision && stored?.date === localToday();
}

function isDismissedPermanently(revision: string): boolean {
  const stored = readJson<PermanentDismiss>(PERMANENT_KEY);
  return stored?.revision === revision;
}

export function shouldShowAnnouncement(
  enabled: boolean,
  revision: string,
  content: string,
): boolean {
  if (!enabled || !content.trim()) return false;
  if (isDismissedPermanently(revision)) return false;
  if (isDismissedToday(revision)) return false;
  return true;
}

export function dismissAnnouncementToday(revision: string) {
  writeJson(TODAY_KEY, { revision, date: localToday() } satisfies TodayDismiss);
}

export function dismissAnnouncementPermanent(revision: string) {
  writeJson(PERMANENT_KEY, { revision } satisfies PermanentDismiss);
}
