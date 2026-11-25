// Old Files: what nothing has opened or changed in a long time, biggest
// first, as the Mac app's page: big files of yours (`old_files`: your own
// folders, not AppData, cloud folders, virtual machines, mail or build
// output) and folders nothing changed for that long. How big and how old is
// chosen above it. A file can go to the Recycle Bin (`mdif clean` checks it
// again first); anything can be kept.

import { h, tile } from "../dom.js";
import * as F from "../format.js";
import * as T from "../text.js";
import { cleanButton } from "./overview.js";
import { busy, button, choice, menu, note, page, row, section } from "../widgets.js";

export function oldFiles(store, act) {
  const r = store.space;
  const measure = button(store.spaceBusy ? "Measuring…" : "Measure Again", () => act.loadSpace(true), {
    disabled: store.spaceBusy, title: "Measure the whole disk again: about a minute",
  });
  if (!r) {
    return page("Old Files", { tools: [measure] },
      store.spaceError && !store.spaceBusy
        ? section(null, {},
          row({
            glyph: ["warning", "var(--orange)"], title: "Couldn't measure the disk", text: store.spaceError,
            end: button("Try Again", () => act.loadSpace(true)),
          }))
        : section(null, {}, busy("Measuring the whole disk: about a minute the first time.")));
  }
  const { minBytes, minDays } = store.oldFilter;
  const rows = T.oldRows(r, minBytes, minDays, store.kept);
  const picked = rows.filter((x) => store.oldPicked.has(x.path));
  const age = T.OLD_AGES.find(([d]) => d === minDays)?.[1] ?? F.age(minDays * 86_400);
  return page("Old Files", { sub: `Measured ${F.ago(r.measured)}`, tools: [measure] },
    section(null, {},
      choice({
        title: "Larger than", value: minBytes, options: T.OLD_SIZES.map((b) => [b, F.bytes(b)]),
        onchange: (v) => act.setOldFilter("minBytes", Number(v)),
      }),
      choice({
        title: "Not used for", value: minDays, options: T.OLD_AGES,
        onchange: (v) => act.setOldFilter("minDays", Number(v)),
      })),
    r.last_opened_unknown
      ? section(null, {}, row({
        glyph: ["info", "var(--accent)"],
        title: "Windows doesn't record here when a file was last opened",
        text: `So these are the files nothing changed for ${age}: a film you still watch or an installer you run again looks the same. Check before you move one.`,
      }))
      : null,
    rows.length
      ? section(`${rows.length} ${rows.length === 1 ? "item" : "items"}, ${F.bytes(rows.reduce((n, x) => n + x.bytes, 0))}`,
        { footer: "Moving a file to the Recycle Bin frees its space once you empty the bin. A folder is only listed: open it in Explorer to look." },
        rows.map((x) => oldRow(x, store, act)))
      : section(null, {}, note("Nothing that old and that big. Try a smaller size or a shorter time.")),
    picked.length ? pickedBar(picked, store, act) : null);
}

/** A file or a folder: ticked for the bar below, its place, how long since it was used, its size and button. */
function oldRow(x, store, act) {
  const ticked = store.oldPicked.has(x.path);
  const when = `${x.how} ${F.age(store.space.measured - x.last)} ago`;
  return h("div", { class: "row" },
    h("input", {
      type: "checkbox", class: "pick", checked: ticked, "aria-label": `Select ${x.name}`,
      dataset: { focus: `pick:${x.path}` }, onchange: () => act.toggleOldPick(x.path),
    }),
    tile(x.isFolder ? "folder" : "doc", x.isFolder ? "var(--k-files)" : "var(--k-media)", 24),
    h("div", { class: "row-text" },
      h("div", { class: "row-title", title: x.path }, x.name),
      h("div", { class: "row-sub" }, h("span", { class: "path" }, x.folder), " · ",
        h("span", { title: F.day(x.last) }, when))),
    h("div", { class: "row-end" },
      h("span", { class: "num" }, F.bytes(x.bytes)),
      x.action ? cleanButton(x.action, store, act) : null,
      menu([["Show in Explorer", () => act.reveal(x.path)], ["Keep It", () => act.keepAll([x.path])]],
        "More", `menu:old:${x.path}`)));
}

/** What's ticked: how many and how big, kept, or moved to the Recycle Bin together. */
function pickedBar(picked, store, act) {
  const all = T.recycleAll(picked);
  const running = picked.some((x) => store.cleaning.has(x.path));
  return h("div", { class: "picked-bar card" },
    h("span", { class: "secondary num" },
      `${picked.length} selected, ${F.bytes(picked.reduce((n, x) => n + x.bytes, 0))}`),
    h("span", { class: "spacer" }),
    button("Clear", act.clearOldPicks, { kind: "link" }),
    button("Keep", () => act.keepAll(picked.map((x) => x.path)), {
      title: "Never suggest deleting them again. Settings lists what you keep.",
    }),
    button(running ? "Moving…" : "Move to Recycle Bin", () => all && act.clean(all), {
      kind: "accent", disabled: !all || running,
      title: all ? null : "Only files can go from here: a folder is opened in Explorer.",
    }));
}
