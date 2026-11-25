// The disk map's layouts and words (map.js), tested without a window, on
// made-up maps of a Windows drive and of a Mac.

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";

import * as M from "../js/map.js";

const GB = 1_000_000_000;

/** A folder as `mdif map --json` gives one: its size is its own and its folders'. */
function node(path, own, kids = []) {
  const total = own + kids.reduce((n, k) => n + k.totals.private, 0);
  return {
    path,
    totals: { private: total, dataless_logical: 0 },
    children: [...kids].sort((a, b) => b.totals.private - a.totals.private),
  };
}

const drive = node("C:\\", 2 * GB, [
  node("C:\\Windows", 20 * GB, [node("C:\\Windows\\WinSxS", 9 * GB)]),
  node("C:\\Users", 0, [
    node("C:\\Users\\me", 1 * GB, [
      node("C:\\Users\\me\\source", 0, [node("C:\\Users\\me\\source\\repos", 30 * GB)]),
      node("C:\\Users\\me\\Videos", 12 * GB),
    ]),
  ]),
  // Too small for a wedge of its own: with the rest.
  node("C:\\Tiny", 0.001 * GB),
]);

test("Windows' and a Mac's folders go up to their drive, share or /", () => {
  assert.equal(M.parentOf("C:\\Users\\me"), "C:\\Users");
  assert.equal(M.parentOf("C:\\Users"), "C:\\");
  assert.equal(M.parentOf("C:\\Users\\"), "C:\\");
  assert.equal(M.parentOf("C:\\"), null);
  assert.equal(M.parentOf("\\\\nas\\share\\films"), "\\\\nas\\share");
  assert.equal(M.parentOf("\\\\nas\\share"), null);
  assert.equal(M.parentOf("/Users/me"), "/Users");
  assert.equal(M.parentOf("/Users"), "/");
  assert.equal(M.parentOf("/"), null);
  assert.equal(M.userPath("/System/Volumes/Data/Users/me"), "/Users/me");
  assert.equal(M.userPath("/System/Volumes/Data"), "/");
  assert.equal(M.nodeName(drive, "Disk C:"), "Disk C:");
  assert.equal(M.nodeName(drive.children[0], "Disk C:"), "Users");
  assert.equal(M.nodeName({ path: "/System/Volumes/Data" }, "Macintosh HD"), "Macintosh HD");
});

test("growth adds up through every folder above it, Windows' whatever its case", () => {
  const g = M.growthIndex([
    { owner: "C:\\Users\\Me\\source\\repos", bytes: 4 * GB },
    { owner: "c:\\users\\me\\videos", bytes: -1 * GB },
    { owner: "/Users/me/Library/Caches", bytes: 2 * GB },
  ]);
  const at = (path) => M.growthOf(g, { path });
  assert.equal(at("C:\\Users\\me\\source\\repos"), 4 * GB);
  assert.equal(at("C:\\Users\\me\\source"), 4 * GB);
  assert.equal(at("C:\\Users\\me"), 3 * GB);
  assert.equal(at("C:\\"), 3 * GB);
  assert.equal(at("C:\\Windows"), undefined);
  assert.equal(at("/System/Volumes/Data/Users/me"), 2 * GB);
  assert.equal(at("/"), 2 * GB);
  // A Mac's paths keep their case.
  assert.equal(at("/users/me"), undefined);
  assert.equal(M.growthOf(g, null), undefined);
});

test("the breadcrumb follows a path down as far as the map goes", () => {
  const names = (path) => M.chain(drive, path).map((n) => n.path);
  assert.deepEqual(names(null), ["C:\\"]);
  assert.deepEqual(names("c:\\users\\me\\source\\repos"),
    ["C:\\", "C:\\Users", "C:\\Users\\me", "C:\\Users\\me\\source", "C:\\Users\\me\\source\\repos"]);
  // Gone from a new map: as far as it still goes.
  assert.deepEqual(names("C:\\Users\\me\\Music\\old"), ["C:\\", "C:\\Users", "C:\\Users\\me"]);
  // Not a folder whose name only begins the same.
  assert.deepEqual(names("C:\\Users\\meet"), ["C:\\", "C:\\Users"]);
  const mac = node("/System/Volumes/Data", 0, [node("/System/Volumes/Data/Users", 1 * GB)]);
  assert.deepEqual(M.chain(mac, "/System/Volumes/Data/Users").length, 2);
});

test("the sunburst's wedges share out each ring, the small ones as the rest", () => {
  const segs = M.segments(drive);
  const first = segs.filter((s) => s.depth === 1);
  // The whole circle, the tiny folder in the grey rest at the end.
  assert.ok(Math.abs(first.reduce((n, s) => n + (s.end - s.start), 0) - 2 * Math.PI) < 1e-9);
  assert.ok(!segs.some((s) => s.node?.path === "C:\\Tiny"));
  assert.equal(first[first.length - 1].node, null);
  // As wide as their share.
  const users = first.find((s) => s.node?.path === "C:\\Users");
  assert.ok(Math.abs((users.end - users.start) / (2 * Math.PI) - 43 / drive.totals.private * GB) < 1e-9);
  // A folder's wedges sit inside its own, in its hue, out to four rings.
  for (const s of segs.filter((x) => x.depth > 1 && x.node)) {
    const parent = segs.find((p) => p.depth === s.depth - 1 && p.node && s.node.path.startsWith(`${p.node.path}\\`));
    assert.ok(parent, s.node.path);
    assert.ok(s.start >= parent.start - 1e-9 && s.end <= parent.end + 1e-9, s.node.path);
    assert.equal(s.hue, parent.hue);
  }
  assert.ok(segs.every((s) => s.depth <= M.RINGS));
  assert.ok(segs.some((s) => s.depth === M.RINGS));
  assert.match(M.wedge(100, 100, 20, 40, 0, 2 * Math.PI), /^M[\d. L-]+Z$/);
});

test("the treemap's tiles fill their space in proportion, none over another", () => {
  const values = [50, 30, 8, 7, 3, 1, 0.5, 0.5];
  const rect = { x: 10, y: 20, w: 600, h: 400 };
  const laid = M.squarify(values.map((v, i) => [i, v]), rect);
  assert.equal(laid.length, values.length);
  const area = (r) => r.w * r.h;
  const total = values.reduce((n, v) => n + v, 0);
  for (const [i, r] of laid) {
    assert.ok(Math.abs(area(r) / area(rect) - values[i] / total) < 1e-9, `${i}`);
    assert.ok(r.x >= rect.x - 1e-9 && r.y >= rect.y - 1e-9, `${i}`);
    assert.ok(r.x + r.w <= rect.x + rect.w + 1e-6 && r.y + r.h <= rect.y + rect.h + 1e-6, `${i}`);
  }
  for (const [i, a] of laid) {
    for (const [j, b] of laid) {
      if (i >= j) continue;
      const overlap = Math.max(0, Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x))
        * Math.max(0, Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y));
      assert.ok(overlap < 1e-6, `${i} and ${j}`);
    }
  }
  assert.deepEqual(M.squarify([], rect), []);
  assert.deepEqual(M.squarify([["x", 1]], { x: 0, y: 0, w: 0, h: 10 }), []);
  // One level inside a group big enough, under its name's strip.
  const tiles = M.tiles(drive, { x: 0, y: 0, w: 800, h: 600 });
  const group = tiles.find((t) => t.depth === 1 && t.node?.path === "C:\\Users");
  const inner = tiles.filter((t) => t.depth === 2 && t.group === group.node);
  assert.ok(inner.length > 0);
  for (const t of inner) {
    assert.ok(t.rect.y >= group.rect.y + 17 - 1e-9 && t.rect.x >= group.rect.x + 2 - 1e-9);
    assert.equal(t.hue, group.hue);
  }
});

test("names are cut to what their place holds", () => {
  assert.equal(M.fit("Program Files (x86)", 200, 10), "Program Files (x86)");
  assert.equal(M.fit("Program Files (x86)", 50, 10), "Program …");
  assert.equal(M.fit("Program Files (x86)", 1, 10), "Pr…");
  // Along the arc at 12 o'clock; across the ring at 3; none on a short arc.
  assert.ok(Math.abs(M.labelRoom(1, 100, 60, 0) - 94) < 1e-9);
  assert.ok(Math.abs(M.labelRoom(1, 100, 60, Math.PI / 2) - 48) < 1e-9);
  assert.equal(M.labelRoom(0.4, 100, 60, 0), 0);
});

test("colours: a hue for each folder, lighter outwards; growth red, shrinking green, the rest grey", () => {
  assert.match(M.colour({}, "folder", M.hue(0), 1), /^hsl\(223\.2 /);
  assert.notEqual(M.colour({}, "folder", M.hue(0), 1), M.colour({}, "folder", M.hue(0), 3));
  assert.equal(M.hue(10), M.hue(0));
  const grew = M.colour({}, "growth", 0, 1, 10 * GB).match(/\d+/g).map(Number);
  const shrank = M.colour({}, "growth", 0, 1, -10 * GB).match(/\d+/g).map(Number);
  assert.ok(grew[0] > grew[1] && shrank[1] > shrank[0]);
  assert.equal(M.colour({}, "growth", 0, 1, 5_000_000), M.colour({}, "growth", 0, 1, undefined));
  assert.match(M.colour(null, "growth", 0, 1, 10 * GB), /0\.25\)$/);
});

test("the caption says what sizes are, of a PC or of a Mac, and the map's own notes", () => {
  const map = {
    scan: { root: { totals: { dataless_logical: 3 * GB } }, shared_once: 6 * GB },
    notes: ["A file held open for writing can be listed behind what it really takes."],
  };
  const pc = M.caption(map, true);
  assert.match(pc[0], /^Sizes are what deleting would free\. 6\.0 GB more is in files linked from elsewhere/);
  assert.match(pc[0], /3\.0 GB is in the cloud only, not on this PC\. Nothing is opened/);
  assert.equal(pc[1], map.notes[0]);
  assert.match(M.caption({ ...map, notes: undefined }, false)[0], /shared by clones and snapshots.*not on this Mac/);
  assert.equal(M.caption({ scan: { root: { totals: { dataless_logical: 0 } }, shared_once: 0 } }, true).length, 1);
});

test("the preview's map of a Windows drive is laid out whole, every folder back to its drive", () => {
  const map = JSON.parse(readFileSync(new URL("../../../app/Tests/map.json", import.meta.url), "utf8"));
  const root = map.scan.root;
  const segs = M.segments(root);
  const ring = segs.filter((s) => s.depth === 1);
  assert.ok(Math.abs(ring.reduce((n, s) => n + (s.end - s.start), 0) - 2 * Math.PI) < 1e-9);
  assert.ok(segs.every((s) => Number.isFinite(s.start) && s.end > s.start));
  const box = { x: 0, y: 0, w: 900, h: 560 };
  const tiles = M.tiles(root, box);
  assert.ok(tiles.length > 10);
  for (const { rect: r } of tiles) {
    assert.ok([r.x, r.y, r.w, r.h].every(Number.isFinite), JSON.stringify(r));
    assert.ok(r.x >= -1e-6 && r.y >= -1e-6 && r.x + r.w <= box.w + 1e-6 && r.y + r.h <= box.h + 1e-6, JSON.stringify(r));
  }
  const walk = (n) => [n, ...(n.children ?? []).flatMap(walk)];
  for (const n of walk(root)) {
    const trail = M.chain(root, n.path);
    assert.equal(trail[trail.length - 1], n, n.path);
    assert.equal(trail.length, n.path.split("\\").filter(Boolean).length, n.path);
  }
  assert.deepEqual(M.caption(map, true).slice(1), map.notes);
});
