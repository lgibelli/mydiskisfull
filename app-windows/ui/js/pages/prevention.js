// Prevention, as the Mac app's: clean-ups the watcher does by itself once a
// day, each once it's turned on (`mdif tidy`), and settings that stop
// something from filling the disk again (`mdif tweaks`). Turning a setting on
// shows what it changes first (a dry run); turning it off puts back what it
// replaced.

import { h } from "../dom.js";
import * as F from "../format.js";
import * as T from "../text.js";
import { busy, button, note, page, row, section } from "../widgets.js";

export function prevention(store, act) {
  const list = store.tweaks;
  const shown = Array.isArray(list) ? list.filter((t) => t.status !== "absent") : [];
  let rows;
  if (list == null) rows = busy("Checking each setting…");
  else if (list.error) rows = row({ glyph: ["warning", "var(--orange)"], text: list.error });
  else if (!shown.length) rows = note("None of them applies to this PC.");
  else rows = shown.map((t) => tweakRow(t, store, act));
  return page("Prevention", {},
    tidySection(store, act),
    section("Settings That Stop the Filling", {
      footer: "Each changes a setting of Windows or an app so that it stops filling the disk. "
        + "MyDiskIsFull keeps what it replaced: turn it off to put it back.",
    }, rows));
}

function tidySection(store, act) {
  const list = store.tidies;
  let rows;
  if (list == null) rows = busy("Looking at each clean-up…");
  else if (list.error) rows = row({ glyph: ["warning", "var(--orange)"], text: list.error });
  else rows = list.map((t) => tidyRow(t, store, act));
  return section("Automatic Clean-ups", {
    footer: "MyDiskIsFull does these once a day while it watches. Never anything you keep.",
  }, rows);
}

/** A clean-up as a switch, with what it takes, why it can't run here, and what it did last. */
function tidyRow(t, store, act) {
  const working = store.tidyBusy.has(t.id);
  // One that's on can still be turned off where it can't run.
  const disabled = working || (!t.available && !t.on);
  const switchId = `tidy-switch-${t.id}`;
  const input = h("input", {
    id: switchId, type: "checkbox", role: "switch", checked: t.on, disabled,
    dataset: { focus: `tidy:${t.id}` },
    "aria-label": t.title,
    onchange: (e) => act.setTidy(t.id, e.target.checked),
  });
  const last = t.last
    ? h("span", { class: t.last.failed > 0 ? "tone-orange" : null }, T.tidyLast(t.last))
    : null;
  const now = t.available
    ? button("Tidy Now", () => act.tidyNow(t.id), { kind: "link", disabled: working, focus: `tidy-now:${t.id}` })
    : null;
  // `for` the switch: without it a label's control is the first one in it,
  // Tidy Now, and a click on the row's text would run the clean-up.
  return h("label", { for: switchId, class: `row toggle tweak${disabled ? " disabled" : ""}` },
    h("div", { class: "row-text" },
      h("div", { class: "row-title" }, t.title),
      h("div", { class: "row-sub" }, t.detail),
      !t.available && t.unavailable_why ? h("div", { class: "row-sub tone-orange" }, t.unavailable_why) : null,
      last || now || working
        ? h("div", { class: "row-sub tidy-last" }, last, now,
          working ? h("span", { class: "spinner small", "aria-hidden": "true" }) : null)
        : null),
    h("div", { class: "row-end" },
      h("span", { class: "switch-word" }, t.on ? "On" : "Off"),
      h("span", { class: "switch" }, input, h("span", { class: "knob" }))));
}

/** A tweak as a switch: on while applied, with what it's for and what it costs. */
function tweakRow(t, store, act) {
  const applied = t.status === "applied";
  const byUs = t.applied != null;
  const changing = store.tweaksChanging.get(t.id);
  const on = changing ?? applied;
  // Applied by hand: there's no record of what it replaced. Being changed: until it's read again.
  const disabled = (applied && !byUs) || changing != null;
  const badge = tweakNote(t);
  const input = h("input", {
    type: "checkbox", role: "switch", checked: on, disabled,
    dataset: { focus: `tweak:${t.id}` },
    "aria-label": t.title,
    onchange: (e) => (e.target.checked ? act.applyTweak(t.id) : act.revertTweak(t.id, t.title)),
  });
  return h("label", {
    class: `row toggle tweak${disabled ? " disabled" : ""}`,
    title: applied && !byUs
      ? "Already done: its settings are as this tweak would set them (in Settings, or by hand). "
        + "MyDiskIsFull only undoes what it set itself."
      : null,
  },
  h("div", { class: "row-text" },
    h("div", { class: "row-title" }, t.title,
      badge ? h("span", { class: `tweak-note tone-${badge[1]}` }, badge[0]) : null),
    h("div", { class: "row-sub" }, t.why),
    h("div", { class: "row-sub" }, `You give up: ${t.cost}`),
    !applied && t.size >= 100e6 ? h("div", { class: "row-sub" }, `${F.bytes(t.size)} there now.`) : null),
  h("div", { class: "row-end" },
    h("span", { class: "switch-word" }, on ? "On" : "Off"),
    h("span", { class: "switch" }, input, h("span", { class: "knob" }))));
}

/** What to know beside its title, and its colour. */
export function tweakNote(t) {
  if (t.status === "partly applied") return ["Partly applied", "orange"];
  if (t.status === "applied" && t.applied == null) return ["Already done", "secondary"];
  // Applied with MyDiskIsFull, and undone since (an update, or the app's own settings).
  if (t.status !== "applied" && t.applied != null) return ["Undone since", "purple"];
  return null;
}
