// The History page's charts, drawn in SVG as wide as where they're put: the
// share of the disk used over time (with the forecast ahead of it), and bars.

import { h, s } from "./dom.js";
import * as F from "./format.js";

const PAD = { top: 12, right: 12, bottom: 26, left: 46 };

/** "Nice" steps for an axis: about `n` of them over `lo`–`hi`. */
export function ticks(lo, hi, n = 4) {
  const span = hi - lo;
  if (!(span > 0)) return [lo];
  const raw = span / n;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * mag).find((x) => x >= raw) ?? 10 * mag;
  const out = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(+v.toFixed(10));
  return out;
}

/** Times along the bottom: hours for a day, days for a few, dates beyond. */
function timeTicks(t0, t1) {
  const span = t1 - t0;
  const hour = 3600;
  const day = 86400;
  const step = span <= 1.5 * day ? 4 * hour : span <= 4 * day ? day : span <= 16 * day ? 2 * day : Math.ceil(span / 6 / day) * day;
  const out = [];
  // At local midnight (or the hour), so the labels say round times.
  const start = new Date(t0 * 1000);
  if (step >= day) start.setHours(0, 0, 0, 0);
  else start.setMinutes(0, 0, 0);
  for (let t = start.getTime() / 1000; t <= t1; t += step) if (t >= t0) out.push(t);
  return { out, step };
}

const hourFmt = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit" });
const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const dateFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const calloutFmt = new Intl.DateTimeFormat(undefined, { weekday: "short", hour: "2-digit", minute: "2-digit" });

function timeLabel(t, step) {
  const d = new Date(t * 1000);
  if (step < 86400) return hourFmt.format(d);
  if (step === 86400) return dayFmt.format(d);
  return dateFmt.format(d);
}

/** At most `n` points, evenly: a chart is no wider than that anyway. */
function thin(points, n) {
  if (points.length <= n) return points;
  const every = points.length / n;
  const out = [];
  for (let i = 0; i < n; i++) out.push(points[Math.floor(i * every)]);
  out.push(points[points.length - 1]);
  return out;
}

/** Drawn again when its width changes. */
function sized(height, drawAt) {
  const host = h("div", { class: "chart", style: { height: `${height}px` } });
  let width = 0;
  new ResizeObserver(([e]) => {
    const w = Math.floor(e.contentRect.width);
    if (w > 0 && w !== width) {
      width = w;
      host.replaceChildren(...drawAt(w));
    }
  }).observe(host);
  return host;
}

/**
 * Percent of the disk used over time.
 * @param {{points: {t: number, pct: number}[], ahead: {mid: any[], low: any[], high: any[]},
 *   floorPct: number, cleanups: {ts: number, freed: number}[], total: number, line: string}} c
 */
export function usedChart(c) {
  const height = 300;
  return sized(height, (width) => {
    const points = thin(c.points, Math.max(50, width));
    const all = [...points, ...c.ahead.mid, ...c.ahead.low, ...c.ahead.high];
    const ys = all.map((p) => p.pct);
    let lo = Math.min(...ys);
    let hi = Math.max(...ys);
    // Fitted to what's shown, not from zero: a few GB of change on a disk of
    // hundreds would otherwise be a flat line.
    const pad = Math.max((hi - lo) * 0.12, 0.5);
    lo = Math.max(0, lo - pad);
    hi = Math.min(100, hi + pad);
    const t0 = points[0].t;
    const t1 = Math.max(points[points.length - 1].t, ...c.ahead.mid.map((p) => p.t));
    const x = (t) => PAD.left + ((t - t0) / Math.max(t1 - t0, 1)) * (width - PAD.left - PAD.right);
    const y = (p) => PAD.top + (1 - (p - lo) / Math.max(hi - lo, 1e-9)) * (height - PAD.top - PAD.bottom);
    const path = (ps) => ps.map((p, i) => `${i ? "L" : "M"}${x(p.t).toFixed(1)} ${y(p.pct).toFixed(1)}`).join("");
    const base = y(lo);
    const g = s("g", {});
    const fine = hi - lo < 4;
    for (const v of ticks(lo, hi)) {
      g.append(s("line", { x1: PAD.left, x2: width - PAD.right, y1: y(v), y2: y(v), class: "grid" }),
        s("text", { x: PAD.left - 8, y: y(v) + 4, class: "axis end" }, fine ? `${v.toFixed(1)}%` : `${Math.round(v)}%`));
    }
    const tt = timeTicks(t0, t1);
    for (const t of tt.out) {
      g.append(s("line", { x1: x(t), x2: x(t), y1: PAD.top, y2: height - PAD.bottom, class: "grid faint" }),
        s("text", { x: x(t), y: height - 8, class: "axis middle" }, timeLabel(t, tt.step)));
    }
    const id = `used${Math.random().toString(36).slice(2, 8)}`;
    const defs = s("defs", {},
      s("linearGradient", { id, x1: 0, y1: 0, x2: 0, y2: 1 },
        s("stop", { offset: 0, style: { "stop-color": c.line, "stop-opacity": 0.28 } }),
        s("stop", { offset: 1, style: { "stop-color": c.line, "stop-opacity": 0 } })));
    const last = points[points.length - 1];
    g.append(s("path", { d: `${path(points)}L${x(last.t)} ${base}L${x(points[0].t)} ${base}Z`, fill: `url(#${id})` }));
    // How sure the trend is: the plausible range of rates, as a band.
    if (c.ahead.mid.length) {
      const band = [...c.ahead.low, ...[...c.ahead.high].reverse()];
      g.append(s("path", { d: `${path(band)}Z`, class: "cone" }),
        s("path", { d: path(c.ahead.mid), class: "forecast-line" }));
    }
    g.append(s("path", { d: path(points), class: "used-line", style: { stroke: c.line } }));
    if (c.floorPct <= hi) {
      g.append(s("line", { x1: PAD.left, x2: width - PAD.right, y1: y(c.floorPct), y2: y(c.floorPct), class: "floor" }));
    }
    for (const step of c.cleanups) {
      const near = nearest(points, step.ts);
      if (!near) continue;
      g.append(s("circle", { cx: x(step.ts), cy: y(near.pct), r: 4.5, class: "cleanup" },
        s("title", {}, `Freed ${F.bytes(step.freed)}, ${F.when(step.ts)}`)));
    }
    // What was free at the pointer, as it moves.
    const rule = s("line", { y1: PAD.top, y2: height - PAD.bottom, class: "hover-rule", visibility: "hidden" });
    const dot = s("circle", { r: 4.5, class: "hover-dot", visibility: "hidden", style: { fill: c.line } });
    const callout = h("div", { class: "callout", hidden: true });
    const svg = s("svg", { width, height, viewBox: `0 0 ${width} ${height}`, role: "img",
      "aria-label": `Share of the disk used: ${last.pct.toFixed(1)}% now` }, defs, g, rule, dot);
    svg.addEventListener("pointermove", (e) => {
      const r = svg.getBoundingClientRect();
      const px = e.clientX - r.left;
      const t = t0 + ((px - PAD.left) / Math.max(width - PAD.left - PAD.right, 1)) * (t1 - t0);
      const p = nearest(points, t);
      if (!p || px < PAD.left) return;
      for (const el of [rule, dot]) el.setAttribute("visibility", "visible");
      rule.setAttribute("x1", x(p.t));
      rule.setAttribute("x2", x(p.t));
      dot.setAttribute("cx", x(p.t));
      dot.setAttribute("cy", y(p.pct));
      callout.hidden = false;
      callout.replaceChildren(
        h("strong", { class: "num" }, `${p.pct.toFixed(1)}% used`),
        h("span", { class: "num" }, `${F.bytes(c.total * (100 - p.pct) / 100)} free`),
        h("span", { class: "secondary" }, calloutFmt.format(new Date(p.t * 1000))));
      const left = Math.min(Math.max(x(p.t) - 70, 0), width - 150);
      callout.style.setProperty("left", `${left}px`);
    });
    svg.addEventListener("pointerleave", () => {
      for (const el of [rule, dot]) el.setAttribute("visibility", "hidden");
      callout.hidden = true;
    });
    return [svg, callout];
  });
}

function nearest(points, t) {
  let best = null;
  for (const p of points) if (!best || Math.abs(p.t - t) < Math.abs(best.t - t)) best = p;
  return best;
}

/**
 * Bars, a day each: what was written, with the day's biggest writers in its
 * tooltip; `mark` draws a line at that height (the disk's whole size).
 */
export function dayBars(days, mark) {
  const height = 180;
  return sized(height, (width) => {
    const top = Math.max(...days.map((d) => d.bytes), mark && mark < Math.max(...days.map((d) => d.bytes)) * 1.5 ? mark : 0, 1);
    const n = days.length;
    const gap = n > 40 ? 1 : 3;
    const bw = Math.max(2, (width - PAD.left - PAD.right - gap * (n - 1)) / n);
    const y = (b) => PAD.top + (1 - b / top) * (height - PAD.top - PAD.bottom);
    const g = s("g", {});
    for (const v of ticks(0, top, 3)) {
      g.append(s("line", { x1: PAD.left, x2: width - PAD.right, y1: y(v), y2: y(v), class: "grid" }),
        s("text", { x: PAD.left - 8, y: y(v) + 4, class: "axis end" }, v ? F.bytes(v) : "0"));
    }
    days.forEach((d, i) => {
      const x0 = PAD.left + i * (bw + gap);
      const tip = [`${F.day(d.day)}: ${F.bytes(d.bytes)} written`, ...d.top.slice(0, 5).map((w) => `${w.name}: ${F.bytes(w.bytes)}`)].join("\n");
      g.append(s("rect", { x: x0, y: y(d.bytes), width: bw, height: Math.max(0, y(0) - y(d.bytes)), rx: Math.min(3, bw / 3), class: "day-bar" },
        s("title", {}, tip)));
      if (i === 0 || i === n - 1 || (n <= 14 && i % 2 === 0) || (n > 14 && i % 7 === 0)) {
        g.append(s("text", { x: x0 + bw / 2, y: height - 8, class: "axis middle" }, F.day(d.day)));
      }
    });
    if (mark && mark <= top) {
      g.append(s("line", { x1: PAD.left, x2: width - PAD.right, y1: y(mark), y2: y(mark), class: "floor" },
        s("title", {}, `The disk's whole size, ${F.bytes(mark)}`)));
    }
    return [s("svg", { width, height, viewBox: `0 0 ${width} ${height}`, role: "img", "aria-label": "What was written each day" }, g)];
  });
}

/** A name and a figure, with a bar as long as its share of the most. */
export function barRow(name, value, largest, color, { detail, sign = "", hint } = {}) {
  return h("div", { class: "row bar-row", title: hint ?? null },
    h("div", { class: "row-text" },
      h("div", { class: "bar-row-head" },
        h("span", { class: "row-title" }, name),
        detail ? h("span", { class: "secondary path" }, detail) : null,
        h("span", { class: "num" }, `${sign}${F.bytes(Math.abs(value))}`)),
      h("div", { class: "bar-track" },
        h("span", { style: { width: `${Math.max(1, (Math.abs(value) / Math.max(largest, 1)) * 100)}%`, background: color } }))));
}
