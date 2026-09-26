// Sizes, relative times and lists, shared by the pages that list files, sessions and projects.
import { locale, t, tn } from "./i18n";

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;

const num = (n: number, digits: number) =>
  n.toLocaleString(locale(), { minimumFractionDigits: digits, maximumFractionDigits: digits });

/** "a、b、c" / "a, b, c". */
export const joinList = (items: readonly string[]) => items.join(t("common.listSep"));

/** "0 KB", "1 KB" (anything from 1 byte up), "12 KB", "3.4 MB", "1.2 GB". */
export function fmtSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB";
  if (bytes >= GB) return `${num(bytes / GB, 1)} GB`;
  if (bytes >= MB) return `${num(bytes / MB, 1)} MB`;
  return `${num(Math.max(1, Math.round(bytes / KB)), 0)} KB`;
}

/**
 * "just now" / "3 minutes ago" / "yesterday" / a short date for a time in ms (or an ISO string); "" when missing
 * or unreadable. Times in the future (clock skew) count as just now.
 */
export function fmtAgo(when: number | string | null | undefined, now = Date.now()): string {
  if (when === null || when === undefined || when === "") return "";
  const ms = typeof when === "number" ? when : new Date(when).getTime();
  if (!Number.isFinite(ms)) return "";
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return t("format.justNow");
  if (s < 3600) return tn("format.minutesAgo", Math.floor(s / 60));
  if (s < 86400) return tn("format.hoursAgo", Math.floor(s / 3600));
  if (s < 2 * 86400) return t("format.yesterday");
  if (s < 7 * 86400) return tn("format.daysAgo", Math.floor(s / 86400));
  const d = new Date(ms);
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return d.toLocaleDateString(locale(), sameYear ? { month: "short", day: "numeric" } : { year: "numeric", month: "short", day: "numeric" });
}
