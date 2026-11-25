// What the window works out from `mdif`'s output, tested without a window:
// `node --test app-windows/ui/tests/*.test.mjs`. The fixtures are the Mac app's
// (app/Tests), so both apps read the same output the same way.

import assert from "node:assert/strict";
import fs from "node:fs";
import { test } from "node:test";

import * as F from "../js/format.js";
import * as T from "../js/text.js";
import { layout, leader } from "../js/rail.js";

const fixture = (name) =>
  JSON.parse(fs.readFileSync(new URL(`../../../app/Tests/${name}`, import.meta.url)));

test("numbers read as the Mac app writes them", () => {
  assert.equal(F.bytes(24_800_000_000), "24.8 GB");
  assert.equal(F.bytes(23_400_000), "23 MB");
  assert.equal(F.bytes(-2_900_000_000), "-2.9 GB");
  assert.equal(F.bytes(512), "512 B");
  assert.equal(F.days(0.2), "5 hours");
  assert.equal(F.days(1), "1 day");
  assert.equal(F.days(12.4), "12 days");
  assert.equal(F.days(200), "7 months");
  assert.equal(F.age(3 * 86400), "3 days");
  assert.equal(F.age(400 * 86400), "13 months");
  assert.equal(F.age(800 * 86400), "2 years");
  assert.equal(F.change(10_000_000), "Quiet");
  assert.equal(F.change(3_400_000_000), "Filled 3.4 GB");
  assert.equal(F.change(-2_900_000_000), "Freed 2.9 GB");
  assert.equal(F.trendWindow(3 * 86400), "3-day");
  assert.equal(F.trendWindow(6 * 3600), "6-hour");
  assert.equal(F.lastName("C:\\Users\\me\\devel\\app\\target"), "target");
  assert.equal(F.lastName("/Users/me/Library/Caches/"), "Caches");
});

test("Windows' paths are matched without regard to case, a Mac's with it", () => {
  assert.ok(T.within("C:\\Users\\Me\\devel\\x", "c:\\users\\me\\devel"));
  assert.ok(T.within("C:\\Users\\me", "C:\\Users\\me\\"));
  assert.ok(!T.within("C:\\Users\\meet", "C:\\Users\\me"));
  assert.ok(T.within("/Users/me/x", "/Users/me"));
  assert.ok(!T.within("/Users/Me/x", "/Users/me"));
  assert.equal(T.diskName("C:\\"), "Disk C:");
  assert.equal(T.diskName("d:"), "Disk D:");
});

test("the headline says what the Mac app's says", () => {
  const s = fixture("status-incidents.json");
  assert.equal(T.health(s), "urgent");
  assert.equal(T.headline.title(s), `Full in about ${F.days(s.forecast.full_in.days)}`);
  assert.match(T.headline.detail(s), /^Around .+, at .+ a day/);
  // Almost full leads, whatever else is going on.
  assert.equal(T.headline.title({ ...s, alert: "low" }), "Your disk is almost full");
  assert.equal(T.headline.detail({ ...s, alert: "low" }), s.alert_sentence);
  // No forecast: the worst runaway leads.
  const noForecast = { ...s, alert: "none", forecast: { ...s.forecast, full_in: null } };
  assert.equal(T.headline.title(noForecast), "DerivedData could fill it in 3 days");
  // Nothing yet.
  const fresh = { ...s, alert: "none", forecast: null, incidents: [] };
  assert.equal(T.headline.title(fresh), "Waiting for the first sample");
  assert.equal(T.health(fresh), "learning");
});

test("an episode is named and told apart as on the Mac", () => {
  const [growth, churn] = fixture("status-incidents.json").incidents;
  assert.equal(T.episodeTitle(growth), "DerivedData keeps growing");
  assert.equal(T.episodeTitle(churn), `${churn.app ?? churn.incident.subject} keeps rewriting the disk`);
  assert.equal(T.episodeTitle({ ...churn, app: null }), `${churn.incident.subject} keeps rewriting the disk`);
  assert.equal(T.episodeId(growth), `growth:${growth.incident.subject}:${growth.opened}`);
});

test("the bar's parts: kinds biggest first, the rest as one, then what's free", () => {
  const r = fixture("space.json");
  const stops = T.stops(r, 6);
  const kinds = r.kinds.filter((k) => k.bytes > 0);
  assert.equal(stops.at(-1).title, "Free");
  assert.equal(stops.at(-1).bytes, r.volume.free);
  assert.ok(stops.at(-1).muted);
  const shown = stops.filter((s) => s.id && !s.hatched);
  assert.equal(shown.length, 6);
  for (let i = 1; i < shown.length; i++) assert.ok(shown[i - 1].bytes >= shown[i].bytes || shown[i - 1].id !== shown[i].id);
  assert.ok(stops.some((s) => s.title === "Everything else"));
  // Every byte counted once: the parts add up to what's used, and what's free.
  const sum = stops.reduce((n, s) => n + s.bytes, 0);
  assert.equal(sum, kinds.reduce((n, k) => n + k.bytes, 0) + r.volume.free);
});

test("the bar's names: never overlapping on a side, inside the width, lines never crossing", () => {
  const stops = [5e9, 80e9, 1e9, 1e9, 2e9, 60e9, 1e8, 3e9, 9e9].map((bytes, i) => ({ bytes, w: 40 + (i % 3) * 25 }));
  const width = 640;
  const l = layout(stops, 300e9, width, (s) => s.w);
  for (const up of [true, false]) {
    const side = l.labels.filter((p) => p.up === up).sort((a, b) => a.x - b.x);
    side.forEach((p, i) => {
      assert.ok(p.x - p.width / 2 >= -1e-9 && p.x + p.width / 2 <= width + 1e-9, "inside");
      if (i) assert.ok(side[i - 1].x + side[i - 1].width / 2 + 10 <= p.x - p.width / 2 + 1e-9, "apart");
    });
  }
  // The last part, what's free, takes what's left.
  assert.equal(l.spans.at(-1)[1], width);
  // Straight up when it can, bent at 45° when it can't.
  assert.equal(leader(10, 10, 66, 32, 0, true), "M10 66L10 32");
  assert.equal(leader(10, 40, 66, 32, 1, true), "M10 66L10 52L16 46L34 46L40 40L40 32");
});

test("recommendations: biggest first, a suggestion with what it's about, the Recycle Bin last of its size", () => {
  const space = fixture("space.json");
  const suggestions = fixture("suggest-actions.json");
  const status = fixture("status-incidents.json");
  const list = T.recommendations({ space, suggestions, status });
  assert.ok(list.length > 0);
  for (let i = 1; i < list.length; i++) assert.ok((list[i - 1].bytes ?? 0) >= (list[i].bytes ?? 0));
  // Each suggestion shows once, with the item it's about or on its own.
  const causes = list.map((r) => r.more ?? "").filter((m) => suggestions.some((s) => m.startsWith(s.cause)));
  assert.equal(new Set(causes).size, causes.length);
  // A kept item has no row.
  const kept = space.kinds.flatMap((k) => k.items).filter((i) => i.flags.includes("kept")).map((i) => i.path);
  assert.ok(list.every((r) => !kept.includes(r.path)));
  // The bin, when there's enough in it.
  const bin = { ...space, comes_back: [{ what: "trash", bytes: 2_000_000_000, how: "", kind: "files" }] };
  const withBin = T.recommendations({ space: bin, suggestions, status });
  // A Mac's volume isn't a drive: the bin is only opened.
  assert.equal(withBin.find((r) => r.id === "recycle-bin")?.act.kind, "recycle-bin");
});

test("the Recycle Bin of the watched drive is emptied, asked first", () => {
  const space = { kinds: [], comes_back: [{ what: "trash", bytes: 2_000_000_000, how: "", kind: "files" }] };
  const [row] = T.recommendations({ space, suggestions: [], status: { volume: "c:\\", incidents: [] } });
  assert.equal(row.act.kind, "clean");
  assert.deepEqual(row.act.action.paths, ["C:\\$Recycle.Bin"]);
  assert.equal(row.act.action.kind, "empty-trash");
  assert.match(row.act.action.confirm, /^The 2\.0 GB in the Recycle Bin is erased for good\. You can't undo this\.$/);
  assert.deepEqual(row.menu, [["Open Recycle Bin", "openRecycleBin"]]);
  assert.equal(T.emptyBin("D:", 1)?.paths[0], "D:\\$Recycle.Bin");
  for (const volume of [undefined, "/System/Volumes/Data", "C:\\Mount\\Data", "\\\\server\\share\\"]) {
    assert.equal(T.emptyBin(volume, 1), null, String(volume));
  }
  assert.deepEqual(T.cleaned({ done: [{ path: "C:\\$Recycle.Bin", how: "erased" }], failed: [], freed: 2e9, in_trash: 0 }),
    ["Emptied the Recycle Bin.", "The disk has 2.0 GB more free."]);
});

test("Clean Up lists what the app can clean, what comes back by itself first", () => {
  const space = fixture("space.json");
  const suggestions = fixture("suggest-actions.json");
  const status = { ...fixture("status-incidents.json"), volume: "C:\\" };
  const bin = { ...space, comes_back: [{ what: "trash", bytes: 2_000_000_000, how: "", kind: "files" }] };
  const rows = T.cleanUpRows({ space: bin, suggestions, status });
  assert.ok(rows.length > 0);
  assert.equal(new Set(rows.map((r) => r.id)).size, rows.length, "a row each");
  // Ticked first, then biggest first.
  const safe = rows.map((r) => Boolean(r.action.safe));
  assert.deepEqual(safe, [...safe].sort((a, b) => Number(b) - Number(a)));
  for (const part of [rows.filter((r) => r.action.safe), rows.filter((r) => !r.action.safe)]) {
    for (let i = 1; i < part.length; i++) assert.ok((part[i - 1].bytes ?? 0) >= (part[i].bytes ?? 0));
  }
  // Nothing a ticked row cleans anyway: the incremental caches are in the target that goes.
  for (const r of rows) {
    assert.ok(!rows.some((o) => o.id !== r.id && o.action.safe && T.cleansWithin(r, o)), r.id);
  }
  assert.ok(!rows.some((r) => r.action.paths[0].endsWith("/incremental")));
  // Each path once; no restart, no Docker list, nothing under 100 MB.
  const paths = rows.flatMap((r) => r.action.paths);
  assert.equal(new Set(paths).size, paths.length);
  assert.ok(rows.every((r) => !["restart", "docker"].includes(r.action.kind)));
  assert.ok(rows.every((r) => r.action.kind === "docker-old" || (r.bytes ?? 0) >= 100_000_000));
  // The Recycle Bin needs a look: unticked, and what can't be undone said.
  const empty = rows.find((r) => r.action.kind === "empty-trash");
  assert.ok(empty && !empty.action.safe && /can't undo/.test(empty.action.confirm));
  // Together: one inside another counted once.
  const target = rows.find((r) => r.action.paths[0].endsWith("app/src-tauri/target"));
  const inner = { id: "x", bytes: 5e9, action: { paths: [`${target.action.paths[0]}/debug`] } };
  assert.equal(T.cleanUpFrees([target, inner]), target.bytes);
  assert.equal(T.cleanUpFinished({ running: false, states: new Map([["a", {}]]) }), true);
  assert.equal(T.cleanUpFinished({ running: true, states: new Map([["a", {}]]) }), false);
});

test("a suggestion about a Windows folder goes with its item", () => {
  const space = {
    kinds: [{
      id: "build", title: "Build output", bytes: 5e9, frees: 5e9, reclaimable: 5e9,
      items: [{
        path: "C:\\Users\\me\\devel\\app\\target", name: "app\\target", bytes: 5e9, frees: 5e9,
        flags: ["build"], action: null,
      }],
    }],
    comes_back: [],
  };
  const s = {
    cause: "~\\devel\\app\\target grew 3.0 GB in the last 2 days (1.5 GB a day).",
    fix: { kind: "run", detail: { command: "cargo clean --manifest-path 'C:\\Users\\me\\devel\\app\\Cargo.toml'", note: "" } },
    path: "c:\\users\\me\\devel\\app\\target", per_day: 1.5e9,
    action: { kind: "delete", label: "Delete Build Output", paths: ["C:\\Users\\me\\devel\\app\\target"], confirm: "x" },
  };
  const place = T.placements(space, [s]);
  assert.equal(place.get(space.kinds[0].items[0].path), s);
  const [row] = T.recommendations({ space, suggestions: [s], status: { incidents: [] } });
  assert.equal(row.title, "app\\target");
  assert.equal(row.detail, "Grew 3.0 GB in the last 2 days (1.5 GB a day).");
  assert.equal(row.act.kind, "clean");
  assert.equal(T.suggestionHeadline(s), "target keeps growing");
});

test("what a clean-up did, in the Mac app's words", () => {
  assert.deepEqual(T.cleaned({ done: [{ path: "a", how: "deleted" }], failed: [], freed: 3e9, in_trash: 0 }),
    ["Deleted. The next build makes it again.", "The disk has 3.0 GB more free."]);
  assert.deepEqual(T.cleaned({ done: [{ path: "a", how: "removed" }, { path: "b", how: "removed" }], failed: [], freed: 0, in_trash: 0 }),
    ["Removed 2 worktrees; their branches stay."]);
});

test("old files: big and unused for as long as asked, kept ones and the bin left out", () => {
  const DAY = 86_400;
  const now = 2_000_000_000;
  const file = (name, bytes, openedDays, changedDays, extra = {}) => ({
    path: `C:\\Users\\Ann\\Videos\\${name}`, name, bytes, frees: bytes, kept: false,
    accessed: now - openedDays * DAY, modified: now - changedDays * DAY,
    action: { kind: "trash", label: "Move to Recycle Bin", paths: [`C:\\Users\\Ann\\Videos\\${name}`] },
    ...extra,
  });
  const r = {
    measured: now,
    kinds: [{
      id: "files", items: [
        { path: "C:\\Users\\Ann\\Old Projects", name: "Old Projects", bytes: 3e9, flags: ["cold"], age: 800 * DAY },
        { path: "C:\\Users\\Ann\\Fresh", name: "Fresh", bytes: 3e9, flags: ["cold"], age: 100 * DAY },
        { path: "C:\\$Recycle.Bin\\S-1-5-21-1", name: "Recycle Bin", bytes: 9e9, flags: ["cold"], age: 900 * DAY },
        { path: "C:\\Users\\Ann\\Kept", name: "Kept", bytes: 3e9, flags: ["cold", "kept"], age: 900 * DAY },
      ],
    }],
    old_files: [
      file("trip.mkv", 2e9, 400, 900),
      file("watched.mkv", 2e9, 30, 900),
      file("small.mkv", 60e6, 400, 900),
      file("mine.mkv", 2e9, 400, 900, { kept: true }),
    ],
  };
  const rows = T.oldRows(r, 100e6, 365);
  assert.deepEqual(rows.map((x) => x.name), ["Old Projects", "trip.mkv"]);
  assert.equal(rows[0].how, "Changed");
  assert.equal(rows[0].isFolder, true);
  assert.equal(rows[1].how, "Opened");
  assert.equal(rows[1].last, now - 400 * DAY);
  assert.equal(rows[1].folder, "C:\\Users\\Ann\\Videos");
  // Kept since the window opened, by the file or its folder.
  assert.deepEqual(T.oldRows(r, 100e6, 365, new Set(["C:\\Users\\Ann\\Videos"])).map((x) => x.name), ["Old Projects"]);
  // Smaller and newer, when asked.
  assert.deepEqual(T.oldRows(r, 50e6, 182).map((x) => x.name), ["Old Projects", "trip.mkv", "small.mkv"]);
  // Where Windows doesn't record opening: what changed, and said so.
  const unknown = T.oldRows({ ...r, last_opened_unknown: true }, 100e6, 365);
  assert.equal(unknown[1].how, "Changed");
  assert.equal(unknown[1].last, now - 900 * DAY);

  // Files go to the bin together; a folder among them can't.
  assert.deepEqual(T.recycleAll([rows[1]]).paths, ["C:\\Users\\Ann\\Videos\\trip.mkv"]);
  assert.equal(T.recycleAll(rows), null);
  assert.equal(T.recycleAll([]), null);
});

test("ask AI: what's asked, and mdif's message as a sentence", () => {
  assert.deepEqual(T.askArgs("C:\\Users\\Ann\\AppData\\Local\\X", 1_734_567_890.4, true),
    ["knowledge", "ask", "C:\\Users\\Ann\\AppData\\Local\\X", "--bytes", "1734567890", "--dry-run"]);
  assert.deepEqual(T.askArgs("C:\\x", 0, false), ["knowledge", "ask", "C:\\x"]);
  assert.equal(T.sentence("can't ask about ~\\Documents: its name could say something about you\nmore"),
    "Can't ask about ~\\Documents: its name could say something about you.");
  assert.equal(T.sentence("Usage limit reached."), "Usage limit reached.");
  assert.equal(T.askedSystem({ name: "x", windows: "11 24H2" }), "Windows 11 24H2");
  assert.equal(T.askedSystem({ name: "x", macos: "27.2" }), "macOS 27.2");
});

test("prevention: a tweak's fix is turned on from Overview; its review and its undoing say what changes", () => {
  const tweak = {
    cause: "The Recycle Bin takes 3.4 GB. Files in the Recycle Bin take space until it is emptied.",
    fix: { kind: "tweak", detail: "recycle-bin-auto-empty" },
    space: 3_400_000_000, title: "Empty the Recycle Bin automatically after 30 days",
    path: null, action: null, sizes: [],
  };
  const space = { kinds: [], comes_back: [] };
  const [row] = T.recommendations({ space, suggestions: [tweak], status: { incidents: [] } });
  assert.deepEqual(row.act, { kind: "tweak", id: "recycle-bin-auto-empty" });
  assert.equal(row.bytes, 3_400_000_000);

  const review = T.tweakReview({
    why: "Why.", cost: "Cost.", changes: ["A: not set → 1", "B: 0 → 7"],
  });
  assert.equal(review, "Why.\n\nYou give up: Cost.\n\nIt changes:\n• A: not set → 1\n• B: 0 → 7\n\n"
    + "MyDiskIsFull keeps what it replaces, so turning it off puts it back.");
  assert.equal(T.tweakUndone({ restored: ["A: 1 → not set"], kept: [], sign_out: true, restart: false }),
    "Put back:\n• A: 1 → not set\n\nIt takes effect once you sign out of Windows and back in.");
  assert.equal(T.tweakUndone({ restored: [], kept: ["B: 14"], sign_out: false, restart: false }),
    "Nothing to put back: its settings were already as they were.\n\n"
    + "Left as they are (changed since the tweak):\n• B: 14");
});

test("prevention: what's said beside a tweak's title", async () => {
  const { tweakNote } = await import("../js/pages/prevention.js");
  assert.deepEqual(tweakNote({ status: "partly applied", applied: null }), ["Partly applied", "orange"]);
  assert.deepEqual(tweakNote({ status: "applied", applied: null }), ["Already done", "secondary"]);
  assert.equal(tweakNote({ status: "applied", applied: 1_790_000_000 }), null);
  assert.deepEqual(tweakNote({ status: "not applied", applied: 1_790_000_000 }), ["Undone since", "purple"]);
  assert.equal(tweakNote({ status: "not applied", applied: null }), null);
});

test("an automatic clean-up says what it did last, and a run now says what it freed", () => {
  const at = Date.UTC(2026, 9, 24, 12) / 1000;
  const day = F.day(at);
  assert.equal(T.tidyLast({ at, freed: 3_200_000_000, done: 2, failed: 0, note: "Deleted …" }),
    `Freed 3.2 GB on ${day}.`);
  assert.equal(T.tidyLast({ at, freed: 0, done: 0, failed: 0, note: "Nothing to tidy" }), `Nothing to tidy on ${day}.`);
  assert.equal(T.tidyLast({ at, freed: 0, done: 0, failed: 1, note: null }), `Nothing to tidy on ${day}.`);

  const freed = T.tidyRunNotice({ done: [{ bytes: 1_310_000_000 }, { bytes: 420_000_000 }], failed: [], freed: 1_730_000_000 });
  assert.deepEqual(freed, { ok: true, title: "Tidied", text: "The disk has 1.7 GB more free.", stays: false });
  const nothing = T.tidyRunNotice({ done: [], failed: [], freed: 0 });
  assert.deepEqual(nothing, { ok: true, title: "All tidy", text: "Nothing needed tidying.", stays: false });
  const refused = T.tidyRunNotice({ done: [], failed: [{ why: "Quit the app first." }], freed: 0 });
  assert.deepEqual(refused, { ok: false, title: "Nothing was tidied", text: "Not done:\n• Quit the app first.", stays: true });
  const some = T.tidyRunNotice({ done: [{ bytes: 5e8 }], failed: [{ why: "a" }, { why: "b" }], freed: 5e8 });
  assert.equal(some.title, "Tidied");
  assert.equal(some.ok, true);
  assert.equal(some.stays, true);
  assert.equal(some.text, "The disk has 500 MB more free.\n\nNot done:\n• a\n• b");
});

test("Overview recommends, after the clean-ups, the tweaks not applied and the clean-ups that are off", () => {
  const m = {
    space: fixture("space.json"),
    suggestions: fixture("suggest-actions.json"),
    status: fixture("status-incidents.json"),
  };
  const before = T.recommendations(m);
  const tweaks = [
    { id: "small", title: "Small", why: "w1", cost: "c1", status: "not applied", applied: null, size: 1e8 },
    { id: "big", title: "Big", why: "w2", cost: "c2", status: "partly applied", applied: null, size: 9e9 },
    { id: "done", title: "Done", why: "w3", cost: "c3", status: "applied", applied: null, size: 5e9 },
    { id: "absent", title: "Absent", why: "w4", cost: "c4", status: "absent", applied: null, size: 0 },
  ];
  const tidies = [
    { id: "builds", title: "Old build folders", detail: "d1", on: false, available: false, unavailable_why: "x", last: null },
    { id: "node-modules", title: "Old node_modules", detail: "d2", on: true, available: true, unavailable_why: null, last: null },
    { id: "incremental", title: "Old Rust build caches", detail: "d3", on: false, available: true, unavailable_why: null, last: null },
  ];
  const after = T.recommendations({ ...m, tweaks, tidies });
  // What there was stays first, in its order.
  assert.deepEqual(after.slice(0, before.length).map((r) => r.id), before.map((r) => r.id));
  const added = after.slice(before.length);
  assert.deepEqual(added.map((r) => r.id), ["tweak:big", "tweak:small", "tidy:incremental"]);
  assert.deepEqual(added[0].act, { kind: "tweak", id: "big" });
  assert.equal(added[0].more, "w2\n\nYou give up: c2");
  assert.equal(added[2].title, "Clean up old Rust build caches every day");
  assert.deepEqual(added[2].act, { kind: "tidy", id: "incremental" });
  // A list that failed to load adds nothing; nor do they reach Clean Up.
  assert.deepEqual(T.recommendations({ ...m, tweaks: { error: "x" }, tidies: { error: "y" } }).map((r) => r.id),
    before.map((r) => r.id));
  assert.ok(T.cleanUpRows({ ...m, tweaks, tidies }).every((r) => !/^(tweak|tidy):/.test(r.id)));
});
