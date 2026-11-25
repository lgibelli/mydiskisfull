// The window: what's known (`store`), what the buttons do (`act`), and the
// page shown. Everything comes from `mdif` (core.js); what an action did is
// said in the banner at the foot of the window, never in a dialog at the end.
// Only the question before what can't be undone is a dialog.

import { app, inApp, mdifJson, on, version } from "./core.js";
import { h, icon, paragraphs } from "./dom.js";
import * as F from "./format.js";
import * as T from "./text.js";
import { overview } from "./pages/overview.js";
import { storage } from "./pages/storage.js";
import { oldFiles } from "./pages/oldfiles.js";
import { history } from "./pages/history.js";
import { prevention } from "./pages/prevention.js";
import { settings } from "./pages/settings.js";
import { welcome } from "./pages/welcome.js";
import { cleanUpSheet } from "./pages/cleanup.js";
import { measuringText } from "./pages/folders.js";

/** A measurement younger than this is shown as it is (the Mac app's). */
const SPACE_MAX_AGE = 6 * 3600;
/** Suggestions older than this are worked out again when the window comes back. */
const SUGGEST_MAX_AGE = 30 * 60;
/** A disk map older than this is measured again when Folders opens (the Mac app's)… */
const MAP_STALE_ON_OPEN = 30 * 60;
/** …and this often while it stays on screen. */
const MAP_REFRESH_EVERY = 60 * 60;

export const store = {
  /** `mdif status --json`. */
  status: null,
  statusError: null,
  /** `mdif space --json`. */
  space: null,
  spaceError: null,
  spaceBusy: false,
  /** `mdif suggest --json`, what's kept since left out. */
  suggestions: null,
  suggestBusy: false,
  /** When the suggestions were worked out, in seconds. */
  measuredAt: 0,
  /** `mdif forecast --json --series`. */
  detail: null,
  /** `mdif agent status --json`. */
  agent: null,
  watcherBusy: false,
  /** The app's own settings, and whether it opens at sign-in ("yes", "off", "no"). */
  settings: null,
  login: null,
  /** Kept since the window opened: gone from the lists at once. */
  kept: new Set(),
  /** Clean-ups under way, by their first path. */
  cleaning: new Set(),
  /** The app's version. */
  version: null,
  page: "overview",
  /** The kind Storage shows, if one. */
  kind: null,
  /** Rows opened to their whole story, by id. */
  opened: new Set(),
  /** Recommendations: all of them, or the first few. */
  allShown: false,
  /**
   * Clean Up, once opened: its rows, what's ticked (`chosen`), what each did
   * (`states`), whether it's running or stopping, what it freed, the
   * measurement its list is from, and whether it's on screen (`shown`).
   */
  cleanup: null,
  /** Storage by "categories" or "folders" (the disk map), as last chosen. */
  storageMode: localStorage.getItem("storage.mode") === "folders" ? "folders" : "categories",
  /**
   * The disk map (`mdif map --json`, read), when it was measured, whether a
   * measurement runs and since when (`started`, null while it waits for
   * another), why and when the last one failed, and the reading of the kept
   * one (a promise, read once).
   */
  map: { data: null, measured: null, running: false, started: null, error: null, failed: null, kept: null },
  /** How the map is shown: its chart, colouring, the folder in focus (a path), the list's open folders. */
  mapView: { style: "sunburst", colouring: "folder", focus: null, opened: new Set() },
  /** Where the keyboard goes after the next drawing (a `data-focus`), when an act says. */
  focusNext: null,
  /** Old Files: how big and how old (kept for next time), and what's ticked, by path. */
  oldFilter: {
    minBytes: storedNumber("oldfiles.minBytes", T.OLD_SIZES, 100_000_000),
    minDays: storedNumber("oldfiles.minDays", T.OLD_AGES.map(([d]) => d), 365),
  },
  oldPicked: new Set(),
  /** `mdif tidy`'s clean-ups (`{ error }` when it failed), and those being switched or run. */
  tidies: null,
  tidyBusy: new Set(),
  /** `mdif tweaks` (`{ error }` when it failed), and those being turned on (true) or off (false). */
  tweaks: null,
  tweaksChanging: new Map(),
  /**
   * Ask AI: whether it's offered (kept), the ChatGPT account (`mdif
   * knowledge account`), whether a sign-in waits for the browser, what went
   * wrong last; and each folder's question, by path: `{ step: "confirm" |
   * "asking" | "answer" | "failed", q, a, why }`.
   */
  askAI: localStorage.getItem("askAI") !== "off",
  gpt: { account: null, signingIn: false, problem: null },
  asks: new Map(),
};

/** A number kept in localStorage, if it's one of `allowed`; `fallback` otherwise. */
function storedNumber(key, allowed, fallback) {
  const n = Number(localStorage.getItem(key));
  return allowed.includes(n) ? n : fallback;
}

const PAGES = { overview, storage, oldfiles: oldFiles, history, prevention, settings };

// -- Drawing -------------------------------------------------------------------

let pending = false;
/** Draws the page again, once, when the screen next does. */
export function render() {
  if (pending) return;
  pending = true;
  requestAnimationFrame(() => {
    pending = false;
    draw();
  });
}

let shownPage = null;
function draw() {
  // Focus on a control survives the redraw when the control is still there.
  const focused = store.focusNext ?? document.activeElement?.dataset?.focus;
  store.focusNext = null;
  const refocus = (where) => {
    if (focused) where.querySelector(`[data-focus="${CSS.escape(focused)}"]`)?.focus();
  };
  // The first window covers the rest until Get Started.
  const first = document.getElementById("welcome");
  const welcoming = store.settings?.welcomed === false;
  const opening = welcoming && first.hidden;
  first.hidden = !welcoming;
  document.querySelector(".shell").inert = welcoming;
  if (welcoming) {
    first.replaceChildren(welcome(store, act));
    refocus(first);
    if (opening) first.querySelector("button")?.focus();
    return;
  }
  first.replaceChildren();
  drawCleanUp(refocus);
  const main = document.getElementById("page");
  const keep = shownPage === store.page + (store.kind ?? "") ? main.scrollTop : 0;
  main.replaceChildren(PAGES[store.page](store, act));
  main.scrollTop = keep;
  shownPage = store.page + (store.kind ?? "");
  refocus(main);
  for (const b of document.querySelectorAll(".nav-item")) {
    b.setAttribute("aria-current", b.dataset.page === store.page ? "page" : "false");
  }
  drawFoot();
}

/** Clean Up, while it's on screen: its list follows the disk until it has run. */
function drawCleanUp(refocus) {
  const c = store.cleanup;
  const d = document.getElementById("cleanup");
  if (!c?.shown) {
    d.replaceChildren();
    return;
  }
  if ((store.space?.measured ?? null) !== c.measured) loadCleanUp({ keepTicks: true });
  d.replaceChildren(cleanUpSheet(store, act));
  refocus(d);
  // Opened, the dialog itself has the focus (it was empty then): its first
  // control takes it; once it has run, Done, as on the Mac.
  const at = document.activeElement;
  if (at === d || !d.contains(at)) {
    (d.querySelector('[data-focus="cleanup:done"]')
      ?? d.querySelector("input:not([disabled]), button:not([disabled])"))?.focus();
  }
}

/** Everything the app can clean itself now, what's kept since left out. */
function cleanUpList() {
  return T.cleanUpRows(store)
    .filter((r) => !r.action.paths.some((p) => [...store.kept].some((k) => T.within(p, k))));
}

/**
 * Clean Up's list as the disk is now, unless it's running or what it did is
 * on its rows (`force`: Look Again). With `keepTicks` (the disk measured again
 * while it's open), what was ticked or unticked stays so: only a row new to
 * the list starts ticked or not by itself.
 */
function loadCleanUp({ force = false, keepTicks = false } = {}) {
  store.cleanup ??= {
    rows: [], chosen: new Set(), states: new Map(), running: false, stopping: false, freed: 0,
    measured: null, shown: false,
  };
  const c = store.cleanup;
  if (c.running || (!force && c.states.size)) return;
  const seen = new Set(c.rows.map((r) => r.id));
  c.states = new Map();
  c.freed = 0;
  c.rows = cleanUpList();
  c.measured = store.space?.measured ?? null;
  const ids = new Set(c.rows.map((r) => r.id));
  const safe = c.rows.filter((r) => r.action.safe).map((r) => r.id);
  c.chosen = keepTicks
    ? new Set([...[...c.chosen].filter((id) => ids.has(id)), ...safe.filter((id) => !seen.has(id))])
    : new Set(safe);
}

/** The disk and the watcher, at the sidebar's foot. */
function drawFoot() {
  const s = store.status;
  const foot = document.getElementById("foot");
  if (!s) {
    foot.replaceChildren();
    return;
  }
  const used = 1 - s.avail / Math.max(s.total, 1);
  const tone = { learning: "accent", calm: "accent", watch: "orange", urgent: "red" }[T.health(s)];
  const running = store.agent?.running;
  foot.replaceChildren(
    h("div", { class: "foot-disk" },
      icon("disk", 16),
      h("span", {}, T.diskName(s.volume)),
      h("span", { class: "num secondary" }, `${F.bytes(s.avail)} free`)),
    h("div", { class: `capacity tone-${tone}`, role: "img", "aria-label": `${F.percent(used)} used` },
      h("span", { style: { width: `${Math.min(100, used * 100)}%` } })),
    h("div", { class: `foot-watcher ${running === false ? "off" : ""}` },
      h("span", { class: "dot" }),
      running === false ? "Not watching" : running ? "Watching" : "…"));
}


// -- What's said ---------------------------------------------------------------

/**
 * A notice at the foot of the window: what an action did. It goes by itself
 * after a while, unless it says what wasn't done (`stays`).
 */
export function notice(ok, title, text = "", stays = false) {
  const box = document.getElementById("notices");
  const close = () => n.remove();
  const n = h("div", { class: `notice ${ok ? "ok" : "bad"}`, role: ok ? "status" : "alert" },
    h("span", { class: "notice-icon" }, icon(ok ? "check" : "warning", 18)),
    h("div", { class: "notice-text" }, h("strong", {}, title), paragraphs(text)),
    h("button", { class: "btn icon-only", type: "button", "aria-label": "Close", title: "Close", onclick: close },
      icon("close", 14)));
  box.append(n);
  // At most three at once.
  while (box.children.length > 3) box.firstElementChild.remove();
  if (!stays) setTimeout(close, 8000);
}

/**
 * Asks before what can't be undone (`danger`), or before a change of
 * settings, saying what it changes.
 */
function confirmDialog(title, text, label, { danger = true } = {}) {
  const d = document.getElementById("confirm");
  d.querySelector(".dialog-title").textContent = title;
  d.querySelector(".dialog-text").replaceChildren(...paragraphs(text));
  const go = d.querySelector(".dialog-go");
  go.textContent = label;
  go.className = `btn ${danger ? "danger" : "accent"} dialog-go`;
  d.returnValue = "";
  d.showModal();
  d.querySelector(".dialog-cancel").focus();
  return new Promise((done) => {
    d.addEventListener("close", () => done(d.returnValue === "go"), { once: true });
  });
}

async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
    notice(true, "Copied", "Paste it into PowerShell or Windows Terminal. MyDiskIsFull never runs it itself.");
  } catch (e) {
    notice(false, "Couldn't copy it", String(e?.message ?? e));
  }
}

const message = (e) => String(e?.message ?? e);

/** Reads of `mdif tweaks` started: only the last one's answer is kept. */
let tweaksRead = 0;

// -- What the buttons do ---------------------------------------------------------

export const act = {
  show(page, kind = null) {
    store.page = page;
    store.kind = kind;
    if (foldersShown()) act.openMap();
    if (page === "settings" && store.askAI) act.loadAccount();
    if (page === "prevention") act.loadPrevention();
    render();
    document.getElementById("page").focus({ preventScroll: true });
  },

  /** Old Files' size or age (kept for next time); what's ticked stays ticked only if still listed. */
  setOldFilter(key, value) {
    store.oldFilter[key] = value;
    localStorage.setItem(`oldfiles.${key}`, String(value));
    render();
  },

  toggleOldPick(path) {
    if (!store.oldPicked.delete(path)) store.oldPicked.add(path);
    render();
  },

  clearOldPicks() {
    store.oldPicked.clear();
    render();
  },

  /** Never suggested again (`mdif keep add`), several at once. */
  async keepAll(paths) {
    try {
      await mdifJson(["keep", "add", ...paths], 30);
      for (const p of paths) {
        store.kept.add(p);
        store.oldPicked.delete(p);
      }
      notice(true, paths.length === 1 ? `Kept ${F.lastName(paths[0])}` : `Kept ${paths.length} items`,
        "MyDiskIsFull won't suggest deleting, cleaning or moving them. Settings lists what you keep.");
      render();
    } catch (e) {
      notice(false, "Couldn't keep it", message(e));
    }
  },

  /** Storage by category, or folder by folder (kept for next time). */
  setStorageMode(mode) {
    store.storageMode = mode;
    localStorage.setItem("storage.mode", mode);
    if (foldersShown()) act.openMap();
    render();
  },

  /**
   * Folders on screen: the kept map, the first time; growth from the
   * watcher's history; and a measurement when the map is older than half an
   * hour (or there's none).
   */
  async openMap() {
    const m = store.map;
    act.loadDetail();
    // Read once; whoever asks meanwhile waits for it, so the age it's judged
    // by is the kept map's.
    m.kept ??= (async () => {
      try {
        const kept = await app("kept_map");
        if (kept && !m.data) useMap(kept);
      } catch (e) {
        console.warn("kept map:", message(e));
      }
      render();
    })();
    await m.kept;
    refreshMapIfOld(MAP_STALE_ON_OPEN);
  },

  /** Maps the disk. `automatic`: nobody asked, so at idle priority. */
  async measureMap(automatic = false) {
    const m = store.map;
    if (m.running) return;
    m.running = true;
    m.started = null;
    m.error = null;
    render();
    const ticking = setInterval(tickMeasuring, 1000);
    try {
      useMap(await oneAtATime(async () => {
        // Its turn came after another measurement: by itself, only if
        // someone is still looking.
        if (automatic && !(foldersShown() && (await app("window_shown").catch(() => false)))) throw SKIPPED;
        m.started = Date.now() / 1000;
        render();
        return app("measure_map", { automatic });
      }));
      m.failed = null;
    } catch (e) {
      if (e !== SKIPPED) {
        m.error = message(e);
        m.failed = Date.now() / 1000;
      }
    } finally {
      clearInterval(ticking);
      m.running = false;
      render();
    }
    act.loadDetail();
  },

  mapStyle(style) {
    store.mapView.style = style;
    render();
  },

  mapColouring(colouring) {
    store.mapView.colouring = colouring;
    render();
  },

  /**
   * The folder the map shows (null: the disk). `keys`: asked from the
   * keyboard, so the keyboard goes to the folder's name in the breadcrumb.
   */
  mapFocus(path, keys = false) {
    store.mapView.focus = path;
    if (keys && path != null) store.focusNext = `crumb:${path}`;
    render();
  },

  /** A folder in the list opened, or closed again. */
  mapToggle(path) {
    const o = store.mapView.opened;
    if (o.has(path)) o.delete(path);
    else o.add(path);
    render();
  },

  toggleOpened(id) {
    if (store.opened.has(id)) store.opened.delete(id);
    else store.opened.add(id);
    render();
  },

  showAll(on) {
    store.allShown = on;
    render();
  },

  copy,

  async reveal(path) {
    try {
      await app("reveal", { path });
    } catch (e) {
      notice(false, "Couldn't show it", message(e));
    }
  },

  async openRecycleBin() {
    try {
      await app("open_recycle_bin");
    } catch (e) {
      notice(false, "Couldn't open the Recycle Bin", message(e));
    }
  },

  async openStorageSettings() {
    try {
      await app("open_storage_settings");
    } catch (e) {
      notice(false, "Couldn't open Storage settings", message(e));
    }
  },

  // -- Ask AI ------------------------------------------------------------------

  setAskAI(on) {
    store.askAI = on;
    localStorage.setItem("askAI", on ? "on" : "off");
    if (on) act.loadAccount();
    render();
  },

  async loadAccount() {
    try {
      store.gpt.account = await mdifJson(["knowledge", "account"], 10);
    } catch (e) {
      store.gpt.problem = T.sentence(message(e));
    }
    render();
  },

  /** In the browser, which `mdif` waits for up to 5 minutes. */
  async signIn() {
    if (store.gpt.signingIn) return;
    store.gpt.signingIn = true;
    store.gpt.problem = null;
    render();
    try {
      store.gpt.account = await mdifJson(["knowledge", "sign-in"], 330);
      // Once, after the first sign-in that lets questions use the plan, as OpenAI asks.
      if (store.gpt.account?.plan && !localStorage.getItem("chatgptWelcomed")) {
        localStorage.setItem("chatgptWelcomed", "yes");
        notice(true, "You're using your ChatGPT plan",
          "Eligible usage in this app uses your ChatGPT plan. Manage usage in your ChatGPT settings.", true);
      }
    } catch (e) {
      store.gpt.problem = T.sentence(message(e));
    } finally {
      store.gpt.signingIn = false;
      render();
    }
  },

  async signOut() {
    store.gpt.problem = null;
    let r = null;
    try {
      r = await mdifJson(["knowledge", "sign-out"], 60);
    } catch (e) {
      store.gpt.problem = T.sentence(message(e));
    }
    await act.loadAccount();
    if (r && !r.revoked) {
      store.gpt.problem = "Signed out here, but OpenAI didn't confirm it: you can disconnect MyDiskIsFull in ChatGPT's settings.";
      render();
    }
  },

  async openChatGPTUsage() {
    try {
      await app("open_chatgpt_usage");
    } catch (e) {
      notice(false, "Couldn't open ChatGPT's settings", message(e));
    }
  },

  /** What would be sent about a folder, from `mdif` itself: what's shown is what goes. */
  async prepareAsk(path, bytes) {
    act.loadAccount();
    try {
      const q = await mdifJson(T.askArgs(path, bytes, true), 30);
      store.asks.set(path, { step: "confirm", q });
    } catch (e) {
      store.asks.set(path, { step: "failed", why: T.sentence(message(e)) });
    }
    render();
  },

  /** Sent: the folder's name, its size, the Windows version; the answer is kept beside the history. */
  async ask(path, bytes) {
    store.asks.set(path, { step: "asking" });
    render();
    try {
      store.asks.set(path, { step: "answer", a: await mdifJson(T.askArgs(path, bytes, false), 120) });
    } catch (e) {
      store.asks.set(path, { step: "failed", why: T.sentence(message(e)) });
    }
    render();
  },

  cancelAsk(path) {
    store.asks.delete(path);
    render();
  },

  // -- Prevention ----------------------------------------------------------------

  loadPrevention() {
    act.loadTidies();
    act.loadTweaks();
  },

  /** The automatic clean-ups: whether each is on, and what each did last. */
  async loadTidies() {
    try {
      store.tidies = (await mdifJson(["tidy"], 30)).tidies;
    } catch (e) {
      store.tidies = { error: T.sentence(message(e)) };
    }
    render();
  },

  /** Turns one on or off; `mdif tidy` gives the list as it is then. */
  async setTidy(id, on) {
    if (store.tidyBusy.has(id)) return;
    store.tidyBusy.add(id);
    render();
    try {
      store.tidies = (await mdifJson(["tidy", on ? "on" : "off", id], 30)).tidies;
    } catch (e) {
      notice(false, "Couldn't change it", message(e));
    }
    store.tidyBusy.delete(id);
    render();
  },

  /** Runs one now, instead of at the watcher's daily turn: what it did goes to a notice. */
  async tidyNow(id) {
    if (store.tidyBusy.has(id)) return;
    store.tidyBusy.add(id);
    render();
    let r = null;
    try {
      r = await mdifJson(["tidy", "run", id], 1800);
    } catch (e) {
      notice(false, "Nothing was tidied", message(e));
    }
    store.tidyBusy.delete(id);
    if (!r) {
      render();
      return;
    }
    const n = T.tidyRunNotice(r);
    notice(n.ok, n.title, n.text, n.stays);
    await act.loadTidies();
    if (r.done.length) {
      act.refresh();
      act.measure();
      act.loadSpace(true);
    }
  },

  /**
   * Reads each tweak's state (and measures what it's about). A read started
   * before a tweak changed would put back its old state: only the last one
   * started counts.
   */
  async loadTweaks() {
    const read = ++tweaksRead;
    let list;
    try {
      list = await mdifJson(["tweaks"], 120);
    } catch (e) {
      list = { error: T.sentence(message(e)) };
    }
    if (read !== tweaksRead) return;
    store.tweaks = list;
    render();
  },

  /** What it changes and costs first (a dry run), then applied: what happened goes to a notice. */
  async applyTweak(id) {
    if (store.tweaksChanging.has(id)) return;
    store.tweaksChanging.set(id, true);
    render();
    try {
      const plan = await mdifJson(["tweaks", "apply", id, "--dry-run"], 60);
      if (!plan.changes.length) {
        notice(true, plan.title, "Already applied: there is nothing to change.");
        return;
      }
      const ok = await confirmDialog(plan.title, T.tweakReview(plan), "Apply", { danger: false });
      if (!ok) return;
      try {
        const done = await mdifJson(["tweaks", "apply", id], 60);
        notice(true, done.title, done.next ?? "Done.");
      } catch (e) {
        notice(false, "The tweak wasn't applied", `${message(e)}\n\nAnything it had changed was put back.`);
      }
    } catch (e) {
      notice(false, "Can't apply this tweak", message(e));
    } finally {
      store.tweaksChanging.delete(id);
      await act.loadTweaks();
      // Its suggestion says it's applied now.
      if (store.suggestions) act.measure();
    }
  },

  /** Puts back what it replaced; a setting changed since is left as it is. */
  async revertTweak(id, title) {
    if (store.tweaksChanging.has(id)) return;
    store.tweaksChanging.set(id, false);
    render();
    try {
      const ok = await confirmDialog(`Undo “${title}”?`,
        "MyDiskIsFull puts back the settings it replaced. A setting changed since then is left as it is.",
        "Undo", { danger: false });
      if (!ok) return;
      const r = await mdifJson(["tweaks", "revert", id], 60);
      notice(true, `Undone: ${r.title}`, T.tweakUndone(r));
    } catch (e) {
      notice(false, "Couldn't undo it", message(e));
    } finally {
      store.tweaksChanging.delete(id);
      await act.loadTweaks();
    }
  },

  async openStartupApps() {
    try {
      await app("open_startup_apps");
    } catch (e) {
      notice(false, "Couldn't open Startup apps", message(e));
    }
  },

  /** Reads the status now; the app sends it back as an event. */
  refresh() {
    app("refresh_now");
  },

  /**
   * The breakdown; `refresh` measures again (a minute or so for a full
   * disk). Otherwise the last measurement shows at once, and if it's old,
   * it's measured again behind it.
   */
  async loadSpace(refresh = false) {
    if (store.spaceBusy) return;
    store.spaceBusy = true;
    render();
    try {
      if (refresh) {
        store.space = await oneAtATime(() => mdifJson(["space", "--refresh"], 900));
      } else {
        // However old; measured now only if there's none.
        store.space = await mdifJson(["space", "--max-age", "2147483647"], 900);
        render();
        if (Date.now() / 1000 - store.space.measured > SPACE_MAX_AGE) {
          store.space = await oneAtATime(() => mdifJson(["space", "--max-age", String(SPACE_MAX_AGE)], 3600));
        }
      }
      store.spaceError = null;
    } catch (e) {
      store.spaceError = message(e);
    } finally {
      store.spaceBusy = false;
      render();
    }
  },

  /** What to do about what fills the disk: `mdif suggest` sizes the folders it names. */
  async measure() {
    if (store.suggestBusy) return;
    store.suggestBusy = true;
    // The tweaks with it, as the Mac app reads them: Overview recommends those not applied.
    act.loadTweaks();
    render();
    try {
      store.suggestions = await mdifJson(["suggest"], 300);
      store.measuredAt = Date.now() / 1000;
    } catch (e) {
      console.warn("suggest:", message(e));
    } finally {
      store.suggestBusy = false;
      render();
    }
  },

  async loadDetail() {
    try {
      store.detail = await mdifJson(["forecast", "--series"], 60);
      render();
    } catch (e) {
      console.warn("forecast:", message(e));
    }
  },

  async loadAgent() {
    try {
      store.agent = await mdifJson(["agent", "status"], 30);
      render();
    } catch (e) {
      console.warn("agent status:", message(e));
    }
  },

  /** Starts the watcher at sign-in and now (`mdif agent install`), or stops it. */
  async setWatching(on) {
    store.watcherBusy = true;
    render();
    try {
      await app("mdif", { args: ["agent", on ? "install" : "uninstall"], timeoutSecs: 120 });
      notice(true, on ? "Watching" : "Stopped watching",
        on
          ? "The watcher takes a sample every 5 minutes and starts at every sign-in."
          : "Nothing is measured until it's started again. The history is kept.");
    } catch (e) {
      notice(false, on ? "Couldn't start the watcher" : "Couldn't stop the watcher", message(e), true);
    } finally {
      store.watcherBusy = false;
      await act.loadAgent();
      act.refresh();
    }
  },

  /** Clean Up, opened: the list as the disk is now, or what the last run did. */
  openCleanUp() {
    loadCleanUp();
    store.cleanup.shown = true;
    const d = document.getElementById("cleanup");
    if (!d.open) d.showModal();
    render();
  },

  closeCleanUp() {
    document.getElementById("cleanup").close();
  },

  tickCleanUp(id, on) {
    const c = store.cleanup;
    if (on) c.chosen.add(id);
    else c.chosen.delete(id);
    render();
  },

  tickAllCleanUp(on) {
    const c = store.cleanup;
    c.chosen = new Set(on ? c.rows.map((r) => r.id) : []);
    render();
  },

  /** Stops after the one under way. */
  stopCleanUp() {
    store.cleanup.stopping = true;
    render();
  },

  lookAgainCleanUp() {
    loadCleanUp({ force: true });
    render();
  },

  /**
   * What's ticked, one after the other, each through `mdif clean`; what each
   * did on its row. Closed meanwhile, it goes on, and says at the end what
   * it did in a banner.
   */
  async runCleanUp() {
    const c = store.cleanup;
    const picked = c.rows.filter((r) => c.chosen.has(r.id));
    if (c.running || !picked.length) return;
    Object.assign(c, { running: true, stopping: false, freed: 0, states: new Map() });
    render();
    let done = 0;
    let failed = 0;
    for (const r of picked) {
      if (c.stopping) break;
      c.states.set(r.id, { state: "running" });
      store.cleaning.add(r.action.paths[0]);
      render();
      try {
        const out = await mdifJson(["clean", ...r.action.paths], 1800);
        if (!out.done.length) {
          c.states.set(r.id, { state: "failed", text: out.failed[0]?.why ?? "Nothing was done." });
          failed += 1;
        } else {
          done += 1;
          // Partly done: its row says what wasn't, and so does the banner.
          if (out.failed.length) failed += 1;
          c.freed += out.freed;
          // What it did, without the total: that's at the foot.
          const said = T.cleaned(out).filter((l) => !l.startsWith("The disk has"))
            .concat(out.failed.map((f) => `Not done: ${f.why}`));
          c.states.set(r.id, { state: "done", text: said.join(" ") || "Done." });
        }
      } catch (e) {
        c.states.set(r.id, { state: "failed", text: message(e) });
        failed += 1;
      } finally {
        store.cleaning.delete(r.action.paths[0]);
        render();
      }
    }
    Object.assign(c, { running: false, stopping: false });
    if (!c.shown) {
      notice(failed === 0, done ? "Cleaned up" : "Nothing was cleaned",
        [done ? `The disk has ${F.bytes(c.freed)} more free.` : "",
          failed ? `${failed} couldn't be done: open Clean Up to see why.` : ""].filter(Boolean).join(" "),
        failed > 0);
    }
    render();
    if (done) {
      act.refresh();
      act.measure();
      act.loadSpace(true);
    }
  },

  /** A clean-up: asked first when it can't be undone, then `mdif clean`, which checks each path again. */
  async clean(a) {
    if (a.confirm && !(await confirmDialog(`${a.label}?`, a.confirm, a.label))) return;
    // Each path shows it's under way (several at once from Old Files).
    for (const p of a.paths) store.cleaning.add(p);
    render();
    let r;
    try {
      // Removing build output can be hundreds of thousands of files.
      r = await mdifJson(["clean", ...a.paths], 1800);
    } catch (e) {
      notice(false, "Nothing was cleaned", message(e), true);
      return;
    } finally {
      for (const p of a.paths) store.cleaning.delete(p);
      render();
    }
    // What went is gone from Old Files at once, before the disk is measured again.
    const gone = new Set(r.done.map((d) => d.path));
    if (store.space?.old_files) store.space.old_files = store.space.old_files.filter((f) => !gone.has(f.path));
    for (const p of gone) store.oldPicked.delete(p);
    const lines = T.cleaned(r);
    if (r.failed.length) lines.push(`Not done:\n${r.failed.map((f) => `• ${f.why}`).join("\n")}`);
    notice(r.done.length > 0, r.done.length ? "Done" : "Nothing was cleaned", lines.join("\n\n"),
      r.failed.length > 0 || r.done.length === 0);
    if (r.done.length) {
      act.refresh();
      act.measure();
      act.loadSpace(true);
    }
  },

  /**
   * The window came forward: what's old is read again (the status always,
   * by the app; the rest when it's been a while), as the Mac app does when a
   * window comes forward.
   */
  catchUp() {
    act.loadAgent();
    act.loadDetail();
    act.loadTidies();
    if (foldersShown()) act.openMap();
    if (Date.now() / 1000 - store.measuredAt > SUGGEST_MAX_AGE) {
      act.loadSpace();
      act.measure();
    }
  },

  /** Never suggested again (`mdif keep add`). */
  async keep(path) {
    try {
      await mdifJson(["keep", "add", path], 30);
      store.kept.add(path);
      notice(true, `Kept ${F.lastName(path)}`,
        "MyDiskIsFull won't suggest deleting, cleaning or moving it. Settings lists what you keep.");
      render();
    } catch (e) {
      notice(false, "Couldn't keep it", message(e));
    }
  },

  /** What raises a notification (`mdif alerts set`), then the status, which carries it. */
  async setAlert(key, value) {
    try {
      await mdifJson(["alerts", "set", key, String(value)], 30);
    } catch (e) {
      notice(false, "Couldn't change it", message(e));
    }
    act.refresh();
  },

  async setNotifications(on) {
    try {
      await app("set_notifications", { on });
      store.settings = { ...store.settings, notifications: on };
    } catch (e) {
      notice(false, "Couldn't change it", message(e));
    }
    render();
  },

  /** Get Started: the pages, and from then on the app opens at sign-in (the app does that). */
  async finishWelcome() {
    try {
      await app("finish_welcome");
    } catch (e) {
      notice(false, "Couldn't save that", message(e));
    }
    store.settings = { ...store.settings, welcomed: true };
    store.login = await app("login_item").catch(() => store.login);
    act.show("overview");
  },

  async setLogin(on) {
    try {
      store.login = await app("set_login_item", { on });
    } catch (e) {
      notice(false, "Couldn't change it", message(e));
    }
    render();
  },
};

// -- The disk map --------------------------------------------------------------------

/** Whether Storage shows Folders, the disk map. */
const foldersShown = () => store.page === "storage" && !store.kind && store.storageMode === "folders";

/** A map the app measured or kept (`{ json, measured }`), read. */
function useMap(kept) {
  let data;
  try {
    data = JSON.parse(kept.json);
  } catch {
    throw new Error("The bundled mdif printed something this version of the app can't read.");
  }
  store.map.data = data;
  store.map.measured = kept.measured;
}

/**
 * Measures again, at idle priority, if the map is older than `limit` seconds
 * (or there's none), and only while the window is on screen: hidden in the
 * tray or minimised, nobody is looking, and each time is the whole disk. One
 * that failed isn't tried again by itself for an hour (Measure Now still is).
 */
async function refreshMapIfOld(limit) {
  const m = store.map;
  const now = Date.now() / 1000;
  if (m.running || (m.measured != null && now - m.measured < limit)) return;
  if (m.failed != null && now - m.failed < MAP_REFRESH_EVERY) return;
  if (await app("window_shown").catch(() => false)) act.measureMap(true);
}

/** An automatic measurement nobody was looking at any more by its turn. */
const SKIPPED = new Error("skipped");

// Whole-disk measurements (the map, the categories) one after the other: two
// at once would each take twice as long, and the disk would be busy for both.
let diskWork = Promise.resolve();
function oneAtATime(work) {
  const run = diskWork.then(work, work);
  diskWork = run.then(() => {}, () => {});
  return run;
}

/** How long it's been measuring, said each second without drawing the page again. */
function tickMeasuring() {
  for (const el of document.querySelectorAll("[data-elapsed]")) {
    if (el.dataset.elapsed) el.textContent = measuringText(Number(el.dataset.elapsed));
  }
}

// While Folders stays on screen, every hour (refreshMapIfOld: only while the
// window is).
setInterval(() => {
  if (foldersShown() && document.visibilityState === "visible") refreshMapIfOld(MAP_REFRESH_EVERY);
}, 60_000);

// -- Starting ----------------------------------------------------------------------

function nav() {
  const pages = [
    ["overview", "Overview", "overview"],
    ["storage", "Storage", "storage"],
    ["oldfiles", "Old Files", "hourglass"],
    ["history", "History", "history"],
    ["prevention", "Prevention", "shield"],
    ["settings", "Settings", "settings"],
  ];
  document.getElementById("nav").replaceChildren(...pages.map(([id, label, glyph]) => h("li", {},
    h("button", {
      class: "nav-item", type: "button", dataset: { page: id },
      onclick: () => act.show(id),
    }, icon(glyph, 18), h("span", {}, label)))));
}

function setStatus(text) {
  try {
    store.status = JSON.parse(text);
    store.statusError = null;
  } catch {
    store.statusError = "The watcher's history couldn't be read.";
  }
  render();
}

async function start() {
  nav();
  // `#storage`, `#storage/build`: a page to open on (while working on the page).
  const [want, kind] = location.hash.slice(1).split("/");
  if (PAGES[want]) {
    store.page = want;
    store.kind = kind || null;
  }
  if (store.page === "settings" && store.askAI) act.loadAccount();
  if (store.page === "prevention") act.loadPrevention();
  document.getElementById("cleanup").addEventListener("close", () => {
    if (store.cleanup) store.cleanup.shown = false;
    render();
  });
  document.getElementById("confirm").addEventListener("cancel", (e) => {
    e.currentTarget.returnValue = "";
  });
  on("status", setStatus);
  // The app started the watcher again (it had ended).
  on("agent", () => act.loadAgent());
  on("status-error", (why) => {
    store.statusError = why;
    render();
  });
  on("shown", () => act.catchUp());
  render();
  const [latest, settings, login, v] = await Promise.all([
    app("latest").catch(() => null),
    app("settings").catch(() => null),
    app("login_item").catch(() => null),
    version().catch(() => null),
  ]);
  store.version = v;
  store.settings = settings;
  store.login = login;
  if (latest) setStatus(latest);
  else if (!inApp) setStatus(JSON.stringify(await mdifJson(["status"])));
  act.refresh();
  act.loadAgent();
  act.loadDetail();
  // Started at sign-in with no window: what measures the disk waits until the
  // window is first shown (catchUp, as nothing has been measured yet).
  if (!(await app("window_shown").catch(() => true))) return;
  if (foldersShown()) act.openMap();
  await act.loadSpace();
  act.measure();
  act.loadTidies();
}

start();
