// Storage's Folders view, the disk map, as the Mac app's MapView.swift: where
// the space is now, folder by folder, from `mdif map` (private sizes: what
// deleting would free). A whole-disk scan reads every folder's names and
// sizes, never what's in a file, so it never runs in the background: only
// while someone is looking (app.js: when the view opens and the kept map is
// over 30 minutes old, every hour while it stays on screen, and when asked).
// The kept map shows meanwhile.

import { onWindows } from "../core.js";
import { h, icon } from "../dom.js";
import * as F from "../format.js";
import * as M from "../map.js";
import * as T from "../text.js";
import { button, segmented } from "../widgets.js";

/** The map's explorer; before the first map, what it is and the button. */
export function folders(store, act) {
  return store.map.data ? explorer(store, act) : emptyMap(store, act);
}

/** Before the first map: what it is, what it costs, and the button. */
function emptyMap(store, act) {
  const m = store.map;
  return h("section", { class: "section" },
    h("div", { class: "card empty-map" },
      h("span", { class: "empty-glyph" }, icon("storage", 34)),
      h("h2", {}, "Map your disk folder by folder"),
      h("p", { class: "secondary" },
        "It reads every folder's size (never what's in a file) and takes a minute or so. It runs again by "
          + "itself while this view is open."),
      m.running
        ? h("div", { class: "measuring", role: "status" },
          h("div", { class: "progress", "aria-hidden": "true" }, h("span")),
          h("span", { class: "secondary num", dataset: { elapsed: m.started == null ? "" : String(m.started) } },
            measuringText(m.started)))
        : button("Measure Now", () => act.measureMap(false), { kind: "accent", focus: "map:measure" }),
      m.error ? h("p", { class: "warn-text" }, icon("warning", 16), m.error) : null));
}

/**
 * "Measuring… 12 s", since `started` (seconds; app.js keeps it ticking), or
 * waiting for the categories' measurement to end first.
 */
export function measuringText(started, now = Date.now() / 1000) {
  if (started == null) return "Waiting for Categories to finish measuring…";
  return `Measuring… ${Math.max(0, Math.floor(now - started))} s`;
}

/** The explorer: the chart's style and colouring, the breadcrumb, what's under the pointer, the chart. */
function explorer(store, act) {
  const map = store.map.data;
  const v = store.mapView;
  const trail = M.chain(map.scan.root, v.focus);
  const focus = trail[trail.length - 1];
  const disk = T.diskName(map.root);
  const name = (n) => M.nodeName(n, disk);
  const index = M.growthIndex(store.detail?.growth_24h);
  const growth = (n) => M.growthOf(index, n);
  const open = (n) => {
    if (M.opens(n) && n !== focus) act.mapFocus(n.path);
  };
  // What's under the pointer, or the folder in focus: said without drawing
  // the page again.
  const info = h("div", { class: "map-info", "aria-live": "polite" });
  const say = (n) => info.replaceChildren(...infoLine(n ?? focus, focus, growth, name));
  say(null);
  const chart = { focus, colouring: v.colouring, growth, name, hover: say, open };
  return [
    h("section", { class: "section" },
      h("div", { class: "card padded map-card" },
        h("div", { class: "map-controls" },
          segmented("Chart", [["sunburst", "Sunburst"], ["treemap", "Treemap"], ["list", "List"]], v.style, act.mapStyle),
          v.style === "list" ? null : colourPicker(v.colouring, act.mapColouring),
          h("span", { class: "grow" }),
          button("Show in Explorer", () => act.reveal(focus.path), { focus: "map:reveal" })),
        breadcrumb(trail, name, act),
        info,
        v.style === "list"
          ? listView(focus, store, act, growth, name)
          : chartHost((width, height) => v.style === "treemap"
            ? M.treemap({ ...chart, width, height })
            : M.sunburst({
              ...chart, width, height, canBack: trail.length > 1,
              back: () => act.mapFocus(trail[trail.length - 2].path),
            })),
        v.style !== "list" && v.colouring === "growth"
          ? h("p", { class: "map-legend secondary" },
            "Red: grew in the last 24 hours. Green: shrank. Grey: steady, or not watched yet. Size is what "
              + "deleting it would free.")
          : null)),
    h("div", { class: "map-caption secondary" }, M.caption(map, onWindows).map((line) => h("p", {}, line))),
    store.map.error ? h("p", { class: "warn-text map-error" }, icon("warning", 16), store.map.error) : null,
  ];
}

/** A chart drawn as wide and tall as its place, again when that changes. */
function chartHost(draw) {
  const host = h("div", { class: "map-host" });
  let size = "";
  new ResizeObserver(([e]) => {
    const w = Math.floor(e.contentRect.width);
    const hgt = Math.floor(e.contentRect.height);
    if (w > 0 && hgt > 0 && `${w}x${hgt}` !== size) {
      size = `${w}x${hgt}`;
      host.replaceChildren(draw(w, hgt));
    }
  }).observe(host);
  return host;
}

function colourPicker(value, onpick) {
  return h("label", { class: "map-colour" },
    h("span", { class: "secondary" }, "Colour"),
    h("select", { dataset: { focus: "map:colour" }, onchange: (e) => onpick(e.target.value) },
      [["growth", "Growth, last 24 hours"], ["folder", "Folder"]]
        .map(([id, text]) => h("option", { value: id, selected: id === value }, text))));
}

/** From the disk down to the folder in focus; each a way back up. */
function breadcrumb(trail, name, act) {
  return h("nav", { class: "crumbs", "aria-label": "Folder" },
    trail.map((n, i) => [
      i > 0 ? icon("chevron", 12, "secondary") : null,
      // The folder shown keeps the keyboard when its crumb was the one pressed.
      i === trail.length - 1
        ? h("span", { class: "crumb", "aria-current": "location", tabindex: "-1", dataset: { focus: `crumb:${n.path}` } },
          name(n))
        : button(name(n), () => act.mapFocus(n.path), { kind: "link", focus: `crumb:${n.path}` }),
    ]));
}

/** A folder's path, its size, its share of the folder in focus, and how it grew. */
export function infoLine(node, focus, growth, name) {
  let line = F.bytes(node.totals.private);
  if (node !== focus) {
    const share = focus.totals.private > 0 ? node.totals.private / focus.totals.private : 0;
    line += ` · ${Math.round(share * 100)}% of ${name(focus)}`;
  }
  const g = growth(node);
  if (g != null && Math.abs(g) >= 1_000_000) {
    line += ` · ${g > 0 ? "grew" : "shrank"} ${F.bytes(Math.abs(g))} in 24 hours`;
  }
  const path = M.userPath(node.path);
  return [h("span", { class: "map-path", title: path }, path), h("span", { class: "secondary num" }, line)];
}

/** The folders in focus as an outline, each with its share as a bar. */
function listView(focus, store, act, growth, name) {
  const top = focus.totals.private;
  const rows = [];
  const add = (node, depth) => {
    const opened = store.mapView.opened.has(node.path);
    rows.push(listRow(node, depth, opened, top, growth, name, act));
    if (opened) for (const c of node.children ?? []) add(c, depth + 1);
  };
  for (const c of focus.children ?? []) add(c, 0);
  return h("div", { class: "map-list", role: "tree", "aria-label": name(focus) },
    rows.length ? rows : h("p", { class: "secondary" }, "No folder in it is big enough to show on its own."));
}

function listRow(node, depth, opened, top, growth, name, act) {
  const share = top > 0 ? node.totals.private / top : 0;
  const g = growth(node);
  const path = M.userPath(node.path);
  return h("div", {
    class: "map-row", role: "treeitem", "aria-level": String(depth + 1),
    "aria-expanded": M.opens(node) ? String(opened) : null, title: path, style: { "--depth": String(depth) },
  },
  M.opens(node)
    ? h("button", {
      class: `disclose${opened ? " open" : ""}`, type: "button",
      "aria-label": `${opened ? "Close" : "Open"} ${name(node)}`,
      dataset: { focus: `map:open:${node.path}` }, onclick: () => act.mapToggle(node.path),
    }, icon("chevron", 12))
    : h("span", { class: "disclose" }),
  icon("folder", 16, "secondary"),
  // A folder with folders in it opens in the chart from its name (from the
  // keyboard too: the charts can't be reached with it).
  M.opens(node)
    ? h("button", {
      class: "map-name plain", type: "button", title: `Show what's in ${name(node)}`,
      dataset: { focus: `map:zoom:${node.path}` }, onclick: () => act.mapFocus(node.path, true),
    }, name(node))
    : h("span", { class: "map-name" }, name(node)),
  g != null && Math.abs(g) >= 10_000_000
    ? h("span", { class: `num small tone-${g > 0 ? "red" : "green"}` }, `${g > 0 ? "+" : "−"}${F.bytes(Math.abs(g))}`)
    : null,
  h("span", { class: "map-bar", "aria-hidden": "true" }, h("span", { style: { width: `${Math.max(2, 80 * share)}px` } })),
  h("span", { class: "num map-size" }, F.bytes(node.totals.private)));
}

