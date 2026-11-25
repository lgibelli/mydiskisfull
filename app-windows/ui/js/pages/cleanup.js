// Clean Up, as the Mac app's sheet (CleanUpView.swift): everything the app can
// clean itself, in one list, each with a checkbox. What comes back by itself
// and loses nothing used lately is ticked to start with; what needs a look
// (the Recycle Bin) isn't, and says what can't be undone beside it. The sheet
// is the question: what's ticked goes, one after the other, each through
// `mdif clean` (which checks it again first), and what each did shows on its
// row. No dialog before, none after.

import { h, icon, inline, tile } from "../dom.js";
import * as F from "../format.js";
import * as T from "../text.js";
import { button } from "../widgets.js";

/** The sheet as `c` (the store's `cleanup`) has it now. */
export function cleanUpSheet(store, act) {
  const c = store.cleanup;
  const safe = c.rows.filter((r) => r.action.safe);
  const look = c.rows.filter((r) => !r.action.safe);
  return h("div", { class: "sheet-body" },
    h("div", { class: "sheet-head" },
      h("h2", { id: "cleanup-title", class: "dialog-title" }, "Clean Up"),
      h("p", { class: "secondary" },
        "What MyDiskIsFull can clean up itself. Ticked: what comes back by itself, and nothing you "
          + "used lately. Each is checked again right before.")),
    c.rows.length
      ? h("div", { class: "sheet-list" },
        safe.length ? group("Comes back by itself", safe, c, act) : null,
        look.length ? group("Look first", look, c, act) : null)
      : h("div", { class: "sheet-empty secondary" },
        store.space
          ? [h("span", { class: "glyph ok" }, icon("check", 28)), h("p", {}, "Nothing to clean up.")]
          : [h("span", { class: "spinner", "aria-hidden": "true" }), h("p", {}, "Measuring the whole disk: about a minute the first time.")]),
    foot(c, act));
}

function group(title, rows, c, act) {
  return h("section", { class: "sheet-group", "aria-label": title },
    h("h3", {}, title),
    h("div", { class: "card list" }, rows.map((r) => cleanUpRow(r, c, act))));
}

function cleanUpRow(r, c, act) {
  const state = c.states.get(r.id);
  // Paired by place: a row's id holds its paths, a line apart.
  const box = `cleanup-box-${c.rows.indexOf(r)}`;
  const lines = [];
  if (state?.state === "done") lines.push(h("div", { class: "row-sub ok-text" }, state.text));
  else if (state?.state === "failed") lines.push(h("div", { class: "row-sub warn-text" }, state.text));
  else {
    if (r.detail) lines.push(h("div", { class: "row-sub clamp" }, inline(r.detail)));
    // What can't be undone, said before it's ticked.
    if (!r.action.safe && r.action.confirm) lines.push(h("div", { class: "row-sub small" }, inline(r.action.confirm)));
  }
  return h("div", { class: "row cleanup-row", title: r.action.confirm ?? null },
    h("span", { class: "cleanup-mark" }, mark(r, state, c, act, box)),
    tile(r.symbol, r.tint, 26),
    h("div", { class: "row-text" },
      h("label", { class: "cleanup-title", for: box },
        h("span", { class: "row-title" }, r.title),
        r.bytes != null ? h("span", { class: "num secondary" }, F.bytes(r.bytes)) : null),
      lines));
}

function mark(r, state, c, act, box) {
  switch (state?.state) {
    case "running": return h("span", { class: "spinner", role: "img", "aria-label": "Cleaning" });
    case "done": return h("span", { class: "glyph ok", role: "img", "aria-label": "Done" }, icon("check", 18));
    case "failed": return h("span", { class: "glyph warn", role: "img", "aria-label": "Not done" }, icon("warning", 18));
    default:
      return h("input", {
        type: "checkbox", id: box, checked: c.chosen.has(r.id), disabled: c.running || T.cleanUpFinished(c),
        dataset: { focus: `cleanup:${r.id}` }, onchange: (e) => act.tickCleanUp(r.id, e.target.checked),
      });
  }
}

function foot(c, act) {
  const picked = c.rows.filter((r) => c.chosen.has(r.id));
  if (c.running) {
    const at = picked.findIndex((r) => c.states.get(r.id)?.state === "running");
    return h("div", { class: "sheet-foot" },
      h("span", { class: "spinner", "aria-hidden": "true" }),
      h("span", { class: "secondary", role: "status" },
        c.stopping ? "Stopping after this one…" : `Cleaning ${at < 0 ? picked.length : at + 1} of ${picked.length}…`),
      h("span", { class: "grow" }),
      button("Stop", act.stopCleanUp, { disabled: c.stopping, focus: "cleanup:stop" }));
  }
  if (T.cleanUpFinished(c)) {
    return h("div", { class: "sheet-foot" },
      h("span", { class: "glyph ok" }, icon("check", 18)),
      h("span", { class: "num", role: "status" }, `The disk has ${F.bytes(c.freed)} more free.`),
      h("span", { class: "grow" }),
      button("Look Again", act.lookAgainCleanUp, { focus: "cleanup:again" }),
      button("Done", act.closeCleanUp, { kind: "accent", focus: "cleanup:done" }));
  }
  const all = picked.length === c.rows.length;
  return h("div", { class: "sheet-foot" },
    c.rows.length
      ? button(all ? "Select None" : "Select All", () => act.tickAllCleanUp(!all), { focus: "cleanup:all" })
      : null,
    picked.length
      ? h("span", { class: "secondary num" }, `Frees about ${F.bytes(T.cleanUpFrees(picked))}`)
      : null,
    h("span", { class: "grow" }),
    button("Cancel", act.closeCleanUp, { focus: "cleanup:cancel" }),
    button(`Clean Up ${picked.length}`, act.runCleanUp,
      { kind: "accent", disabled: !picked.length, focus: "cleanup:run" }));
}
