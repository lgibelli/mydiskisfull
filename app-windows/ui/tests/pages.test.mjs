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

// What core.js reads when it loads: no app, so it's a page in a browser.
globalThis.window = {};
globalThis.location = { search: "" };

const { prevention } = await import("../js/pages/prevention.js");
const { updates } = await import("../js/pages/settings.js");

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

/** The text in `el`, every piece joined. */
const text = (el) => all(el).flatMap((e) => e.children.filter((c) => c instanceof Text).map((c) => c.data)).join(" ");
const buttons = (el) => all(el).filter((e) => e.tagName === "BUTTON").map(text);

test("Updates offers Check Now, then Install and Restart for what's found, and no button while busy", () => {
  assert.equal(updates({ update: null }, {}), null, "not read yet");
  const none = updates({ update: { on: true, checked: Date.now() / 1000 - 7200, found: null, busy: null } }, {});
  assert.deepEqual(buttons(none), ["Check Now"]);
  assert.match(text(none), /Last checked 2 hours ago/);
  const checking = updates({ update: { on: true, checked: null, found: null, busy: "checking" } }, {});
  assert.deepEqual(buttons(checking), [], "one check at a time");
  assert.match(text(checking), /Checking…/);
  const found = { version: "0.1.7" };
  const ready = updates({ update: { on: false, checked: null, found, busy: null } }, {});
  assert.deepEqual(buttons(ready), ["Check Now", "Install and Restart"], "found, though checks are off: Check Now found it");
  assert.match(text(ready), /MyDiskIsFull 0\.1\.7 is ready/);
  assert.match(text(ready), /closes while it installs/);
  const installing = updates({ update: { on: true, checked: null, found, busy: "installing" } }, {});
  assert.deepEqual(buttons(installing), [], "installed once");
  assert.match(text(installing), /Installing…/);
  // A check by itself found it while Check Now ran: nothing to press until it's done.
  const both = updates({ update: { on: true, checked: null, found, busy: "checking" } }, {});
  assert.deepEqual(buttons(both), []);
  assert.match(text(both), /Checking…/);
});
