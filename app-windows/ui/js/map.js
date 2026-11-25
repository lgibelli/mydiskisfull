// The disk map drawn two ways, as the Mac app's MapCharts.swift draws it: a
// sunburst (rings out from the folder in focus) and a treemap (tiles sized by
// space, coloured like a market heatmap by how much each folder grew or
// shrank in the last day), and the layouts they're drawn from, which the
// tests check. A folder is clicked to zoom in; the centre goes back out.
// Sizes are `mdif map`'s private bytes: what deleting would free.

import { s } from "./dom.js";
import * as F from "./format.js";

/** The rings the sunburst shows, out from the folder in focus. */
export const RINGS = 4;

// -- Paths -------------------------------------------------------------------------

/** A Windows path: a drive's (`C:\…`) or a share's (`\\server\…`). */
const windowsPath = (p) => /^[A-Za-z]:[\\/]/.test(p) || p.startsWith("\\\\");

/** How paths compare: Windows' whatever their case, and however they end. */
const fold = (p) => (windowsPath(p) ? p.replace(/\//g, "\\").replace(/\\+$/, "").toLowerCase() : p);

/** The folder `path` is in; null for a drive's root, a share or `/`. */
export function parentOf(path) {
  if (windowsPath(path)) {
    const t = path.replace(/[\\/]+$/, "");
    const cut = Math.max(t.lastIndexOf("\\"), t.lastIndexOf("/"));
    if (cut < 0) return null;
    // `C:\Users` is in the drive's root, `C:\`.
    if (cut === 2 && t[1] === ":") return t.slice(0, 3);
    // As far up as `\\server\share`.
    if (t.startsWith("\\\\") && cut <= t.indexOf("\\", 2)) return null;
    return t.slice(0, cut);
  }
  if (!path.startsWith("/") || path === "/") return null;
  const cut = path.replace(/\/+$/, "").lastIndexOf("/");
  return cut <= 0 ? "/" : path.slice(0, cut);
}

/** The path as the user knows it: the Mac's Data volume is `/`. */
export function userPath(path) {
  const data = "/System/Volumes/Data";
  if (path === data) return "/";
  return path.startsWith(`${data}/`) ? path.slice(data.length) : path;
}

/** A folder's name: the disk's (`disk`) for its root. */
export function nodeName(node, disk) {
  const p = userPath(node.path);
  if (p === "/" || (windowsPath(p) && parentOf(p) == null)) return disk ?? p;
  return F.lastName(p);
}

/** Whether a folder has folders of its own shown in the map. */
export const opens = (node) => Boolean(node?.children?.length);

/**
 * The folders from `root` down to the one at `path` (root first), as far as
 * the map goes: a breadcrumb kept across measurements stops where a folder
 * no longer is.
 */
export function chain(root, path) {
  const out = [root];
  const want = fold(userPath(path ?? root.path));
  let at = root;
  while (fold(userPath(at.path)) !== want) {
    const next = (at.children ?? []).find((c) => within(want, fold(userPath(c.path))));
    if (!next) break;
    out.push(next);
    at = next;
  }
  return out;
}

/** Whether the folded path `p` is `dir` or inside it. */
function within(p, dir) {
  if (p === dir) return true;
  const sep = windowsPath(dir) || /^[a-z]:$/i.test(dir) ? "\\" : "/";
  return p.startsWith(dir.endsWith(sep) ? dir : dir + sep);
}

// -- Growth ------------------------------------------------------------------------

/**
 * Growth in the last 24 hours, summed over the watcher's folders at or below
 * each path: every ancestor gets the growth of what's below it.
 */
export function growthIndex(growth24h) {
  const g = new Map();
  for (const row of growth24h ?? []) {
    for (let p = row.owner; p != null; p = parentOf(p)) {
      const k = fold(p);
      g.set(k, (g.get(k) ?? 0) + row.bytes);
    }
  }
  return g;
}

/** A folder's growth in the index, if the watcher saw any. */
export const growthOf = (index, node) => (node ? index.get(fold(userPath(node.path))) : undefined);

// -- Colours -----------------------------------------------------------------------

/** The Mac's `Color(hue:saturation:brightness:)`, as CSS. */
function hsb(h, sat, bri) {
  const l = bri * (1 - sat / 2);
  const sl = l === 0 || l === 1 ? 0 : (bri - l) / Math.min(l, 1 - l);
  return `hsl(${(h * 360).toFixed(1)} ${(sl * 100).toFixed(1)}% ${(l * 100).toFixed(1)}%)`;
}

const rgb = (r, g, b) => `rgb(${Math.round(r * 255)} ${Math.round(g * 255)} ${Math.round(b * 255)})`;

/**
 * Top-level folders take these in turn, starting from the brand's blue; none
 * is red, which means growth in the other colouring.
 */
const PALETTE = [0.62, 0.50, 0.76, 0.90, 0.09, 0.40, 0.68, 0.55, 0.14, 0.83];
export const hue = (i) => PALETTE[i % PALETTE.length];

/**
 * By folder: its top-level folder's hue, strong in the middle and lighter
 * towards the edge. By growth: red for what grew, green for what shrank,
 * grey for steady or not watched, stronger with more bytes (10 MB faint,
 * 10 GB full). `node` null: what's too small to show on its own.
 */
export function colour(node, mode, h, depth, growth) {
  if (!node) return "rgb(142 142 147 / 0.25)";
  if (mode === "folder") {
    const d = depth - 1;
    return hsb(h, Math.max(0.28, 0.62 - 0.1 * d), Math.min(0.96, 0.84 + 0.04 * d));
  }
  if (growth == null || Math.abs(growth) < 10_000_000) {
    const w = 0.42 - 0.04 * Math.min(depth, 4);
    return rgb(w, w, w);
  }
  const t = Math.min(1, Math.log10(Math.abs(growth) / 1e7) / 3);
  return growth > 0
    ? rgb(0.45 + 0.45 * t, 0.30 - 0.12 * t, 0.30 - 0.12 * t)
    : rgb(0.22 - 0.1 * t, 0.45 + 0.35 * t, 0.30 - 0.05 * t);
}

// -- Sunburst ----------------------------------------------------------------------

/**
 * The sunburst's wedges: each folder a part of its parent's, as wide as its
 * share, out to `rings`; what's too small to show on its own (files, small
 * folders) one grey wedge at the end (`node` null). Angles in radians,
 * clockwise from 12 o'clock.
 */
export function segments(focus, rings = RINGS) {
  const out = [];
  const add = (node, depth, start, end, h) => {
    const total = Math.max(node.totals.private, 1);
    let at = start;
    (node.children ?? []).forEach((child, i) => {
      const span = ((end - start) * child.totals.private) / total;
      if (span <= 0.004) return;
      const own = h ?? hue(i);
      out.push({ node: child, depth, start: at, end: at + span, hue: own });
      if (depth < rings) add(child, depth + 1, at, at + span, own);
      at += span;
    });
    if (end - at > 0.01) out.push({ node: null, depth, start: at, end, hue: 0 });
  };
  add(focus, 1, 0, 2 * Math.PI, null);
  return out;
}

/**
 * How wide a level name can be in a wedge `span` radians wide at radius `r`
 * on a ring `ring` thick, centred at angle `mid`: along the arc at 12 and 6
 * o'clock, across the ring at 3 and 9 (a level line crosses it over
 * ring / |sin|). None on an arc 46 points long or shorter, as on the Mac.
 */
export function labelRoom(span, r, ring, mid) {
  const arc = span * r;
  if (arc <= 46) return 0;
  return Math.min(arc, (ring - 6) / Math.max(Math.abs(Math.sin(mid)), 0.05)) - 6;
}

/** An annular sector, as a polygon: no arc flags to get wrong, and a whole ring works. */
export function wedge(cx, cy, r0, r1, a0, a1) {
  const steps = Math.max(2, Math.floor((a1 - a0) / 0.03));
  const at = (r, a) => `${(cx + r * Math.sin(a)).toFixed(2)} ${(cy - r * Math.cos(a)).toFixed(2)}`;
  const outer = [];
  const inner = [];
  for (let k = 0; k <= steps; k++) outer.push(at(r1, a0 + ((a1 - a0) * k) / steps));
  for (let k = steps; k >= 0; k--) inner.push(at(r0, a0 + ((a1 - a0) * k) / steps));
  return `M${outer.join("L")}L${inner.join("L")}Z`;
}

/**
 * The sunburst of `focus`, `width` by `height`. `name` names a folder,
 * `growth` gives its growth, `hover` is told what's under the pointer (null
 * when nothing is), `open` a folder clicked, `back` the centre clicked.
 */
export function sunburst({ focus, width, height, colouring, growth, name, hover, open, back, canBack }) {
  const cx = width / 2;
  const cy = height / 2;
  const radius = Math.min(width, height) / 2 - 4;
  const core = radius * 0.24;
  const ring = (radius - core) / RINGS;
  const svg = s("svg", {
    class: "map-chart", width, height, viewBox: `0 0 ${width} ${height}`, role: "group",
    "aria-label": `${name(focus)}: its folders as rings, the biggest the widest`,
    onmouseleave: () => hover(null),
  });
  const names = [];
  for (const sg of segments(focus)) {
    const inner = core + ring * (sg.depth - 1);
    const fill = colour(sg.node, colouring, sg.hue, sg.depth, growth(sg.node));
    svg.append(s("path", {
      class: `seg${sg.node && opens(sg.node) ? " opens" : ""}`,
      d: wedge(cx, cy, inner + 0.5, inner + ring - 0.5, sg.start, sg.end),
      fill,
      onmouseenter: () => hover(sg.node), onmouseleave: () => hover(null),
      onclick: sg.node ? () => open(sg.node) : null,
    }, sg.node ? s("title", {}, `${name(sg.node)}: ${F.bytes(sg.node.totals.private)}`) : null));
    // Names where they fit: on the first two rings, on wide arcs, cut to
    // what a level line has of the wedge there.
    const mid = (sg.start + sg.end) / 2;
    const r = inner + ring / 2;
    const room = labelRoom(sg.end - sg.start, r, ring, mid);
    if (sg.node && sg.depth <= 2 && room >= 24) {
      names.push(s("text", {
        class: "seg-name", x: (cx + r * Math.sin(mid)).toFixed(1), y: (cy - r * Math.cos(mid)).toFixed(1),
      }, fit(name(sg.node), room, 10)));
    }
  }
  svg.append(...names);
  svg.append(
    s("circle", {
      class: `core${canBack ? " back" : ""}`, cx, cy, r: core,
      onmouseenter: () => hover(null), onclick: canBack ? back : null,
    }, s("title", {}, canBack ? "Back out" : name(focus))),
    s("text", { class: "core-name", x: cx, y: cy - 9 }, name(focus)),
    s("text", { class: "core-size", x: cx, y: cy + 10 }, F.bytes(focus.totals.private)));
  return svg;
}

// -- Treemap -----------------------------------------------------------------------

/**
 * Squarified treemap layout (Bruls, Huizing, van Wijk): rows along the
 * shorter side, each row kept while it makes its tiles squarer. `items` are
 * [what, size]; gives [what, rect], biggest first.
 */
export function squarify(items, rect) {
  const sorted = [...items].sort((a, b) => b[1] - a[1]);
  const total = sorted.reduce((n, [, v]) => n + v, 0);
  if (!(total > 0) || !(rect.w > 0) || !(rect.h > 0)) return [];
  const scale = (rect.w * rect.h) / total;
  const queue = sorted.map(([t, v]) => [t, v * scale]);
  const out = [];
  let rest = { ...rect };
  let row = [];
  const sum = (r) => r.reduce((n, [, a]) => n + a, 0);
  const worst = (r, side) => {
    const total = sum(r);
    if (!r.length || !(total > 0)) return Infinity;
    const areas = r.map(([, a]) => a);
    const mx = Math.max(...areas);
    const mn = Math.min(...areas);
    return Math.max((side * side * mx) / (total * total), (total * total) / (side * side * mn));
  };
  const place = (r) => {
    const total = sum(r);
    if (rest.w >= rest.h) {
      const w = total / rest.h;
      let y = rest.y;
      for (const [t, a] of r) {
        out.push([t, { x: rest.x, y, w, h: a / w }]);
        y += a / w;
      }
      rest = { x: rest.x + w, y: rest.y, w: Math.max(0, rest.w - w), h: rest.h };
    } else {
      const h = total / rest.w;
      let x = rest.x;
      for (const [t, a] of r) {
        out.push([t, { x, y: rest.y, w: a / h, h }]);
        x += a / h;
      }
      rest = { x: rest.x, y: rest.y + h, w: rest.w, h: Math.max(0, rest.h - h) };
    }
  };
  let i = 0;
  while (i < queue.length && rest.w > 0 && rest.h > 0) {
    const side = Math.min(rest.w, rest.h);
    if (!row.length || worst([...row, queue[i]], side) <= worst(row, side)) {
      row.push(queue[i]);
      i += 1;
    } else {
      place(row);
      row = [];
    }
  }
  if (row.length && rest.w > 0 && rest.h > 0) place(row);
  return out;
}

/** A folder's folders by size, and what's left (files, small folders) as one item. */
function items(node) {
  const list = (node.children ?? []).map((c) => [c, c.totals.private]);
  const rest = node.totals.private - list.reduce((n, [, v]) => n + v, 0);
  if (rest > node.totals.private * 0.01) list.push([null, rest]);
  return list.filter(([, v]) => v > 0);
}

/**
 * The treemap's tiles: the folders in `focus` (depth 1, a hue each), and one
 * level inside each tile big enough to show it (depth 2, under a strip for
 * its name). `group` is the top-level tile a tile sits in.
 */
export function tiles(focus, rect) {
  const out = [];
  squarify(items(focus), rect).forEach(([node, r], i) => {
    const h = hue(i);
    out.push({ node, rect: r, depth: 1, hue: h, group: node });
    if (!opens(node) || r.w <= 90 || r.h <= 60) return;
    const inner = { x: r.x + 2, y: r.y + 17, w: r.w - 4, h: r.h - 19 };
    for (const [sub, sr] of squarify(items(node), inner)) {
      out.push({ node: sub, rect: sr, depth: 2, hue: h, group: node });
    }
  });
  return out;
}

/**
 * What the sizes are, and what the map doesn't show; then what it isn't
 * exact about, in `mdif map`'s words (Windows). `windows`: said of a PC.
 */
export function caption(map, windows) {
  const t = map.scan.root.totals;
  const parts = ["Sizes are what deleting would free"];
  if (map.scan.shared_once >= 1_000_000_000) {
    parts.push(windows
      ? `${F.bytes(map.scan.shared_once)} more is in files linked from elsewhere (hard links), counted once`
      : `${F.bytes(map.scan.shared_once)} more is shared by clones and snapshots`);
  }
  if (t.dataless_logical >= 1_000_000_000) {
    parts.push(`${F.bytes(t.dataless_logical)} is in the cloud only, not on this ${windows ? "PC" : "Mac"}`);
  }
  return [`${parts.join(". ")}. Nothing is opened, and cloud files are never downloaded.`, ...(map.notes ?? [])];
}

/**
 * `text` cut to about what `width` holds at `size`: about 0.55 em a
 * character, `em` for bold.
 */
export function fit(text, width, size, em = 0.55) {
  const fits = Math.max(3, Math.floor(width / (size * em)));
  return text.length > fits ? `${text.slice(0, fits - 1)}…` : text;
}

/** The treemap of `focus`, as `sunburst` takes it. */
export function treemap({ focus, width, height, colouring, growth, name, hover, open }) {
  const svg = s("svg", {
    class: "map-chart", width, height, viewBox: `0 0 ${width} ${height}`, role: "group",
    "aria-label": `${name(focus)}: its folders as tiles, the biggest the largest`,
    onmouseleave: () => hover(null),
  });
  for (const t of tiles(focus, { x: 0, y: 0, w: width, h: height })) {
    const r = { x: t.rect.x + 1, y: t.rect.y + 1, w: t.rect.w - 2, h: t.rect.h - 2 };
    if (r.w <= 1 || r.h <= 1) continue;
    const n = t.node;
    // A folder with folders inside opens; one without opens its group.
    const click = !n ? null : opens(n) ? () => open(n) : t.group && t.group !== n ? () => open(t.group) : null;
    const tip = n ? s("title", {}, `${name(n)}: ${F.bytes(n.totals.private)}`) : null;
    if (t.depth === 1 && opens(n) && r.w > 90 && r.h > 60) {
      // A group: a strip with its name over its own tiles, as a sector of a
      // market heatmap.
      svg.append(
        s("rect", {
          class: "tile group opens", x: r.x, y: r.y, width: r.w, height: r.h, rx: 5,
          onmouseenter: () => hover(n), onmouseleave: () => hover(null), onclick: click,
        }, tip),
        s("text", { class: "tile-head", x: r.x + 4, y: r.y + 11 },
          fit(`${name(n)}  ${F.bytes(n.totals.private)}`, r.w - 8, 10, 0.62)));
      continue;
    }
    svg.append(s("rect", {
      class: `tile${click ? " opens" : ""}`, x: r.x, y: r.y, width: r.w, height: r.h, rx: 4,
      fill: colour(n, colouring, t.hue, t.depth, growth(n)),
      onmouseenter: () => hover(n), onmouseleave: () => hover(null), onclick: click,
    }, tip));
    if (n) svg.append(...tileLabel(n, r, growth(n), name));
  }
  return svg;
}

/** A tile's name, size and (where it fits) growth, as big as the tile allows. */
function tileLabel(n, r, g, name) {
  if (r.w <= 54 || r.h <= 28) return [];
  const big = Math.min(Math.max(10, Math.min(r.w / 8, r.h / 3.2)), 22);
  const lines = [[name(n), big, "name"], [F.bytes(n.totals.private), big * 0.72, "size"]];
  if (g != null && Math.abs(g) >= 10_000_000 && r.h > 3.6 * big) {
    lines.push([`${g > 0 ? "+" : "−"}${F.bytes(Math.abs(g))} in 24 h`, big * 0.62, "size"]);
  }
  const height = lines.reduce((sum, [, size]) => sum + size * 1.25, 0);
  let y = r.y + r.h / 2 - height / 2;
  return lines.map(([text, size, cls]) => {
    const el = s("text", {
      class: `tile-${cls}`, x: (r.x + r.w / 2).toFixed(1), y: (y + size * 0.62).toFixed(1),
      style: { "font-size": `${size.toFixed(1)}px` },
    }, fit(text, r.w - 6, size, cls === "name" ? 0.62 : 0.55));
    y += size * 1.25;
    return el;
  });
}
