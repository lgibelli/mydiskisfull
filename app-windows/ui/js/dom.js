// Building the page: elements made with `h`, never from HTML text, so a
// folder's name is always text, whatever it holds. Icons are drawn here, in
// a 20-point box, with lines in the text's colour.

/**
 * `h("div", {class: "row", onclick: f}, child, …)`: an element with its
 * attributes, handlers (`on…`) and children (text, elements, arrays; null
 * and false are left out).
 */
export function h(tag, attrs = {}, ...children) {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs ?? {})) {
    if (v == null || v === false) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "class") el.className = v;
    // Hyphenated names, as in CSS: custom properties work too.
    else if (k === "style") for (const [p, x] of Object.entries(v)) el.style.setProperty(p, x);
    else if (k === "dataset") Object.assign(el.dataset, v);
    else el.setAttribute(k, v === true ? "" : String(v));
  }
  append(el, children);
  return el;
}

function append(el, children) {
  for (const c of children) {
    if (c == null || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
}

const SVG = "http://www.w3.org/2000/svg";

/** An SVG element with its attributes (`style` takes an object, for CSS variables). */
export function s(tag, attrs = {}, ...children) {
  const el = document.createElementNS(SVG, tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
    else if (k === "style") for (const [p, x] of Object.entries(v)) el.style.setProperty(p, x);
    else if (k === "class") el.setAttribute("class", v);
    else el.setAttribute(k, String(v));
  }
  for (const c of children.flat()) {
    if (c == null || c === false) continue;
    el.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return el;
}

/**
 * `mdif`'s texts mark commands and names with backticks: shown as code.
 * Never a link, and never HTML: names come from the disk.
 */
export function inline(text) {
  const parts = String(text ?? "").split("`");
  // An odd backtick out: all of it is text.
  if (parts.length % 2 === 0) return document.createTextNode(text);
  const out = document.createDocumentFragment();
  parts.forEach((p, i) => {
    if (!p) return;
    out.append(i % 2 ? h("code", {}, p) : document.createTextNode(p));
  });
  return out;
}

/** Paragraphs, from text with blank lines between them. */
export function paragraphs(text) {
  return String(text ?? "")
    .split(/\n\s*\n/)
    .filter((p) => p.trim())
    .map((p) => h("p", {}, inline(p.trim())));
}

// -- Icons ---------------------------------------------------------------------

/** Lines, in a 20-point box; `fill`: shapes filled instead. */
const ICONS = {
  overview: { d: "M3 14.5a7 7 0 0 1 14 0 M10 14.5l3.6-4.4 M5.5 14.5h.01 M14.5 14.5h.01" },
  storage: { d: "M4 16.5V10 M8 16.5V4 M12 16.5V7.5 M16 16.5V12" },
  history: { d: "M3 16.5h14 M3.5 13l4-4.5 3 3 5.5-6.5" },
  settings: { d: "M3.5 6h13 M3.5 14h13 M7 4v4 M13 12v4" },
  trash: { d: "M4 6h12 M8 6V4h4v2 M5.8 6l.9 10.5h6.6l.9-10.5 M8.5 9v5 M11.5 9v5" },
  branch: { d: "M6 3.5v13 M6 12.5c0-4 8-2.5 8-7 M6 3.5h.01 M14 5.5h.01 M6 16.5h.01" },
  restart: { d: "M15.6 10.4a5.6 5.6 0 1 1-1.8-4.6 M14.2 2.8v3.5h-3.5" },
  flame: { d: "M10 2.8c.6 3 4.5 4.6 4.5 8.6a4.5 4.5 0 0 1-9 0c0-2 .9-3.4 2.1-4.4 0 1.9.9 3.1 2.1 3.3-.8-2.3-.3-5.3.3-7.5z" },
  terminal: { d: "M3 4.5h14v11H3z M6.5 8l2.5 2-2.5 2 M10.5 12.5h3" },
  checklist: { d: "M9 6h7.5 M9 10h7.5 M9 14h7.5 M3.5 6l1.2 1.2 2-2.2 M3.5 10l1.2 1.2 2-2.2 M3.5 14l1.2 1.2 2-2.2" },
  leaf: { d: "M4 16.5c0-7.5 4.5-11.5 12.5-12.5 0 8.5-4.5 12.5-11.5 12.5 M4 16.5l6.5-6.5" },
  wand: { d: "M4 16.5l9-9 M11.5 6l2.5 2.5 M15 3v2.5 M13.8 4.2h2.5 M16.5 9v2 M15.5 10h2" },
  // Clean Up's, as the Mac's `sparkles`: a big four-pointed star and a small one.
  sparkles: { d: "M8 3.5c.6 3.4 1.6 4.4 5 5-3.4.6-4.4 1.6-5 5-.6-3.4-1.6-4.4-5-5 3.4-.6 4.4-1.6 5-5z M14.5 11.5c.3 1.6.9 2.2 2.5 2.5-1.6.3-2.2.9-2.5 2.5-.3-1.6-.9-2.2-2.5-2.5 1.6-.3 2.2-.9 2.5-2.5z" },
  folder: { d: "M2.8 5.5h5l1.6 2h7.8v8.5H2.8z" },
  info: { d: "M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14z M10 9v5 M10 6.4v.01" },
  copy: { d: "M7 7h9.5v9.5H7z M4 13V3.5h9.5" },
  more: { d: "M5 10h.01 M10 10h.01 M15 10h.01", width: 2.6 },
  warning: { d: "M10 3.2l7.6 13.3H2.4z M10 8.2v4 M10 14.4v.01" },
  check: { d: "M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14z M6.8 10.2l2.2 2.2 4.4-4.7" },
  // Prevention's, as the Mac's `checkmark.shield`.
  shield: { d: "M10 2.6l6 2.3v4.7c0 3.9-2.5 6.5-6 7.9-3.5-1.4-6-4-6-7.9V4.9z M7.3 9.9l1.9 1.9 3.6-3.9" },
  hourglass: { d: "M6 3h8 M6 17h8 M7 3c0 4.2 6 3.8 6 7s-6 2.8-6 7 M13 3c0 4.2-6 3.8-6 7s6 2.8 6 7" },
  exclaim: { d: "M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14z M10 6.3v4.8 M10 13.6v.01" },
  pause: { d: "M10 17a7 7 0 1 0 0-14 7 7 0 0 0 0 14z M8.2 7.5v5 M11.8 7.5v5" },
  disk: { d: "M2.5 6.5h15v7h-15z M5.5 10h.01 M12 10h3" },
  windows: { d: "M3 3h6.5v6.5H3z M10.5 3H17v6.5h-6.5z M3 10.5h6.5V17H3z M10.5 10.5H17V17h-6.5z", fill: true },
  grid: { d: "M3.5 3.5h5v5h-5z M11.5 3.5h5v5h-5z M3.5 11.5h5v5h-5z M11.5 11.5h5v5h-5z" },
  hammer: { d: "M11.8 4.2l4 4 M9.3 6.7l4 4 M8 3.5l2.8-1 5.7 5.7-1 2.8 M10.5 9.5l-6.8 6.8-1.2-1.2 6.8-6.8" },
  wrench: { d: "M13.8 2.8a3.8 3.8 0 0 0-3.6 5L3.4 14.6l2 2 6.8-6.8a3.8 3.8 0 0 0 5-3.6l-2.3 2.3-2.3-.6-.6-2.3z" },
  cloud: { d: "M6 15.5h8.6a3.2 3.2 0 0 0 .3-6.4 4.8 4.8 0 0 0-9.2-1A3.8 3.8 0 0 0 6 15.5z" },
  pc: { d: "M2.5 3.5h15v9.5h-15z M6.5 17h7 M10 13v4" },
  photo: { d: "M2.5 4.5h15v11h-15z M2.5 13l4.5-4 3.5 3 2.2-2 4.8 4.5 M13.5 7.8h.01" },
  envelope: { d: "M2.5 4.5h15v11h-15z M2.5 5l7.5 6 7.5-6" },
  cycle: { d: "M15.5 7.5a6 6 0 0 0-10.4-1.2 M4.5 12.5a6 6 0 0 0 10.4 1.2 M15.8 3.3v4.4h-4.4 M4.2 16.7v-4.4h4.4" },
  tray: { d: "M2.5 11.5l2.2-7h10.6l2.2 7v5h-15z M2.5 11.5h4.5l1 2h4l1-2h4.5" },
  doc: { d: "M5 2.5h6.5l4 4v11H5z M11.5 2.5v4h4" },
  eye: { d: "M2 10s3-6 8-6 8 6 8 6-3 6-8 6-8-6-8-6z M10 12.4a2.4 2.4 0 1 0 0-4.8 2.4 2.4 0 0 0 0 4.8z" },
  close: { d: "M5 5l10 10 M15 5L5 15" },
  chevron: { d: "M7.5 4.5l5.5 5.5-5.5 5.5" },
  back: { d: "M12.5 4.5L7 10l5.5 5.5" },
};

/** An icon by name (ICONS), `size` points square. */
export function icon(name, size = 16, cls = "") {
  const i = ICONS[name] ?? ICONS.more;
  return s(
    "svg",
    { class: `icon ${cls}`.trim(), width: size, height: size, viewBox: "0 0 20 20", "aria-hidden": "true" },
    s("path", {
      d: i.d,
      fill: i.fill ? "currentColor" : "none",
      stroke: i.fill ? "none" : "currentColor",
      "stroke-width": i.width ?? 1.6,
      "stroke-linecap": "round",
      "stroke-linejoin": "round",
    }),
  );
}

/** A rounded tile in `tint` with the icon in white, as the Mac app's IconTile. */
export function tile(name, tint, size = 28) {
  return h("span", { class: "tile", style: { width: `${size}px`, height: `${size}px`, "--tint": tint } },
    icon(name, Math.round(size * 0.6)));
}
