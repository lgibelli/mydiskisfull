// Storage, as the Mac app shows it by category: the disk's bar with every
// kind, the kinds biggest first, and what comes back without deleting
// anything; a kind opens to its items, each with its size, its button and
// what's known about it.

import { h, icon, inline, paragraphs, tile } from "../dom.js";
import * as F from "../format.js";
import * as T from "../text.js";
import { railBar } from "../rail.js";
import { cleanButton } from "./overview.js";
import { folders } from "./folders.js";
import { continueButton, usingPlan } from "./settings.js";
import {
  busy, button, iconButton, labeled, menu, note, page, pill, row, section, segmented,
} from "../widgets.js";

/** How many items show before "Show More", unless more are advised. */
const FIRST = 12;

export function storage(store, act) {
  const r = store.space;
  const mode = segmented("View", [["categories", "Categories"], ["folders", "Folders"]], store.storageMode,
    act.setStorageMode);
  if (store.storageMode === "folders" && !store.kind) {
    const m = store.map;
    return page("Storage", {
      sub: m.measured != null ? `Measured ${F.ago(m.measured)}` : null,
      tools: [mode, button(m.running ? "Measuring…" : "Measure Again", () => act.measureMap(false), {
        disabled: m.running, title: "Map the whole disk again, folder by folder: about a minute",
      })],
    }, folders(store, act));
  }
  const measure = button(store.spaceBusy ? "Measuring…" : "Measure Again", () => act.loadSpace(true), {
    disabled: store.spaceBusy, title: "Measure the whole disk again: about a minute",
  });
  const tools = [mode, measure];
  if (!r) {
    return page("Storage", { tools },
      store.spaceError && !store.spaceBusy
        ? section(null, {},
          row({
            glyph: ["warning", "var(--orange)"], title: "Couldn't measure the disk", text: store.spaceError,
            end: button("Try Again", () => act.loadSpace(true)),
          }))
        : section(null, {},
          busy("Measuring the whole disk: about a minute the first time. It reads names, sizes and dates, never what's in a file.")));
  }
  const k = store.kind && r.kinds.find((x) => x.id === store.kind);
  if (k) return kindPage(k, r, store, act, measure);
  const sub = `${F.bytes(r.volume.used)} used · measured ${F.ago(r.measured)}`;
  return page("Storage", { sub, tools },
    h("section", { class: "section" },
      h("div", { class: "card padded" },
        railBar(T.stops(r), Math.max(r.volume.total, 1), (id) => act.show("storage", id)),
        changedSince(store, act))),
    section(null, {},
      r.kinds.filter((x) => x.bytes > 0).sort((a, b) => b.bytes - a.bytes).map((x) => kindRow(x, act)),
      row({ tile: ["more", "var(--free-tile)"], title: "Available", end: h("span", { class: "num secondary" }, F.bytes(r.volume.free)) })),
    comesBack(r, store, act));
}

/** The disk changed since it was measured: what's free now is live, the breakdown isn't. */
function changedSince(store, act) {
  const s = store.status;
  if (!s || store.spaceBusy) return null;
  const freed = s.avail - store.space.volume.free;
  if (Math.abs(freed) < 1_000_000_000) return null;
  return h("div", { class: "changed" },
    h("span", { class: `glyph tone-${freed > 0 ? "green" : "orange"}` }, icon(freed > 0 ? "check" : "exclaim", 18)),
    h("span", {}, freed > 0
      ? `${F.bytes(freed)} more is available than when this was measured.`
      : `${F.bytes(-freed)} less is available than when this was measured.`),
    button("Measure Again", () => act.loadSpace(true), { kind: "link" }));
}

function kindRow(k, act) {
  const notes = [k.reclaimable >= 500_000_000 ? `${F.bytes(k.reclaimable)} you may not need` : null].filter(Boolean);
  return h("button", { class: "row link-row", type: "button", onclick: () => act.show("storage", k.id) },
    tile(T.kindIcon(k.id), T.kindColor(k.id), 24),
    h("div", { class: "row-text" },
      h("div", { class: "row-title" }, k.title),
      notes.length ? h("div", { class: "row-sub" }, notes.join(" · ")) : null),
    h("div", { class: "row-end" },
      h("span", { class: "num secondary" }, F.bytes(k.bytes)),
      icon("chevron", 16, "secondary")));
}

/** What's freed without deleting anything: on Windows, the Recycle Bin. */
function comesBack(r, store, act) {
  const parts = r.comes_back.filter((p) => p.bytes >= (p.what === "trash" ? 100_000_000 : 500_000_000));
  if (!parts.length) return null;
  return section("Freed Without Deleting Anything", {},
    parts.map((p) => row({
      tile: [p.what === "trash" ? "trash" : "cycle", "var(--k-files)"],
      title: T.comesBackLabel(p.what),
      text: `${p.how.charAt(0).toUpperCase()}${p.how.slice(1)}.`,
      end: [
        h("span", { class: "num secondary" }, F.bytes(p.bytes)),
        p.what === "trash" ? binButtons(p.bytes, store, act) : null,
      ],
    })));
}

/** The Recycle Bin's: Empty (asked first) and, in ⋯, Open; Open alone where it can't be emptied from here. */
function binButtons(bytes, store, act) {
  const empty = T.emptyBin(store.status?.volume, bytes);
  if (!empty) return button("Open Recycle Bin", act.openRecycleBin);
  return [
    cleanButton(empty, store, act),
    menu([["Open Recycle Bin", act.openRecycleBin]], "More", "menu:recycle-bin"),
  ];
}

// -- A kind ------------------------------------------------------------------------

function kindPage(k, r, store, act, tools) {
  const place = T.placements(r, store.suggestions);
  const kept = (i) => [...store.kept].some((p) => T.within(i.path, p));
  const advised = (i) => (i.about?.bytes ?? 0) >= 500_000_000 && !i.flags.includes("kept");
  const all = store.opened.has(`all:${k.id}`);
  const shown = all ? k.items : k.items.filter((i, n) => n < FIRST || advised(i));
  const hidden = k.items.length - shown.length;
  const rest = k.bytes - shown.reduce((n, i) => n + i.bytes, 0);
  const appdata = k.id === "apps" ? r.kinds.find((x) => x.id === "appdata") : null;
  const head = h("div", { class: "kind-head" },
    button("Storage", () => act.show("storage"), { kind: "link", glyph: "back" }));
  return page(k.title, { sub: F.bytes(k.bytes), tools },
    head,
    section(null, {},
      row({
        tile: [T.kindIcon(k.id), T.kindColor(k.id)],
        title: k.title,
        text: k.reclaimable >= 500_000_000
          ? `${F.bytes(k.bytes)}, ${F.bytes(k.reclaimable)} of it you may not need`
          : F.bytes(k.bytes),
      }),
      appdata
        ? h("button", { class: "row link-row", type: "button", onclick: () => act.show("storage", "appdata") },
          h("div", { class: "row-text" }, h("div", { class: "row-title" }, "What apps keep (data, caches)")),
          h("div", { class: "row-end" }, h("span", { class: "secondary" }, `${appdata.title}, ${F.bytes(appdata.bytes)}`),
            icon("chevron", 16, "secondary")))
        : null),
    section(null, {},
      k.items.length ? null : note("Nothing big enough to list on its own."),
      shown.filter((i) => !kept(i)).map((i) => itemRow(i, k, place.get(i.path), store, act)),
      all || hidden > 0
        ? h("div", { class: "row more-row" },
          button(all ? "Show Fewer" : `Show ${hidden} More`, () => act.toggleOpened(`all:${k.id}`), { kind: "link" }))
        : null,
      rest >= 100_000_000
        ? labeled(hidden === 0 ? "Everything else, in folders too small to list on their own"
          : "Everything else, in the folders not shown and ones too small to list",
        h("span", { class: "num" }, F.bytes(rest)))
        : null));
}

/** One item: its name and marks, its size, its button, what's known (i), and ⋯. */
function itemRow(i, k, suggestion, store, act) {
  const id = `item:${i.path || i.name}`;
  const marks = T.marks(i);
  const sub = marks.length
    ? h("div", { class: "row-sub marks" }, marks.map(([text, tone]) => pill(text, tone)))
    : (suggestion && T.suggestionHeadline(suggestion) !== i.name ? h("div", { class: "row-sub" }, inline(suggestion.cause))
      : i.path && F.lastName(i.path) === i.name ? (i.note ? h("div", { class: "row-sub" }, inline(i.note)) : null)
        : h("div", { class: "row-sub path" }, i.path || i.note || ""));
  const action = suggestion?.action ?? i.action ?? (i.flags.includes("kept") ? null : i.about?.action);
  const about = i.flags.includes("kept") ? null : i.about;
  const story = Boolean(about || i.note || i.detail || i.known || suggestion);
  // Nothing's known about it: Ask AI, where it's on.
  const askable = !story && store.askAI && Boolean(i.path);
  const keepable = !i.flags.includes("kept") && T.mayGo(i);
  const end = [
    h("div", { class: "sizes" },
      h("span", { class: "num" }, F.bytes(i.bytes)),
      // Nothing to free (the system's own, a cloud copy's clones) says nothing.
      i.frees > 0 && i.frees + 500_000_000 < i.bytes
        ? h("span", { class: "num secondary small", title: "Part of it is shared with other files (hard links): deleting it frees only this much." },
          `frees ${F.bytes(i.frees)}`)
        : null),
    action ? cleanButton(action, store, act)
      : storageSettings(i.note) ? button("Storage Settings…", act.openStorageSettings,
        { kind: "link", title: "Settings → System → Storage: what Windows cleans itself", focus: `settings:${id}` })
        : null,
    story || askable
      ? iconButton(about ? "wand" : "info", about ? `${about.title}: how to make it smaller` : "More about it",
        () => act.toggleOpened(id), { expanded: store.opened.has(id), focus: `more:${id}` })
      : null,
    i.path
      ? menu([["Show in Explorer", () => act.reveal(i.path)], keepable ? ["Keep It", () => act.keep(i.path)] : null],
        "More", `menu:${id}`)
      : null,
  ];
  return h("div", { class: "row-group" },
    h("div", { class: "row" },
      tile(T.kindIcon(k.id), T.kindColor(k.id), 24),
      h("div", { class: "row-text" }, h("div", { class: "row-title", title: i.path || null }, i.name), sub),
      h("div", { class: "row-end" }, end)),
    store.opened.has(id) ? (askable ? askPanel(i, store, act) : advice(i, about, suggestion, act)) : null);
}

/**
 * Where nothing's known about a folder: "Ask AI…", which first shows what
 * would be sent (its name as `mdif` makes it, its size, the Windows
 * version) and sends it only on "Ask AI", to OpenAI on the ChatGPT plan of
 * whoever signed in ("Continue with ChatGPT" first).
 */
function askPanel(i, store, act) {
  const s = store.asks.get(i.path);
  const gpt = store.gpt.account;
  const usable = Boolean(gpt?.signed_in && gpt.plan);
  let body;
  switch (s?.step) {
    case "confirm":
      body = [
        h("p", {}, h("strong", {}, "Ask AI what it is?")),
        h("p", {}, usable ? "MyDiskIsFull sends only this to OpenAI, on your ChatGPT plan:"
          : gpt?.signed_in ? "MyDiskIsFull sends only this to OpenAI, on your ChatGPT plan, once you allow it:"
            : "MyDiskIsFull sends only this to OpenAI, on your ChatGPT plan (Plus or Pro), once you've signed in:"),
        h("div", { class: "sent" }, h("code", {}, s.q.name),
          h("div", { class: "secondary" }, [s.q.bytes ? F.bytes(s.q.bytes) : null, T.askedSystem(s.q)].filter(Boolean).join(" · "))),
        h("p", { class: "secondary small" }, "Nothing inside the folder, no other names, nothing about you. The answer is kept on this PC."),
        h("div", { class: "ask-buttons" },
          usable ? usingPlan(act) : null,
          h("span", { class: "spacer" }),
          button("Cancel", () => act.cancelAsk(i.path)),
          usable ? button("Ask AI", () => act.ask(i.path, i.bytes), { kind: "accent" }) : continueButton(store, act)),
        store.gpt.problem && !usable ? h("p", { class: "tone-orange small" }, store.gpt.problem) : null,
      ];
      break;
    case "asking":
      body = h("div", { class: "working secondary" }, h("span", { class: "spinner", "aria-hidden": "true" }), "Asking…");
      break;
    case "answer":
      body = s.a.known
        ? [
          s.a.what ? h("p", {}, inline(s.a.what)) : null,
          s.a.grows ? h("p", {}, h("strong", {}, "Why it grows: "), inline(s.a.grows)) : null,
          s.a.helps ? h("p", {}, h("strong", {}, "What helps: "), inline(s.a.helps)) : null,
          aiNote(s.a.by),
        ]
        : h("p", { class: "secondary" }, "The AI doesn't know what it is either.");
      break;
    case "failed":
      body = h("div", { class: "ask-buttons" }, h("p", { class: "secondary" }, s.why), h("span", { class: "spacer" }),
        /^Usage limit reached/.test(s.why)
          ? button("Manage Usage", act.openChatGPTUsage, { kind: "accent" })
          : button("Try Again", () => act.prepareAsk(i.path, i.bytes)));
      break;
    default:
      body = h("div", { class: "ask-buttons" }, h("p", { class: "secondary" }, "Nothing's known about it yet."),
        h("span", { class: "spacer" }), button("Ask AI…", () => act.prepareAsk(i.path, i.bytes), { glyph: "sparkles" }));
  }
  return h("div", { class: "more ask" }, body);
}

/** Under what the AI wrote: whose it is, and that nobody checked it. */
const aiNote = (by) => h("p", { class: "secondary small ai-note" }, icon("sparkles", 14),
  ` Written by AI (${by}) and not checked: it may be wrong.`);

/** What mdif says Windows' own Storage settings clean (temporary files, Windows.old). */
const storageSettings = (note) => /Settings → System → Storage/.test(note ?? "");

/** What's known about an item: its note, what it is, how its app makes it smaller, how it grew. */
function advice(i, about, suggestion, act) {
  const known = i.known;
  return h("div", { class: "more" },
    i.note ? h("p", { class: "secondary" }, inline(i.note)) : null,
    i.detail ? paragraphs(i.detail) : null,
    known?.what ? h("p", {}, inline(known.what)) : null,
    known?.grows ? h("p", {}, h("strong", {}, "Why it grows: "), inline(known.grows)) : null,
    known?.helps ? h("p", {}, h("strong", {}, "What helps: "), inline(known.helps)) : null,
    known?.ai ? aiNote(known.ai) : null,
    about
      ? h("div", { class: "about" },
        h("strong", {}, `${about.title}, ${F.bytes(about.bytes)}`),
        h("p", {}, inline(about.what)),
        h("p", {}, inline(about.how)))
      : null,
    suggestion ? h("p", {}, inline(suggestion.cause)) : null,
    suggestion && !suggestion.action && suggestion.fix.kind === "run"
      ? h("div", { class: "command" }, h("code", {}, suggestion.fix.detail.command),
        button("Copy", () => act.copy(suggestion.fix.detail.command), { glyph: "copy" }))
      : null,
    suggestion && !suggestion.action && suggestion.fix.kind === "steps" ? paragraphs(suggestion.fix.detail) : null);
}
