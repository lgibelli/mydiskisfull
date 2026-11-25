// The disk as one bar split into coloured parts, a kind each, then what's
// free, labelled like stations on a railway map: a station on each part's
// edge and a line, in its colour, to its name and size, above and below in
// turn, moved aside where they'd overlap, the lines bending at 45° and never
// crossing. A part or its name can be clicked: it opens the kind. The Mac
// app's RailBar (Theme.swift), drawn in SVG: the same sizes, the same layout.

import { h, s } from "./dom.js";
import * as F from "./format.js";

const BAR = 22;
const LABEL = 30;
const BAND = 36;
const GAP = 1.5;
const TOP = LABEL + BAND;
const BOTTOM = TOP + BAR;
const HEIGHT = LABEL * 2 + BAND * 2 + BAR;

// As style.css draws the names.
const FONT = '"Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif';
const TITLE_FONT = `600 12px ${FONT}`;
const SIZE_FONT = `11px ${FONT}`;

let measurer;
/** As wide as its name or its size, whichever is wider. */
function textWidth(stop) {
  measurer ??= document.createElement("canvas").getContext("2d");
  measurer.font = TITLE_FONT;
  const title = measurer.measureText(stop.title).width;
  measurer.font = SIZE_FONT;
  const size = measurer.measureText(F.bytes(stop.bytes)).width;
  return Math.ceil(Math.max(title, size)) + 2;
}

/**
 * The parts, as wide as their share (the last one, what's free, takes
 * what's left), at least 2 points each; then the names, above and below in
 * turn, each side spread on its own.
 */
export function layout(stops, total, width, measure = textWidth) {
  const spans = [];
  const room = width - GAP * Math.max(stops.length - 1, 0);
  let x = 0;
  stops.forEach((st, i) => {
    const w = i === stops.length - 1
      ? Math.max(2, width - x)
      : Math.max(2, (room * st.bytes) / Math.max(total, 1));
    spans.push([x, Math.min(width, x + w)]);
    x += w + GAP;
  });
  const labels = [];
  for (const side of [0, 1]) {
    const placed = stops
      .map((_, i) => i)
      .filter((i) => i % 2 === side)
      .map((i) => {
        const c = (spans[i][0] + spans[i][1]) / 2;
        return { stop: i, up: side === 0, center: c, width: measure(stops[i]), x: c, level: 0 };
      });
    spread(placed, width);
    // A label moved a point or two goes straight over its station.
    for (const p of placed) if (Math.abs(p.x - p.center) < 3) p.x = p.center;
    labels.push(...placed);
  }
  return { spans, labels };
}

/**
 * Moves labels apart so none overlaps the next, and keeps them inside the
 * width; then gives each moved one the height its line turns at, so lines
 * going the same way never cross.
 */
function spread(ls, width) {
  const space = 10;
  ls.sort((a, b) => a.center - b.center);
  for (let i = 0; i < ls.length; i++) {
    let x = Math.max(ls[i].x, ls[i].width / 2);
    if (i > 0) x = Math.max(x, ls[i - 1].x + (ls[i - 1].width + ls[i].width) / 2 + space);
    ls[i].x = x;
  }
  for (let i = ls.length - 1; i >= 0; i--) {
    let x = Math.min(ls[i].x, width - ls[i].width / 2);
    if (i < ls.length - 1) x = Math.min(x, ls[i + 1].x - (ls[i].width + ls[i + 1].width) / 2 - space);
    ls[i].x = x;
  }
  // Moved right: the rightmost of a run turns nearest the bar; moved left,
  // the leftmost.
  let i = 0;
  while (i < ls.length) {
    const dir = ls[i].x - ls[i].center;
    if (Math.abs(dir) < 1) {
      i++;
      continue;
    }
    let j = i;
    while (
      j + 1 < ls.length
      && (ls[j + 1].x - ls[j + 1].center) * dir > 0
      && Math.abs(ls[j + 1].x - ls[j + 1].center) >= 1
    ) j++;
    for (let k = i; k <= j; k++) ls[k].level = (dir > 0 ? j - k : k - i) % 3;
    i = j + 1;
  }
}

/**
 * From the station at `from` on the bar's `edge` to the label at `to` (its
 * near edge at `end`): straight, or up (down), across at its level's height
 * and up (down) again, the corners cut at 45°.
 */
export function leader(from, to, edge, end, level, up) {
  const sg = up ? -1 : 1;
  const dx = to - from;
  if (Math.abs(dx) < 1) return `M${from} ${edge}L${from} ${end}`;
  const c = Math.min(6, Math.abs(dx) / 2);
  const hd = dx > 0 ? 1 : -1;
  const run = edge + sg * (8 + level * 6 + c);
  return [
    `M${from} ${edge}`,
    `L${from} ${run - sg * c}`,
    `L${from + hd * c} ${run}`,
    `L${to - hd * c} ${run}`,
    `L${to} ${run + sg * c}`,
    `L${to} ${end}`,
  ].join("");
}

let drawn = 0;

function draw(stops, total, width, onSelect) {
  const l = layout(stops, total, width);
  const id = `rail${++drawn}`;
  const defs = s("defs", {},
    s("clipPath", { id: `${id}-bar` },
      s("rect", { x: 0, y: TOP, width, height: BAR, rx: 6 })));
  const bar = s("g", { "clip-path": `url(#${id}-bar)` });
  stops.forEach((st, i) => {
    const [x0, x1] = l.spans[i];
    const w = Math.max(0, x1 - x0);
    defs.append(s("linearGradient", { id: `${id}-g${i}`, x1: 0, y1: 0, x2: 0, y2: 1 },
      s("stop", { offset: 0, style: { "stop-color": st.color, "stop-opacity": 0.85 } }),
      s("stop", { offset: 1, style: { "stop-color": st.color, "stop-opacity": 1 } })));
    bar.append(s("rect", { x: x0, y: TOP, width: w, height: BAR, fill: `url(#${id}-g${i})` }));
    if (st.hatched) {
      defs.append(s("clipPath", { id: `${id}-p${i}` }, s("rect", { x: x0, y: TOP, width: w, height: BAR })));
      let d = "";
      for (let x = x0 - BAR; x < x1; x += 7) d += `M${x} ${BOTTOM}L${x + BAR} ${TOP}`;
      bar.append(s("path", {
        d, class: "rail-hatch", "clip-path": `url(#${id}-p${i})`,
      }));
    }
  });
  const lines = s("g", {});
  for (const p of l.labels) {
    const st = stops[p.stop];
    const edge = p.up ? TOP : BOTTOM;
    const end = p.up ? LABEL + 2 : BOTTOM + BAND - 2;
    const color = st.muted ? "var(--muted-line)" : st.color;
    lines.append(s("path", {
      d: leader(p.center, p.x, edge, end, p.level, p.up),
      class: "rail-line", style: { stroke: color },
    }));
    // The station, on the bar's edge.
    lines.append(s("circle", { cx: p.center, cy: edge, r: 4, class: "rail-station", style: { stroke: color } }));
  }
  const hits = s("g", {});
  const said = (st) => `${st.title}: ${F.bytes(st.bytes)}${st.id ? ". Click to see what's in it." : ""}`;
  const select = (st) => (st.id ? () => onSelect?.(st.id) : null);
  const hover = (i, on) => () => {
    for (const el of svg.querySelectorAll(`[data-stop="${i}"]`)) el.classList.toggle("on", on);
  };
  const clickable = (el, st, i) => {
    el.setAttribute("data-stop", i);
    el.append(s("title", {}, said(st)));
    el.addEventListener("mouseenter", hover(i, true));
    el.addEventListener("mouseleave", hover(i, false));
    if (!st.id) return el;
    el.classList.add("hit");
    el.setAttribute("role", "button");
    el.setAttribute("tabindex", "0");
    el.setAttribute("aria-label", `${st.title}, ${F.bytes(st.bytes)}`);
    el.addEventListener("click", select(st));
    el.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        select(st)();
      }
    });
    return el;
  };
  stops.forEach((st, i) => {
    const [x0, x1] = l.spans[i];
    const g = s("g", { class: "rail-part" },
      s("rect", { x: x0, y: TOP, width: Math.max(2, x1 - x0), height: BAR, class: "rail-hit-area" }));
    hits.append(clickable(g, st, i));
  });
  for (const p of l.labels) {
    const st = stops[p.stop];
    const top = p.up ? 2 : BOTTOM + BAND - 2;
    const g = s("g", { class: `rail-label${st.muted ? " muted" : ""}` },
      s("rect", { x: p.x - p.width / 2, y: top, width: p.width, height: 30, class: "rail-hit-area" }),
      s("text", { x: p.x, y: top + 12, class: "rail-title" }, st.title),
      s("text", { x: p.x, y: top + 26, class: "rail-size" }, F.bytes(st.bytes)));
    hits.append(clickable(g, st, p.stop));
  }
  const svg = s("svg", {
    class: "rail", width, height: HEIGHT, viewBox: `0 0 ${width} ${HEIGHT}`,
    role: "group", "aria-label": "What the disk holds",
  }, defs, bar, lines, hits);
  return svg;
}

/**
 * The bar, as wide as where it's put (drawn again when that changes).
 * @param {{id: string|null, title: string, bytes: number, color: string, muted?: boolean, hatched?: boolean}[]} stops
 * @param {number} total
 * @param {(id: string) => void} [onSelect]
 */
export function railBar(stops, total, onSelect) {
  const host = h("div", { class: "rail-host", style: { height: `${HEIGHT}px` } });
  let width = 0;
  new ResizeObserver(([e]) => {
    const w = Math.floor(e.contentRect.width);
    if (w > 0 && w !== width) {
      width = w;
      host.replaceChildren(draw(stops, total, w, onSelect));
    }
  }).observe(host);
  return host;
}
