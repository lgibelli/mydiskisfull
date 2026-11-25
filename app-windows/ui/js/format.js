// Numbers and dates the way the command line prints them: the Mac app's
// Format.swift, word for word.

/** Decimal units, like `mdif`: "24.8 GB", "23 MB". */
export function bytes(n) {
  const v = Math.abs(n);
  const sign = n < 0 ? "-" : "";
  if (v >= 1e9) return `${sign}${(v / 1e9).toFixed(1)} GB`;
  if (v >= 1e6) return `${sign}${Math.round(v / 1e6)} MB`;
  if (v >= 1e3) return `${sign}${Math.round(v / 1e3)} KB`;
  return `${sign}${Math.trunc(v)} B`;
}

/** "12 days", "5 hours", "3 months". */
export function days(d) {
  let n;
  let unit;
  if (d < 1) [n, unit] = [Math.max(1, Math.round(d * 24)), "hour"];
  else if (d < 100) [n, unit] = [Math.round(d), "day"];
  else [n, unit] = [Math.round(d / 30.4), "month"];
  return n === 1 ? `1 ${unit}` : `${n} ${unit}s`;
}

const fmt = (options) => new Intl.DateTimeFormat(undefined, options);
const short = fmt({ month: "short", day: "numeric" });
const shortYear = fmt({ month: "short", year: "numeric" });
const weekdayTime = fmt({ weekday: "short", hour: "2-digit", minute: "2-digit" });
const dayTime = fmt({ month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
const clock = fmt({ hour: "2-digit", minute: "2-digit" });

/** The day `inDays` from `at`: "Oct 31", or "Tue 14:00" when close. */
export function date(at, inDays) {
  const when = new Date((at + inDays * 86400) * 1000);
  return (inDays < 2 ? weekdayTime : inDays > 300 ? shortYear : short).format(when);
}

/** "Oct 25 13:40". */
export const when = (ts) => dayTime.format(new Date(ts * 1000));

/** "Oct 25". */
export const day = (ts) => short.format(new Date(ts * 1000));

/** "13:40", in the person's format. */
export const time = (ts) => clock.format(new Date(ts * 1000));

/** "4 min", "3 hours", "2 days". */
export function duration(secs) {
  if (secs >= 2 * 86400) return `${Math.floor(secs / 86400)} days`;
  if (secs >= 2 * 3600) return `${Math.floor(secs / 3600)} hours`;
  if (secs >= 120) return `${Math.floor(secs / 60)} min`;
  return `${Math.max(Math.floor(secs), 0)} s`;
}

/** How old: "5 hours", "12 days", "3 months", "2 years". */
export function age(secs) {
  const d = Math.floor(secs / 86400);
  if (d >= 730) return `${Math.floor(d / 365)} years`;
  if (d >= 60) return `${Math.floor(d / 30)} months`;
  if (d >= 2) return `${d} days`;
  return duration(secs);
}

/** "just now", "4 min ago", "3 hours ago". */
export function ago(ts, now = Date.now() / 1000) {
  const secs = now - ts;
  return secs < 60 ? "just now" : `${duration(secs)} ago`;
}

/** "3-day" or "6-hour", for a trend window. */
export function trendWindow(secs) {
  return secs >= 86400 ? `${Math.floor(secs / 86400)}-day` : `${Math.floor(secs / 3600)}-hour`;
}

/** "43%". */
export const percent = (p) => `${Math.round(p * 100)}%`;

/** A net change in what's used: "Filled 3.4 GB", "Freed 2.9 GB", "Quiet" under 50 MB. */
export function change(b) {
  if (Math.abs(b) < 50_000_000) return "Quiet";
  return (b > 0 ? "Filled " : "Freed ") + bytes(Math.abs(b));
}

/** The last name in a path, Windows' or a Mac's. */
export function lastName(path) {
  const names = path.split(/[\\/]/).filter(Boolean);
  return names.length ? names[names.length - 1] : path;
}
