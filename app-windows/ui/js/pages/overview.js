// The overview, as the Mac app shows it: the disk and its bar, how it's
// doing in a sentence, what needs attention, one list of what to do (biggest
// first, each with its button), and what changed lately.

import { h, icon, inline } from "../dom.js";
import * as F from "../format.js";
import * as T from "../text.js";
import { railBar } from "../rail.js";
import {
  busy, button, change, iconButton, labeled, menu, more, note, page, row, section,
} from "../widgets.js";

/** How many recommendations show before "Show All". */
const FIRST = 5;

export function overview(store, act) {
  const s = store.status;
  if (!s) {
    return page("Overview", {},
      store.statusError
        ? section(null, {}, row({ glyph: ["warning", "var(--orange)"], title: "Can't read the watcher's history", text: store.statusError }))
        : section(null, {}, busy("Reading the watcher's history…")));
  }
  return page("Overview", {
    sub: s.history.first ? `Watched since ${F.day(s.history.first)}` : null,
    tools: [
      button("Clean Up…", act.openCleanUp,
        { glyph: "sparkles", title: "Pick what to clean up, and do it all at once" }),
      button("Measure Again", () => {
        act.loadSpace(true);
        act.measure();
      }, { disabled: store.spaceBusy, title: "Measure the whole disk again: about a minute" }),
    ],
  },
  disk(store, act),
  attention(store, act),
  recommendations(store, act),
  recent(store, act));
}

// -- The disk ------------------------------------------------------------------

function disk(store, act) {
  const s = store.status;
  const r = store.space;
  const hl = T.health(s);
  const tint = { learning: "var(--blue)", calm: "var(--green)", watch: "var(--orange)", urgent: "var(--red)" }[hl];
  const glyph = { learning: "hourglass", calm: "check", watch: "exclaim", urgent: "warning" }[hl];
  const used = 1 - s.avail / Math.max(s.total, 1);
  return h("section", { class: "section" },
    h("div", { class: "card disk", style: { "--tint": tint } },
      h("div", { class: "disk-head" },
        h("span", { class: "disk-icon" }, icon("disk", 30)),
        h("div", {},
          h("div", { class: "disk-name" }, T.diskName(s.volume)),
          h("div", { class: "secondary num" }, `${F.bytes(s.avail)} available of ${F.bytes(s.total)}`))),
      r
        ? railBar(T.stops(r, 6), Math.max(r.volume.total, 1), (kind) => act.show("storage", kind))
        : h("div", { class: `capacity big tone-${hl === "calm" || hl === "learning" ? "accent" : hl === "watch" ? "orange" : "red"}` },
          h("span", { style: { width: `${Math.min(100, used * 100)}%` } })),
      h("div", { class: "status-row" },
        h("span", { class: "glyph", style: { color: tint } }, icon(glyph, 22)),
        h("div", {},
          h("h2", { class: "status-title" }, T.headline.title(s)),
          h("p", { class: "secondary" }, inline(T.headline.detail(s)))))));
}

// -- What needs attention --------------------------------------------------------

function attention(store, act) {
  const rows = [];
  const a = store.agent;
  if (a && !a.running) {
    const off = a.at_sign_in === "off";
    rows.push(row({
      tile: ["pause", "var(--orange)"],
      title: "The watcher isn't running",
      text: off
        ? "It's switched off in Task Manager's Startup apps, so nothing is measured or forecast. Starting it here switches it on again."
        : "Until it runs, nothing is measured or forecast.",
      end: [
        off ? button("Startup Apps…", act.openStartupApps, { kind: "link" }) : null,
        button("Start Watching", () => act.setWatching(true), { kind: "accent", disabled: store.watcherBusy }),
      ],
    }));
  }
  for (const e of store.status.incidents) rows.push(incident(e, store, act));
  if (!rows.length) return null;
  return section("Needs Attention", {}, rows);
}

/** A runaway filler: what it does, and what helps. */
function incident(e, store, act) {
  const fix = e.fix;
  // The suggestion made from it carries its button (Delete, Restart).
  const sg = store.suggestions?.find((x) => x.cause === e.sentence);
  const id = `incident:${T.episodeId(e)}`;
  const end = [];
  if (sg?.action) end.push(cleanButton(sg.action, store, act));
  else if (fix.kind === "run") end.push(button("Copy Command", () => act.copy(fix.detail.command), { title: fix.detail.command }));
  const story = fix.kind === "run" ? [fix.detail.note] : fix.kind === "steps" ? [fix.detail] : fix.kind === "leave" ? [`Leave it: ${fix.detail}`] : [];
  end.push(iconButton("info", "More about this", () => act.toggleOpened(id),
    { expanded: store.opened.has(id), focus: `more:${id}` }));
  if (e.incident.kind === "growth") {
    end.push(menu([["Show in Explorer", () => act.reveal(e.incident.subject)]], "More", `menu:${id}`));
  }
  return h("div", { class: "row-group" },
    row({
      tile: ["flame", "var(--red)"],
      title: T.episodeTitle(e),
      text: e.sentence,
      end,
    }),
    store.opened.has(id)
      ? more({ text: story.filter(Boolean).join("\n\n"), command: fix.kind === "run" ? fix.detail.command : null, copy: act.copy })
      : null);
}

// -- What to do -------------------------------------------------------------------

/** What a clean-up says while it runs. */
function busyText(kind) {
  return {
    trash: "Moving to the Recycle Bin…",
    "empty-trash": "Emptying the Recycle Bin…",
    delete: "Deleting…",
    empty: "Emptying…",
    restart: "Restarting…",
  }[kind] ?? "Removing…";
}

/** A clean-up's button (the Mac app's CleanButton); while it runs, what it's doing. */
export function cleanButton(a, store, act) {
  if (a.done) {
    // Pressing it again would do nothing more until the watcher has seen
    // whether it worked.
    return h("span", { class: "done secondary", title: "Watching whether its writes stop." },
      icon("check", 16), `Restarted at ${F.time(a.done)}`);
  }
  if (store.cleaning.has(a.paths[0])) {
    return h("span", { class: "working secondary" }, h("span", { class: "spinner", "aria-hidden": "true" }), busyText(a.kind));
  }
  return button(a.label, () => act.clean(a), { title: a.note ?? null, focus: `clean:${a.paths[0]}` });
}

/** A recommendation's button. */
function actButton(r, store, act) {
  const a = r.act;
  if (!a) return null;
  switch (a.kind) {
    case "clean": return cleanButton(a.action, store, act);
    case "copy": return button("Copy Command", () => act.copy(a.command), { title: a.command });
    case "kind": return button(a.label, () => act.show("storage", a.id));
    case "recycle-bin": return button(a.label, act.openRecycleBin);
    case "tweak": return button("Turn On…", () => act.applyTweak(a.id),
      { disabled: store.tweaksChanging.has(a.id), focus: `tweak:${a.id}` });
    case "tidy": return button("Turn On", () => act.setTidy(a.id, true),
      { disabled: store.tidyBusy.has(a.id), focus: `tidy-on:${a.id}` });
    default: return null;
  }
}

/** A recommendation as a row: its size, its button, (i) and ⋯. */
export function recommendationRow(r, store, act) {
  const end = [
    r.bytes != null ? h("span", { class: "num secondary size" }, F.bytes(r.bytes)) : null,
    actButton(r, store, act),
  ];
  if (r.more || r.command || (r.sizes?.length ?? 0) >= 2) {
    end.push(iconButton("info", "More about this", () => act.toggleOpened(r.id),
      { expanded: store.opened.has(r.id), focus: `more:${r.id}` }));
  }
  // Its own entries, by the name of what they do (`["Open Recycle Bin", "openRecycleBin"]`).
  const own = (r.menu ?? []).map(([label, name]) => [label, () => act[name]()]);
  if (r.path || own.length) {
    end.push(menu([
      r.path ? ["Show in Explorer", () => act.reveal(r.path)] : null,
      r.path && r.keepable ? ["Keep It", () => act.keep(r.path)] : null,
      ...own,
    ], "More", `menu:${r.id}`));
  }
  return h("div", { class: "row-group" },
    row({ tile: [r.symbol, r.tint], title: r.title, text: r.detail, end, titleHint: r.path }),
    store.opened.has(r.id)
      ? more({ text: r.more, command: r.command, sizes: r.sizes, tint: r.tint, copy: act.copy })
      : null);
}

/** What's in the list now: what was kept since is out of it. */
export function shownRecommendations(store) {
  return T.recommendations(store).filter((r) => !r.path || ![...store.kept].some((k) => T.within(r.path, k)));
}

function recommendations(store, act) {
  const list = shownRecommendations(store);
  const rows = [];
  if (!store.space) {
    rows.push(store.spaceError
      ? row({ glyph: ["warning", "var(--orange)"], title: "The disk couldn't be measured", text: store.spaceError })
      : busy("Measuring the whole disk: about a minute the first time."));
  } else if (!list.length) {
    rows.push(note(store.suggestBusy ? "Looking at what grows…" : "Nothing big to clean up."));
  }
  const shown = store.allShown ? list : list.slice(0, FIRST);
  rows.push(...shown.map((r) => recommendationRow(r, store, act)));
  if (list.length > FIRST) {
    rows.push(h("div", { class: "row more-row" },
      button(store.allShown ? "Show Fewer" : `Show All ${list.length}`, () => act.showAll(!store.allShown), { kind: "link" })));
  }
  return section("Recommendations", {
    aside: list.length
      ? button("Clean Up…", act.openCleanUp,
        { kind: "link", title: "Pick what to clean up, and do it all at once", focus: "cleanup:open" })
      : null,
  }, rows);
}

// -- What changed lately ------------------------------------------------------------

function recent(store, act) {
  const paces = store.status.forecast?.paces;
  const d = store.detail;
  const growth = d?.growth_3d ?? d?.growth_24h ?? [];
  const grew = growth.find((g) => g.bytes >= 100_000_000);
  const rows = [];
  if (paces?.day != null) rows.push(labeled("Last 24 hours", change(paces.day)));
  if (paces?.week != null) rows.push(labeled("Last 7 days", change(paces.week * 7)));
  if (grew) {
    rows.push(labeled(`Grew the most in ${d?.growth_3d ? "3 days" : "24 hours"}`,
      h("span", {}, F.lastName(grew.owner), " ", h("span", { class: "num" }, `+${F.bytes(grew.bytes)}`)),
      grew.owner));
  }
  const written = (d?.writes_24h ?? []).reduce((n, w) => n + w.bytes, 0);
  if (written > 0) rows.push(labeled("Written in 24 hours", h("span", { class: "num" }, F.bytes(written))));
  if (!rows.length) rows.push(note("Nothing yet: the watcher needs a few hours."));
  return section("Recent Changes", {
    aside: button("Show History", () => act.show("history"), { kind: "link" }),
  }, rows);
}
