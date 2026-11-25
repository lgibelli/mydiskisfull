// What the pages say, worked out from what `mdif` printed: the Mac app's
// Headline, Health, SpaceText and Recommendations, for Windows. The wording
// is the Mac app's, so the two say the same thing.

import * as F from "./format.js";

// -- How the disk is doing ---------------------------------------------------

const ALERTS = { none: 0, "14_days": 1, "3_days": 2, "1_day": 3, low: 4 };

/** From least to most urgent, as `mdif` names them. */
export const alertRank = (a) => ALERTS[a] ?? 0;

/**
 * "learning" (no forecast yet), "calm", "watch" (could be full within two
 * weeks, or something runs away) or "urgent" (within three days, or below
 * the floor).
 */
export function health(s) {
  if (!s) return "learning";
  const soonest = Math.min(...s.incidents.map((e) => e.incident.full_in_days ?? Infinity));
  if (alertRank(s.alert) >= ALERTS["3_days"] || soonest <= 3) return "urgent";
  if (alertRank(s.alert) > 0 || s.incidents.length || (s.forecast?.full_in?.days ?? Infinity) <= 14) {
    return "watch";
  }
  if (!s.forecast?.rate) return "learning";
  return "calm";
}

/** The runaway that fills the disk soonest. */
function worst(s) {
  let best = null;
  for (const e of s.incidents) {
    const key = [e.incident.full_in_days ?? Infinity, -e.incident.per_day];
    if (!best || key[0] < best.key[0] || (key[0] === best.key[0] && key[1] < best.key[1])) {
      best = { e, key };
    }
  }
  return best?.e ?? null;
}

/** A folder by its name, a program as it is. */
export function subject(e) {
  const i = e.incident;
  return i.kind === "growth" ? F.lastName(i.subject) : (e.app ?? i.subject);
}

/** "target keeps growing", "OneDrive keeps rewriting the disk". */
export function episodeTitle(e) {
  return e.incident.kind === "growth"
    ? `${F.lastName(e.incident.subject)} keeps growing`
    : `${e.app ?? e.incident.subject} keeps rewriting the disk`;
}

/** One per episode, stable across refreshes. */
export const episodeId = (e) => `${e.incident.kind}:${e.incident.subject}:${e.opened}`;

export const headline = {
  title(s) {
    if (s.alert === "low") return "Your disk is almost full";
    const f = s.forecast;
    if (f?.full_in) {
      // Filling on average: when. Otherwise some days fill it and others
      // free it, and it only could be.
      return (f.rate?.per_day ?? 0) > 0
        ? `Full in about ${F.days(f.full_in.days)}`
        : `Could be full in about ${F.days(f.full_in.days)}`;
    }
    // The disk as a whole isn't filling, but something on it is: that
    // leads, or the status would contradict what's under it.
    const e = worst(s);
    if (e) {
      const d = e.incident.full_in_days;
      return d != null ? `${subject(e)} could fill it in ${F.days(d)}` : episodeTitle(e);
    }
    if (!f) return "Waiting for the first sample";
    if (!f.rate) return "Getting to know your disk";
    if (f.rate.per_day < 50_000_000) return "Not filling up";
    return "Filling slowly";
  },

  detail(s) {
    if (s.alert === "low") return s.alert_sentence ?? `Only ${F.bytes(s.avail)} left.`;
    const f = s.forecast;
    const e = worst(s);
    if (!f?.full_in && e) {
      const i = e.incident;
      let t = i.kind === "growth"
        ? `It grows ${F.bytes(i.per_day)} a day`
        : `It writes ${F.bytes(i.per_day)} a day`;
      if (f?.rate && f.rate.per_day <= 0) t += ", though free space overall held up";
      return `${t}.`;
    }
    if (!f) return "The watcher takes a sample every 5 minutes.";
    const r = f.rate;
    if (!r) {
      return `A forecast needs 8 samples over 3 hours; there are ${s.history.samples} so far.`;
    }
    if (f.full_in) {
      const date = F.date(f.at, f.full_in.days);
      let t;
      if (r.per_day > 0) {
        t = `Around ${date}, at ${F.bytes(r.per_day)} a day`;
        if ((f.purgeable ?? 0) >= 1_000_000_000) {
          t += `, counting the ${F.bytes(f.purgeable)} the system frees by itself`;
        }
        t += ".";
      } else {
        t = `Some days fill it and others free it. If the next days are like the last ones, it could be full by ${date}.`;
      }
      // How sure: the nearest horizon with a real chance.
      const p = f.full_by;
      if (p) {
        if (p.d1 >= 0.1) t += ` ${F.percent(p.d1)} likely within a day.`;
        else if (p.d3 >= 0.1) t += ` ${F.percent(p.d3)} likely within 3 days.`;
        else if (r.per_day <= 0) t += ` ${F.percent(p.d14)} likely within 2 weeks.`;
      } else if (f.recent_full_in) {
        t += ` At the last few hours' pace, in ${F.days(f.recent_full_in.days)}.`;
      }
      const a = (f.arrivals ?? []).reduce((m, x) => (!m || x.bytes > m.bytes ? x : m), null);
      if (a) {
        t += ` One-offs, like the ${F.bytes(a.bytes)} that arrived on ${F.day(a.ts)}, don't count as a pace.`;
      }
      return t;
    }
    // Free space that grew is clean-ups, not a pace to project.
    if (r.per_day < 0) return "Free space hasn't been shrinking lately.";
    if (r.per_day < 50_000_000) {
      return `Free space held steady over the ${F.trendWindow(r.window)} trend.`;
    }
    return `At ${F.bytes(r.per_day)} a day, there's room for a long time.`;
  },
};

/** "Disk C:", for the disk `mdif` names `C:\`. */
export function diskName(volume) {
  const m = /^([A-Za-z]:)\\?$/.exec(volume ?? "");
  return m ? `Disk ${m[1].toUpperCase()}` : (volume ?? "Disk");
}

// -- What the disk holds -----------------------------------------------------

/** Each kind's colour: a CSS variable, so it follows light and dark. */
export function kindColor(kind) {
  const known = [
    "macos", "apps", "build", "dev", "cloud", "vms", "media", "mail", "caches", "appdata", "files",
  ];
  return known.includes(kind) ? `var(--k-${kind})` : "var(--k-other)";
}

/** Each kind's icon (icons.js). */
export function kindIcon(kind) {
  return {
    macos: "windows",
    apps: "grid",
    build: "hammer",
    dev: "wrench",
    cloud: "cloud",
    vms: "pc",
    media: "photo",
    mail: "envelope",
    caches: "cycle",
    appdata: "tray",
    files: "doc",
  }[kind] ?? "more";
}

/** What's freed without a clean-up, by Windows' names. */
export function comesBackLabel(what) {
  return { trash: "In the Recycle Bin", swap: "Page file", purgeable: "Purgeable" }[what]
    ?? what.charAt(0).toUpperCase() + what.slice(1);
}

/** An item's marks, as words: text and tone. */
export function marks(i, now = Date.now() / 1000) {
  const out = [];
  const has = (f) => i.flags.includes(f);
  if (has("kept")) out.push(["Kept", "muted"]);
  if (has("unused")) {
    out.push([i.last_used ? `Not opened in ${F.age(now - i.last_used)}` : "Never opened", "orange"]);
  }
  if (has("cold") && i.age != null) out.push([`Unchanged for ${F.age(i.age)}`, "blue"]);
  if (has("build")) out.push(["Build output", "orange"]);
  if (has("cache")) out.push(["Cache", "yellow"]);
  return out;
}

/** What may go without much thought: unused, cold, build output, a cache. */
export const mayGo = (i) => ["unused", "cold", "build", "cache"].some((f) => i.flags.includes(f));

/**
 * The report's kinds as the bar's parts, biggest first (past `limit`, the
 * rest as one); then what comes back by itself, taken out of the kind that
 * counts it and drawn hatched; then what's free.
 */
export function stops(r, limit = Infinity) {
  const kinds = r.kinds.filter((k) => k.bytes > 0).sort((a, b) => b.bytes - a.bytes);
  const left = new Map(kinds.map((k) => [k.id, k.bytes]));
  const back = [];
  for (const b of r.comes_back.filter((c) => c.what !== "trash")) {
    const has = left.get(b.kind) ?? 0;
    if (b.bytes < 500_000_000 || !b.kind || has <= 0) continue;
    const n = Math.min(b.bytes, has);
    left.set(b.kind, has - n);
    back.push({ id: b.kind, title: comesBackLabel(b.what), bytes: n, color: "var(--k-files)", hatched: true });
  }
  const shown = kinds
    .slice(0, limit)
    .map((k) => ({ id: k.id, title: k.title, bytes: left.get(k.id) ?? 0, color: kindColor(k.id) }))
    .filter((s) => s.bytes > 0);
  const rest = kinds.slice(limit).reduce((n, k) => n + (left.get(k.id) ?? 0), 0);
  const others = rest > 0
    ? [{ id: null, title: "Everything else", bytes: rest, color: "var(--k-rest)" }]
    : [];
  return [
    ...shown,
    ...others,
    ...back,
    { id: null, title: "Free", bytes: r.volume.free, color: "var(--free)", muted: true },
  ];
}

// -- Paths -------------------------------------------------------------------

/** A path as `mdif` writes one at the start of a sentence. */
const PATH_START = /^(~[\\/]|[\\/]|[A-Za-z]:\\)/;

/** Windows' paths ignore case; a Mac's don't. */
const fold = (p) => (/^[A-Za-z]:\\|^\\\\/.test(p) ? p.toLowerCase() : p);

/** `child` is `parent` or in it. */
export function within(child, parent) {
  const c = fold(child);
  const p = fold(parent).replace(/[\\/]+$/, "");
  return c === p || c.startsWith(`${p}\\`) || c.startsWith(`${p}/`);
}

/** `child` is in `parent`, and isn't it. */
const inside = (child, parent) => within(child, parent) && !within(parent, child);

/** The same path (on Windows, whatever the case). */
export const samePath = (a, b) => within(a, b) && within(b, a);

// -- Suggestions --------------------------------------------------------------

/** A folder that keeps growing: its path, as the cause starts with it. */
function grower(s) {
  if (!PATH_START.test(s.cause)) return null;
  const at = s.cause.indexOf(" grew ");
  return at > 0 ? s.cause.slice(0, at) : null;
}

/** Whether it says to leave it be: nothing to do. */
export const leave = (s) => s.fix.kind === "leave";

/** A short title: a growing folder's name, or the cause's first sentence. */
export function suggestionHeadline(s) {
  if (s.title) return s.title;
  const g = grower(s);
  if (g) return `${F.lastName(g)} keeps growing`;
  const first = s.cause.split(". ")[0];
  return first.endsWith(".") ? first.slice(0, -1) : first;
}

/** The rest of the cause; null when the headline says it all. */
function suggestionDetail(s) {
  if (s.title) return null;
  if (grower(s)) return `Grew ${s.cause.slice(s.cause.indexOf(" grew ") + 6)}`;
  const parts = s.cause.split(". ");
  return parts.length > 1 ? parts.slice(1).join(". ") : null;
}

/** What to do, as far as its first sentence goes. */
function firstStep(s) {
  if (s.fix.kind !== "steps") return null;
  return s.fix.detail.split(/[.:;]/)[0] || null;
}

function suggestionIcon(s) {
  if (s.per_day != null) return ["flame", "var(--red)"];
  if (s.action) {
    return [s.action.kind === "worktrees" ? "branch" : s.action.kind === "restart" ? "restart" : "trash",
      "var(--blue)"];
  }
  return {
    tweak: ["wand", "var(--green)"],
    run: ["terminal", "var(--indigo)"],
    steps: ["checklist", "var(--blue)"],
    leave: ["leaf", "var(--teal)"],
  }[s.fix.kind] ?? ["info", "var(--blue)"];
}

/**
 * Each suggestion goes with the first item it's about (its folder, or one in
 * it or around it), biggest kinds first.
 * @returns {Map<string, object>} by the item's path
 */
export function placements(space, suggestions) {
  const out = new Map();
  if (!space || !suggestions) return out;
  const placed = new Set();
  for (const k of space.kinds) {
    for (const i of k.items) {
      if (!i.path) continue;
      const s = suggestions.find((s) => {
        if (!s.path || placed.has(s.cause)) return false;
        // About a folder, cleaning something in it (a merged worktree among
        // a repo's): about that, not its neighbours.
        const cleans = (s.action?.paths ?? []).filter((a) => inside(a, s.path));
        return (cleans.length ? cleans : [s.path]).some((a) => within(i.path, a) || within(a, i.path));
      });
      if (s) {
        out.set(i.path, s);
        placed.add(s.cause);
      }
    }
  }
  return out;
}

/** Smaller than this, a clean-up isn't worth a row. */
const MIN_ROW = 100_000_000;

/**
 * Everything there is to do, biggest first: clean-ups of what's on Storage,
 * the rest of the suggestions, the Recycle Bin. What runs away is under
 * Needs Attention instead.
 * @param {{space: any, suggestions: any[]|null, status: any}} m
 * @param {boolean} [grouped] three or more plain clean-ups of a kind as one row
 */
export function recommendations(m, grouped = true) {
  const r = m.space;
  if (!r) return [];
  const place = placements(r, m.suggestions);
  const out = [];
  for (const k of r.kinds) {
    const listed = k.items.filter((i) => !i.flags.includes("kept"));
    const plain = [];
    for (const i of listed) {
      if (!i.path) continue;
      // Inside one that is listed: it's counted there.
      const counted = listed.some(
        (o) => o.path && inside(i.path, o.path) && (o.action || place.get(o.path)),
      );
      if (counted) continue;
      const s = place.get(i.path);
      const tint = kindColor(k.id);
      const symbol = kindIcon(k.id);
      if (s) {
        if (leave(s)) continue;
        out.push(fromSuggestion(s, {
          id: i.path,
          title: samePath(s.path, i.path) ? i.name : suggestionHeadline(s),
          symbol,
          tint,
          bytes: (s.frees > 0 ? s.frees : null) ?? (i.frees > 0 ? i.frees : i.bytes),
          fallback: i.action,
        }));
      } else if (i.action) {
        plain.push({
          id: i.path, symbol, tint, title: i.name,
          detail: i.action.note ?? marks(i)[0]?.[0] ?? i.note,
          bytes: i.action.frees ?? (i.frees > 0 ? i.frees : i.bytes),
          act: { kind: "clean", action: i.action },
          path: i.path, keepable: mayGo(i), more: i.note,
        });
      } else if (i.about?.action) {
        out.push({
          id: i.path, symbol, tint, title: i.about.title, detail: i.name, bytes: i.about.bytes,
          act: { kind: "clean", action: i.about.action }, path: i.about.path, more: i.about.how,
        });
      }
    }
    const big = plain.filter((p) => (p.bytes ?? 0) >= MIN_ROW);
    if (grouped && big.length >= 3) out.push(group(k, big));
    else out.push(...big);
  }
  const placedCauses = new Set([...place.values()].map((s) => s.cause));
  const incidents = new Set((m.status?.incidents ?? []).map((e) => e.sentence));
  for (const s of m.suggestions ?? []) {
    if (placedCauses.has(s.cause) || incidents.has(s.cause) || leave(s)) continue;
    const [symbol, tint] = suggestionIcon(s);
    out.push(fromSuggestion(s, {
      id: s.cause, title: suggestionHeadline(s), symbol, tint,
      bytes: (s.frees > 0 ? s.frees : null) ?? s.space, fallback: null,
    }));
  }
  const bin = r.comes_back.find((c) => c.what === "trash")?.bytes ?? 0;
  if (bin >= MIN_ROW) {
    const empty = emptyBin(m.status?.volume, bin);
    out.push({
      id: "recycle-bin", symbol: "trash", tint: "var(--k-other)", title: "Empty the Recycle Bin",
      detail: "What's in it takes its space until it's emptied.", bytes: bin,
      act: empty ? { kind: "clean", action: empty } : { kind: "recycle-bin", label: "Open Recycle Bin" },
      menu: empty ? [["Open Recycle Bin", "openRecycleBin"]] : null,
    });
  }
  out.sort((a, b) => (b.bytes ?? 0) - (a.bytes ?? 0));
  // Then, as on the Mac: the tweaks not applied (biggest first) that no
  // suggestion has placed, and the clean-ups that could run and are off.
  const placedTweaks = new Set(out.filter((x) => x.act?.kind === "tweak").map((x) => x.act.id));
  const tweaks = (Array.isArray(m.tweaks) ? m.tweaks : [])
    .filter((t) => t.status !== "applied" && t.status !== "absent" && !placedTweaks.has(t.id))
    .sort((a, b) => (b.size ?? 0) - (a.size ?? 0));
  for (const t of tweaks) {
    out.push({
      id: `tweak:${t.id}`, symbol: "wand", tint: "var(--green)", title: t.title, detail: t.why,
      act: { kind: "tweak", id: t.id }, more: `${t.why}\n\nYou give up: ${t.cost}`,
    });
  }
  for (const t of Array.isArray(m.tidies) ? m.tidies : []) {
    if (t.on || !t.available) continue;
    out.push({
      id: `tidy:${t.id}`, symbol: "sparkles", tint: "var(--green)",
      title: `Clean up ${t.title.charAt(0).toLowerCase()}${t.title.slice(1)} every day`,
      detail: t.detail, act: { kind: "tidy", id: t.id },
    });
  }
  return out;
}

/**
 * Emptying the Recycle Bin of the watched drive (`volume`, as `C:\`), asked
 * first, in the Mac app's words for its Trash: what's in it is erased. `null`
 * when the volume isn't a drive's root.
 */
export function emptyBin(volume, bytes) {
  const m = /^([A-Za-z]:)\\?$/.exec(volume ?? "");
  if (!m) return null;
  return {
    kind: "empty-trash", label: "Empty Recycle Bin", paths: [`${m[1].toUpperCase()}\\$Recycle.Bin`],
    confirm: `The ${F.bytes(bytes)} in the Recycle Bin is erased for good. You can't undo this.`,
  };
}

// -- Clean Up ---------------------------------------------------------------------

/** What frees less than this isn't worth a row in Clean Up (the Mac app's). */
const CLEANUP_MIN = 100_000_000;

/** What Clean Up runs one after another: not a restart (it frees nothing), nor Docker's list. */
const batched = (a) => a.paths.length > 0 && a.done == null && a.kind !== "restart" && a.kind !== "docker";

/** Inside what `other` cleans: counted there. */
export function cleansWithin(r, other) {
  return r.action.paths.every((p) => other.action.paths.some((o) => inside(p, o)));
}

/**
 * Everything the app can clean itself, a row each, as the Mac app's Clean Up
 * lists it: Overview's recommendations that clean (groups opened up), then
 * what a listed item offers besides; nothing a ticked row cleans anyway,
 * nothing that frees under 100 MB. What comes back by itself (`safe`, ticked
 * to start with) first, then biggest first.
 */
export function cleanUpRows(m) {
  const out = [];
  const seen = new Set();
  const add = (symbol, tint, title, detail, bytes, a) => {
    if (!batched(a) || a.paths.some((p) => seen.has(fold(p)))) return;
    for (const p of a.paths) seen.add(fold(p));
    out.push({
      id: `${a.kind}:${a.paths.join("\n")}`, symbol, tint, title,
      detail: a.note ?? detail, bytes: a.frees ?? bytes, action: a,
    });
  };
  // An item's own clean-up goes by the item's name: a suggestion about a
  // folder may offer the clean-up of one in it.
  const owned = new Set((m.space?.kinds ?? []).flatMap((k) => k.items)
    .filter((i) => i.action).map((i) => i.action.paths.map(fold).join("\n")));
  for (const r of recommendations(m, false)) {
    if (r.act?.kind !== "clean") continue;
    const a = r.act.action;
    if (owned.has(a.paths.map(fold).join("\n")) && r.path && !a.paths.some((p) => samePath(p, r.path))) continue;
    add(r.symbol, r.tint, r.title, r.detail, r.bytes, a);
  }
  for (const k of m.space?.kinds ?? []) {
    for (const i of k.items) {
      if (i.flags.includes("kept")) continue;
      if (i.parts?.length) {
        for (const p of i.parts) add(kindIcon(k.id), kindColor(k.id), p.name, null, p.action.frees, p.action);
        continue;
      }
      const a = i.action;
      if (!a) continue;
      const bytes = a.frees ?? (i.frees > 0 ? i.frees : null);
      if (a.kind !== "docker-old" && (bytes ?? 0) < CLEANUP_MIN) continue;
      add(kindIcon(k.id), kindColor(k.id), i.name, i.note, a.kind === "docker-old" ? null : bytes, a);
    }
  }
  const big = out.filter((r) => r.action.kind === "docker-old" || (r.bytes ?? Infinity) >= CLEANUP_MIN);
  const rows = big.filter((r) => !big.some((o) => o.id !== r.id && o.action.safe && cleansWithin(r, o)));
  return rows.map((r, n) => [r, n]).sort(([a, x], [b, y]) => {
    if (Boolean(a.action.safe) !== Boolean(b.action.safe)) return a.action.safe ? -1 : 1;
    return (b.bytes ?? 0) - (a.bytes ?? 0) || x - y;
  }).map(([r]) => r);
}

/** Clean Up has run, and what each did is on its rows. */
export const cleanUpFinished = (c) => !c.running && c.states.size > 0;

/** What the ticked rows free together: one inside another ticked one counted there. */
export function cleanUpFrees(rows) {
  return rows.filter((r) => !rows.some((o) => o.id !== r.id && cleansWithin(r, o)))
    .reduce((n, r) => n + (r.bytes ?? 0), 0);
}

/** A kind's clean-ups as one row: how many, the biggest by name. */
function group(k, rows) {
  rows = [...rows].sort((a, b) => (b.bytes ?? 0) - (a.bytes ?? 0));
  const [title, noun] = {
    caches: ["Empty caches", "caches"],
    build: ["Delete build output", "folders"],
  }[k.id] ?? [`Clean up ${k.title.toLowerCase()}`, "items"];
  const names = rows.slice(0, 3).map((r) => r.title).join(", ");
  return {
    id: `kind:${k.id}`, symbol: kindIcon(k.id), tint: kindColor(k.id), title,
    detail: `${rows.length} ${noun}: ${names}${rows.length > 3 ? ` and ${rows.length - 3} more` : ""}`,
    bytes: rows.reduce((n, r) => n + (r.bytes ?? 0), 0),
    act: { kind: "kind", id: k.id, label: "Review…" },
  };
}

/** A suggestion as a row: its button, or the command to copy; the rest behind (i). */
function fromSuggestion(s, { id, title, symbol, tint, bytes, fallback }) {
  const r = {
    id, symbol, tint, title, bytes, path: s.path, keepable: Boolean(s.path),
    more: s.cause, sizes: s.sizes, act: null, command: null,
  };
  r.detail = suggestionDetail(s) ?? (s.title ? s.cause : null) ?? firstStep(s);
  if (r.detail === title) r.detail = null;
  const action = s.action ?? fallback;
  switch (s.fix.kind) {
    case "run":
      r.act = action ? { kind: "clean", action } : { kind: "copy", command: s.fix.detail.command };
      r.more = [s.cause, s.fix.detail.note].filter(Boolean).join("\n\n");
      r.command = s.fix.detail.command;
      break;
    case "steps":
      r.act = action ? { kind: "clean", action } : null;
      r.more = [s.cause, s.fix.detail].join("\n\n");
      break;
    case "leave":
      r.more = [s.cause, s.fix.detail].join("\n\n");
      break;
    case "tweak":
      // The setting that stops it: turned on after a look at what it changes.
      r.act = s.action ? { kind: "clean", action: s.action } : { kind: "tweak", id: s.fix.detail };
      break;
    default:
      r.act = s.action ? { kind: "clean", action: s.action } : null;
  }
  return r;
}

// -- What a clean-up did -------------------------------------------------------

/** What was done, a line each kind, and what the disk has more free. */
export function cleaned(r) {
  const lines = [];
  const noted = r.done.filter((d) => d.note);
  const rest = r.done.filter((d) => !d.note);
  lines.push(...noted.map((d) => (d.note.endsWith(".") ? d.note : `${d.note}.`)));
  const n = (how) => rest.filter((d) => d.how === how).length;
  const removed = n("removed");
  if (removed) {
    lines.push(removed === 1 ? "Removed the worktree; its branch stays."
      : `Removed ${removed} worktrees; their branches stay.`);
  }
  if (n("deleted")) lines.push("Deleted. The next build makes it again.");
  if (n("erased")) lines.push("Emptied the Recycle Bin.");
  if (n("trashed")) {
    lines.push(r.in_trash > 0
      ? `Moved to the Recycle Bin: its ${F.bytes(r.in_trash)} comes back once you empty it.`
      : "Moved to the Recycle Bin: its space comes back once you empty it.");
  }
  if (r.freed > 0) lines.push(`The disk has ${F.bytes(r.freed)} more free.`);
  return lines;
}

// -- Prevention ------------------------------------------------------------------

/** What applying a tweak changes and costs, as asked before it's applied (`mdif tweaks apply --dry-run`). */
export function tweakReview(plan) {
  return [
    plan.why,
    `You give up: ${plan.cost}`,
    `It changes:\n${plan.changes.map((c) => `• ${c}`).join("\n")}`,
    "MyDiskIsFull keeps what it replaces, so turning it off puts it back.",
  ].join("\n\n");
}

/** What undoing a tweak did (`mdif tweaks revert`). */
export function tweakUndone(r) {
  const parts = [r.restored.length
    ? `Put back:\n${r.restored.map((c) => `• ${c}`).join("\n")}`
    : "Nothing to put back: its settings were already as they were."];
  if (r.kept.length) parts.push(`Left as they are (changed since the tweak):\n${r.kept.map((c) => `• ${c}`).join("\n")}`);
  if (r.sign_out) parts.push("It takes effect once you sign out of Windows and back in.");
  if (r.restart) parts.push("It takes effect after a restart.");
  return parts.join("\n\n");
}

// -- Ask AI ----------------------------------------------------------------------

/** `mdif knowledge ask`'s arguments: the folder, its size if known, and `--dry-run` for what would be sent. */
export function askArgs(path, bytes, dryRun) {
  return ["knowledge", "ask", path, ...(bytes > 0 ? ["--bytes", String(Math.round(bytes))] : []),
    ...(dryRun ? ["--dry-run"] : [])];
}

/** `mdif`'s message, its first line, as a sentence. */
export function sentence(text) {
  const line = String(text).split("\n")[0].trim();
  const s = line.charAt(0).toUpperCase() + line.slice(1);
  return !s || /[.!?]$/.test(s) ? s : `${s}.`;
}

/** What a dry run says would be sent, under the system's name: "11 24H2" for `windows`. */
export const askedSystem = (q) => (q.windows != null ? `Windows ${q.windows}` : q.macos != null ? `macOS ${q.macos}` : "");

// -- Old Files -----------------------------------------------------------------

/** How big and how old Old Files lists, as the Mac app's pickers offer. */
export const OLD_SIZES = [50_000_000, 100_000_000, 500_000_000, 1_000_000_000, 5_000_000_000];
export const OLD_AGES = [[182, "6 months"], [365, "1 year"], [730, "2 years"], [1825, "5 years"]];

/** A drive's Recycle Bin, as the measurement lists it (`C:\$Recycle.Bin\<SID>`): it has its own button. */
const inRecycleBin = (path) => /[\\/]\$Recycle\.Bin[\\/][^\\/]+[\\/]?$/i.test(path);

/** The folder `path` is in. */
const folderOf = (path) => path.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]*$/, "") || path;

/**
 * What Old Files lists, biggest first: folders nothing changed for `days`
 * (the measurement's items marked cold) and big files of yours nothing used
 * for that long (`old_files`), each of at least `min` bytes, none kept.
 * `how` says what last happened to it: "Opened", or "Changed" (only that
 * where the disk doesn't record when a file was opened).
 */
export function oldRows(r, min, days, kept = new Set()) {
  const span = days * 86_400;
  const isKept = (p) => [...kept].some((k) => within(p, k));
  const folders = r.kinds.flatMap((k) => k.items)
    .filter((i) => i.path && i.flags.includes("cold") && !i.flags.includes("kept") && i.bytes >= min
      && (i.age ?? 0) >= span && !inRecycleBin(i.path) && !isKept(i.path))
    .map((i) => ({
      path: i.path, name: F.lastName(i.path) || i.name, folder: folderOf(i.path), bytes: i.bytes,
      last: r.measured - i.age, how: "Changed", action: i.action ?? null, isFolder: true,
    }));
  const unknown = Boolean(r.last_opened_unknown);
  const files = (r.old_files ?? [])
    .filter((f) => !f.kept && f.bytes >= min && Math.max(f.accessed, f.modified) <= r.measured - span
      && !isKept(f.path))
    .map((f) => {
      const opened = !unknown && f.accessed >= f.modified;
      return {
        path: f.path, name: f.name, folder: folderOf(f.path), bytes: f.bytes,
        last: opened ? f.accessed : f.modified, how: opened ? "Opened" : "Changed",
        action: f.action ?? null, isFolder: false,
      };
    });
  return [...folders, ...files].sort((a, b) => b.bytes - a.bytes || a.path.localeCompare(b.path));
}

/** All of `rows` to the Recycle Bin in one clean-up, if each can go there (`mdif clean` checks each again). */
export function recycleAll(rows) {
  if (!rows.length || !rows.every((x) => x.action?.kind === "trash")) return null;
  return { kind: "trash", label: "Move to Recycle Bin", paths: rows.flatMap((x) => x.action.paths) };
}

// -- Automatic clean-ups (`mdif tidy`) -------------------------------------------

/** What one did last: "Freed 3.2 GB on Oct 24.", "Nothing to tidy on Oct 24." */
export function tidyLast(l) {
  const day = F.day(l.at);
  if (l.freed >= 1_000_000) return `Freed ${F.bytes(l.freed)} on ${day}.`;
  return `${l.note ?? "Nothing to tidy"} on ${day}.`;
}

/** What running one now (`mdif tidy run <id>`) says, as a notice. */
export function tidyRunNotice(r) {
  const did = r.done.length > 0;
  const lines = [];
  if (did) lines.push(`The disk has ${F.bytes(r.freed)} more free.`);
  else if (!r.failed.length) lines.push("Nothing needed tidying.");
  if (r.failed.length) lines.push(`Not done:\n${r.failed.map((f) => `• ${f.why}`).join("\n")}`);
  return {
    ok: did || !r.failed.length,
    title: did ? "Tidied" : r.failed.length ? "Nothing was tidied" : "All tidy",
    text: lines.join("\n\n"),
    stays: r.failed.length > 0,
  };
}
