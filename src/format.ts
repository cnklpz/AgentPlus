// Numbers, sizes, durations and relative times, formatted for the UI language.
import { locale, t, tn } from "./i18n";

const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;

/** `n` with exactly `digits` decimals, in the UI language's number format ("1,234.5"). */
export const fmtNum = (n: number, digits: number) =>
  n.toLocaleString(locale(), { minimumFractionDigits: digits, maximumFractionDigits: digits });

/** Milliseconds as seconds, without a unit: "1.2", "12.35". */
export const fmtSecs = (ms: number, digits = 1) => fmtNum(ms / 1000, digits);

/** "a、b、c" / "a, b, c". */
export const joinList = (items: readonly string[]) => items.join(t("common.listSep"));

/** "0 KB", "1 KB" (anything from 1 byte up), "12 KB", "3.4 MB", "1.2 GB". */
export function fmtSize(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 KB";
  if (bytes >= GB) return `${fmtNum(bytes / GB, 1)} GB`;
  if (bytes >= MB) return `${fmtNum(bytes / MB, 1)} MB`;
  return `${fmtNum(Math.max(1, Math.round(bytes / KB)), 0)} KB`;
}

/**
 * "刚刚" / "3 分钟前" / "昨天" / "9月20日" for a time in ms (or an ISO string); "" when missing
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
