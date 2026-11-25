// Pages built in node, on a stand-in document: what can be checked of how
// they're put together without a browser. `node --test app-windows/ui/tests/*.test.mjs`.

import assert from "node:assert/strict";
import { test } from "node:test";

// Just enough of a document for `h` to build a page: elements with their
// attributes and children, text, fragments.
class Node {}
class Text extends Node {
  constructor(data) {
    super();
    this.data = data;
  }
}
class Element extends Node {
  constructor(tag) {
    super();
    this.tagName = tag.toUpperCase();
    this.attrs = {};
    this.children = [];
    this.dataset = {};
    this.className = "";
    this.style = { setProperty() {} };
  }
  setAttribute(k, v) { this.attrs[k] = v; }
  getAttribute(k) { return this.attrs[k] ?? null; }
  addEventListener() {}
  append(...children) {
    for (const c of children) this.children.push(...(c instanceof Fragment ? c.children : [c]));
  }
}
class Fragment extends Element {
  constructor() { super("#fragment"); }
}
globalThis.Node = Node;
globalThis.document = {
  createElement: (tag) => new Element(tag),
  createElementNS: (_, tag) => new Element(tag),
  createTextNode: (data) => new Text(data),
  createDocumentFragment: () => new Fragment(),
};

const { prevention } = await import("../js/pages/prevention.js");

/** `el` and every element in it. */
const all = (el) => [el, ...el.children.flatMap((c) => (c instanceof Element ? all(c) : []))];
const LABELABLE = ["BUTTON", "INPUT", "METER", "OUTPUT", "PROGRESS", "SELECT", "TEXTAREA"];

test("a click on a switch's row flips the switch, never another control in the row", () => {
  const store = {
    tidies: [
      {
        id: "incremental", title: "Old Rust build caches", detail: "d", on: false, available: true,
        unavailable_why: null, last: { at: 1_792_600_000, freed: 2e9, done: 2, failed: 0, note: "n" },
      },
      { id: "builds", title: "Old build folders", detail: "d", on: true, available: false, unavailable_why: "why", last: null },
    ],
    tidyBusy: new Set(),
    tweaks: [{ id: "gradle-caches", title: "T", why: "w", cost: "c", status: "not applied", applied: null, size: 5e8 }],
    tweaksChanging: new Map(),
  };
  const labels = all(prevention(store, {})).filter((e) => e.tagName === "LABEL");
  assert.equal(labels.length, 3);
  let withButton = 0;
  for (const label of labels) {
    const controls = all(label).slice(1).filter((e) => LABELABLE.includes(e.tagName));
    const toggle = controls.find((e) => e.getAttribute("role") === "switch");
    assert.ok(toggle, "each row has its switch");
    if (controls.some((e) => e.tagName === "BUTTON")) withButton += 1;
    // Without `for`, a label's control is the first one in it: a click on the
    // row's text would press a button that comes before the switch.
    if (controls[0] !== toggle) {
      assert.ok(toggle.getAttribute("id"), "the switch has an id");
      assert.equal(label.getAttribute("for"), toggle.getAttribute("id"), "the label names its switch");
    }
  }
  assert.equal(withButton, 1, "the clean-up that can run has Tidy Now in its row");
});
