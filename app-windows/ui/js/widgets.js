// The pieces the pages are made of, laid out like Windows' Settings: cards
// of rows, each with its title, what it says, and its control at the end.

import { h, icon, inline, paragraphs, tile } from "./dom.js";
import * as F from "./format.js";

/** A page: its title (and what's under it), its tools, then its sections. */
export function page(title, { sub, tools } = {}, ...body) {
  return h("div", { class: "page" },
    h("header", { class: "page-head" },
      h("div", { class: "page-titles" }, h("h1", {}, title), sub ? h("p", { class: "sub" }, sub) : null),
      tools ? h("div", { class: "tools" }, tools) : null),
    body);
}

/** A titled group of rows; `aside` goes at the end of its title (a link). */
export function section(title, { aside, footer, cls } = {}, ...rows) {
  return h("section", { class: `section ${cls ?? ""}`.trim() },
    title || aside
      ? h("div", { class: "section-head" }, title ? h("h2", {}, title) : h("span"), aside ?? null)
      : null,
    h("div", { class: "card list" }, rows),
    footer ? h("p", { class: "footer" }, inline(footer)) : null);
}

/**
 * A row: an icon tile or an icon, its title and text (inline code), and what
 * goes at its end.
 */
export function row({ tile: t, glyph, title, text, end, cls, titleHint }) {
  return h("div", { class: `row ${cls ?? ""}`.trim() },
    t ? tile(t[0], t[1]) : glyph ? h("span", { class: "glyph", style: { color: glyph[1] } }, icon(glyph[0], 20)) : null,
    h("div", { class: "row-text" },
      title ? h("div", { class: "row-title", title: titleHint ?? null }, inline(title)) : null,
      text ? h("div", { class: "row-sub" }, inline(text)) : null),
    end ? h("div", { class: "row-end" }, end) : null);
}

/** A row that only says something. */
export const note = (text) => h("div", { class: "row note" }, h("div", { class: "row-sub" }, inline(text)));

/** A label and its value, as Settings shows a figure. */
export function labeled(label, value, hint) {
  return h("div", { class: "row" },
    h("div", { class: "row-text" }, h("div", { class: "row-title" }, label)),
    h("div", { class: "row-end value", title: hint ?? null }, value));
}

/**
 * A button; `kind` "accent" (the one to press), "danger" (what can't be
 * undone) or "link".
 */
export function button(label, onclick, { kind, title, disabled, glyph, focus } = {}) {
  return h("button", {
    class: `btn ${kind ?? ""}`.trim(), type: "button", title: title ?? null, disabled: disabled ?? false,
    dataset: { focus: focus ?? `button:${label}` }, onclick,
  }, glyph ? icon(glyph, 16) : null, label ? h("span", {}, label) : null);
}

/** A button with an icon only, its name in its tooltip. */
export function iconButton(glyph, label, onclick, { expanded, focus } = {}) {
  return h("button", {
    class: "btn icon-only", type: "button", title: label, "aria-label": label,
    "aria-expanded": expanded == null ? null : String(expanded),
    dataset: focus ? { focus } : null, onclick,
  }, icon(glyph, 16));
}

/** A row of buttons, one of them chosen, as Windows' segmented control. */
export function segmented(label, options, value, onpick) {
  return h("div", { class: "segmented", role: "radiogroup", "aria-label": label },
    options.map(([id, text]) => h("button", {
      type: "button", role: "radio", "aria-checked": String(id === value),
      class: id === value ? "on" : "", dataset: { focus: `segment:${label}:${id}` }, onclick: () => onpick(id),
    }, text)));
}

/** A switch, as Windows draws one, with its title and what it does. */
export function toggle({ title, text, on, onchange, disabled }) {
  const input = h("input", {
    type: "checkbox", role: "switch", checked: on, disabled: disabled ?? false,
    dataset: { focus: `toggle:${title}` }, onchange: (e) => onchange(e.target.checked),
  });
  return h("label", { class: `row toggle${disabled ? " disabled" : ""}` },
    h("div", { class: "row-text" },
      h("div", { class: "row-title" }, title),
      text ? h("div", { class: "row-sub" }, inline(text)) : null),
    h("div", { class: "row-end" },
      h("span", { class: "switch-word" }, on ? "On" : "Off"),
      h("span", { class: "switch" }, input, h("span", { class: "knob" }))));
}

/** A choice among values: `options` are [value, label]. */
export function choice({ title, text, value, options, onchange, disabled }) {
  const select = h("select", {
    disabled: disabled ?? false, dataset: { focus: `choice:${title}` }, onchange: (e) => onchange(e.target.value),
  },
    options.map(([v, label]) => h("option", { value: String(v), selected: String(v) === String(value) }, label)));
  return h("label", { class: `row${disabled ? " disabled" : ""}` },
    h("div", { class: "row-text" },
      h("div", { class: "row-title" }, title),
      text ? h("div", { class: "row-sub" }, inline(text)) : null),
    h("div", { class: "row-end" }, select));
}

/** A small coloured word: "Build output", "Kept". */
export const pill = (text, tone) => h("span", { class: `pill ${tone ?? ""}`.trim() }, text);

/** "Filled 3.4 GB" in orange, "Freed 2.9 GB" in green, "Quiet". */
export function change(b) {
  const tone = Math.abs(b) < 50_000_000 ? "muted" : b > 0 ? "orange" : "green";
  return h("span", { class: `num tone-${tone}` }, F.change(b));
}

/** A spinner, and what it waits for. */
export function busy(text) {
  return h("div", { class: "row note" }, h("span", { class: "spinner", "aria-hidden": "true" }),
    h("div", { class: "row-sub" }, text));
}

/**
 * A menu behind ⋯: items are [label, action] (null for a line). Closes on a
 * click elsewhere and on Esc.
 */
export function menu(items, label = "More", focus = null) {
  const list = h("div", { class: "menu", role: "menu", hidden: true },
    items.filter(Boolean).map(([text, act]) => h("button", {
      class: "menu-item", role: "menuitem", type: "button",
      onclick: () => {
        close();
        act();
      },
    }, text)));
  const opener = iconButton("more", label, () => (list.hidden ? open() : close()), { focus });
  const wrap = h("div", { class: "menu-wrap" }, opener, list);
  const away = (e) => {
    if (!wrap.contains(e.target)) close();
  };
  const esc = (e) => {
    if (e.key === "Escape") {
      close();
      opener.focus();
    }
  };
  function open() {
    list.hidden = false;
    opener.setAttribute("aria-expanded", "true");
    document.addEventListener("pointerdown", away, true);
    document.addEventListener("keydown", esc, true);
    list.querySelector("button")?.focus();
  }
  function close() {
    list.hidden = true;
    opener.setAttribute("aria-expanded", "false");
    document.removeEventListener("pointerdown", away, true);
    document.removeEventListener("keydown", esc, true);
  }
  return wrap;
}

/** A command to copy, as code, with its button. */
export function commandBox(command, copy) {
  return h("div", { class: "command" },
    h("code", {}, command),
    button("Copy", () => copy(command), { glyph: "copy" }));
}

/** The whole story behind a row's (i), shown under it. */
export function more({ text, command, sizes, tint, copy }) {
  return h("div", { class: "more" },
    paragraphs(text),
    sizes && sizes.length >= 2 ? sizeBars(sizes, tint) : null,
    command ? commandBox(command, copy) : null);
}

/** A folder's size at the end of each of the last days. */
export function sizeBars(points, tint) {
  const top = Math.max(...points.map((p) => p.bytes), 1);
  return h("div", { class: "size-bars", role: "img",
    "aria-label": `Its size over the last ${points.length} days, from ${F.bytes(points[0].bytes)} to ${F.bytes(points[points.length - 1].bytes)}` },
  points.map((p) => h("div", { class: "size-bar", title: `${F.day(p.ts)}: ${F.bytes(p.bytes)}` },
    h("span", { style: { height: `${Math.max(2, (p.bytes / top) * 100)}%`, background: tint } }))));
}
