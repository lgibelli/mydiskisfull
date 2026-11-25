// History, as the Mac app shows it: the share of the disk used over time,
// with the forecast ahead and the clean-ups on it, what grew and shrank the
// most; and Writes: what was written, each day and by which programs (not on
// Windows, where no program's writes are recorded).

import { onWindows } from "../core.js";
import { h } from "../dom.js";
import * as F from "../format.js";
import * as T from "../text.js";
import { barRow, dayBars, usedChart } from "../chart.js";
import { busy, change, note, page, section, segmented } from "../widgets.js";

const SPANS = [
  ["day", "24 Hours", 86400],
  ["threeDays", "3 Days", 3 * 86400],
  ["twoWeeks", "2 Weeks", 14 * 86400],
  ["all", "All", null],
];

const view = { mode: "space", span: "threeDays" };

export function history(store, act) {
  const s = store.status;
  const pick = (k) => (v) => {
    view[k] = v;
    act.show("history");
  };
  if (onWindows) view.mode = "space";
  const tools = [
    onWindows ? null : segmented("Show", [["space", "Space Used"], ["writes", "Writes"]], view.mode, pick("mode")),
    view.mode === "space" ? segmented("Span", SPANS.map(([id, t]) => [id, t]), view.span, pick("span")) : null,
  ];
  if (!s) return page("History", { tools }, section(null, {}, busy("Reading the watcher's history…")));
  return page("History", { tools }, view.mode === "space" ? spaceUsed(store) : writes(store));
}

// -- Space used ------------------------------------------------------------------

function spaceUsed(store) {
  const s = store.status;
  const d = store.detail;
  const span = SPANS.find(([id]) => id === view.span)[2];
  const now = Date.now() / 1000;
  const used = (free) => ((s.total - free) / Math.max(s.total, 1)) * 100;
  const points = (d?.series ?? [])
    .filter(([t]) => span == null || t >= now - span)
    .map(([t, free]) => ({ t, pct: used(free) }));
  const hl = T.health(s);
  const line = hl === "calm" || hl === "learning" ? "var(--brand)" : `var(--${hl === "watch" ? "orange" : "red"})`;
  const paces = s.forecast?.paces;
  const head = h("div", { class: "card padded" },
    h("h2", { class: "status-title" }, T.headline.title(s)),
    h("p", { class: "secondary" }, T.headline.detail(s)),
    paces
      ? h("div", { class: "paces", title: "How much the disk filled or freed, clean-ups left out." },
        [["Last hour", paces.hour != null ? paces.hour / 24 : null], ["Last 24 hours", paces.day],
          ["Last 7 days", paces.week != null ? paces.week * 7 : null]]
          .filter(([, b]) => b != null)
          .map(([label, b]) => h("div", { class: "pace" }, h("span", { class: "caps" }, label), change(b))))
      : null,
    !d ? busy("Reading the samples…")
      : points.length < 2
        ? note("Not enough history yet. The chart starts once the watcher has a few samples; it takes one every 5 minutes.")
        : usedChart({
          points,
          ahead: projection(s, span, used),
          floorPct: used(s.floor),
          cleanups: cleanups(s, span, now),
          total: s.total,
          line,
        }),
    d && points.length >= 2
      ? h("div", { class: "legend" },
        h("span", { class: "key used", style: { "--c": line } }, "Used"),
        projection(s, span, used).mid.length ? h("span", { class: "key forecast" }, "Forecast") : null,
        h("span", { class: "key floor" }, `Almost full (${F.bytes(s.floor)} free)`),
        cleanups(s, span, now).length ? h("span", { class: "key cleanup" }, "Clean-up") : null)
      : null);
  return [h("section", { class: "section" }, head), grew(d, view.span)];
}

/** Ahead of the last sample, at the trend's pace and its plausible range. */
function projection(s, span, used) {
  const f = s.forecast;
  const r = f?.rate;
  if (!r || r.per_day <= 0) return { mid: [], low: [], high: [] };
  const ahead = (span ?? 14 * 86400) / 3;
  const secs = Math.min(ahead, (f.full_in?.days ?? Infinity) * 86400);
  const at = (t, rate) => ({ t: f.at + t, pct: used(Math.max(0, f.avail - (rate * t) / 86400)) });
  const ts = [];
  const step = Math.max(secs / 12, 1);
  for (let t = 0; t <= secs + 1e-6; t += step) ts.push(t);
  return {
    mid: ts.map((t) => at(t, r.per_day)),
    low: ts.map((t) => at(t, Math.max(0, r.low))),
    high: ts.map((t) => at(t, Math.max(r.high, r.per_day))),
  };
}

/** Clean-ups within an hour of each other as one, so they don't pile up. */
function cleanups(s, span, now) {
  const cutoff = span == null ? 0 : now - span;
  const out = [];
  for (const st of [...(s.forecast?.steps ?? [])].filter((x) => x.ts >= cutoff).sort((a, b) => a.ts - b.ts)) {
    const last = out[out.length - 1];
    if (last && st.ts - last.ts < 3600) out[out.length - 1] = { ts: st.ts, freed: last.freed + st.freed };
    else out.push({ ...st });
  }
  return out;
}

/** What grew and shrank the most over the span the chart shows (two weeks at most). */
function grew(d, span) {
  const [rows, window] = span === "day"
    ? [d?.growth_24h ?? [], "24 Hours"]
    : span === "threeDays"
      ? [d?.growth_3d ?? d?.growth_24h ?? [], d?.growth_3d ? "3 Days" : "24 Hours"]
      : [d?.growth_14d ?? d?.growth_24h ?? [], d?.growth_14d ? "2 Weeks" : "24 Hours"];
  const up = rows.filter((g) => g.bytes >= 1_000_000).slice(0, 8);
  // Deleted, moved to the Recycle Bin, or cleaned by the program that keeps it.
  const down = rows.filter((g) => g.bytes <= -100_000_000).sort((a, b) => a.bytes - b.bytes).slice(0, 3);
  const growthRow = (g, largest, color) => barRow(F.lastName(g.owner), g.bytes, largest, color,
    { detail: parentOf(g.owner), sign: g.bytes > 0 ? "+" : "−", hint: g.owner });
  return [
    section(`Grew the Most, Last ${window}`, {},
      up.length ? up.map((g) => growthRow(g, up[0].bytes, "var(--orange)")) : note("Nothing grew much.")),
    down.length
      ? section(`Shrank the Most, Last ${window}`, {}, down.map((g) => growthRow(g, Math.abs(down[0].bytes), "var(--green)")))
      : null,
  ];
}

/** Where a folder is: its path without its own name. */
function parentOf(path) {
  const at = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/"));
  return at > 0 ? path.slice(0, at) : "";
}

// -- Writes ------------------------------------------------------------------------

function writes(store) {
  const d = store.detail;
  const total = store.status.total;
  if (!d) return section(null, {}, busy("Reading the samples…"));
  const rows = d.writes_24h ?? [];
  const written = rows.reduce((n, w) => n + w.bytes, 0);
  const times = written / Math.max(total, 1);
  const share = times >= 1
    ? `That's ${times.toFixed(1)} times the whole disk, in a day.`
    : times >= 0.05 ? `That's ${F.percent(times)} of the disk's size, in a day.`
      : "A small share of the disk's size: nothing to worry about.";
  const days = (d.writes_daily ?? []).slice(-30);
  return [
    section(null, {
      footer: "Writing doesn't fill the disk: a program can rewrite the same files all day. But an SSD takes only so much writing in its life, so a program that writes a lot wears it out. One that keeps rewriting the disk shows on Overview, with its fix.",
    }, h("div", { class: "row" }, h("div", { class: "row-text" },
      h("div", { class: "row-title strong" }, `${F.bytes(written)} written in the last 24 hours`),
      h("div", { class: "row-sub" }, share)))),
    days.length >= 2
      ? section("Written Each Day", {}, h("div", { class: "row chart-row" }, dayBars(days, total)))
      : null,
    section("Wrote the Most, Last 24 Hours", {},
      rows.length
        ? rows.slice(0, 15).map((w) => barRow(w.name, w.bytes, rows[0].bytes, "var(--purple)"))
        : note("Nothing yet: the watcher counts writes from its next samples.")),
  ];
}
