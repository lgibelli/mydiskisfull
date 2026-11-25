// The app's side of the window: `mdif` runs through the app (src-tauri's
// `mdif` command), as the Mac app's Core.run does; the window never measures
// anything itself.
//
// Opened in a browser instead (while working on the page, on any computer),
// there is no app: what `mdif` would print comes from the Mac app's test
// files in app/Tests (or the folder `?fixtures=` names), and what the app
// would do is only logged.

const tauri = window.__TAURI__;
const params = new URLSearchParams(location.search);

/** Whether this is the app, not a page in a browser. */
export const inApp = Boolean(tauri);

/**
 * Windows, where `mdif` records no program's writes (its counters can't
 * tell a disk from a pipe), so what's about them isn't shown. In a browser,
 * `?windows` says it.
 */
export const onWindows = inApp ? /Windows/.test(navigator.userAgent) : params.has("windows");

/**
 * `mdif <args> --json`, read.
 * @param {string[]} args
 * @param {number} [timeoutSecs]
 * @returns {Promise<any>}
 */
export async function mdifJson(args, timeoutSecs) {
  const text = inApp
    ? await tauri.core.invoke("mdif", { args: [...args, "--json"], timeoutSecs })
    : await fixture(args);
  try {
    return JSON.parse(text);
  } catch {
    throw new Error("mdif printed something this version of the app can't read.");
  }
}

/**
 * One of the app's own commands (main.rs).
 * @param {string} command
 * @param {object} [payload]
 */
export async function app(command, payload) {
  if (inApp) return tauri.core.invoke(command, payload);
  console.info("[dev] the app would", command, payload ?? "");
  return devAnswers[command]?.(payload) ?? null;
}

/** The app's version (tauri.conf.json's). */
export async function version() {
  return inApp ? tauri.app.getVersion() : "dev";
}

/**
 * What the app tells the window: "status" (what `mdif status` printed, as
 * text), "status-error", "shown" (the window came forward) and "update"
 * (what a check found, or null).
 * @param {string} event
 * @param {(payload: any) => void} handler
 */
export function on(event, handler) {
  if (inApp) tauri.event.listen(event, (e) => handler(e.payload));
}

// -- In a browser -----------------------------------------------------------

const fixtures = params.get("fixtures") ?? "../../app/Tests";

/** Test files by command; `null`: answered here. */
const files = {
  status: "status-incidents.json",
  space: "space.json",
  suggest: "suggest-actions.json",
  alerts: "alerts.json",
  forecast: "forecast.json",
};

/**
 * Ask AI, in a browser: `?signed-in` is signed in with ChatGPT already;
 * signing in takes a moment; the answer is the Mac app's test file's.
 */
let signedIn = params.has("signed-in");
async function knowledgeFixture([sub, ...rest]) {
  const account = () => ({
    signed_in: signedIn, email: signedIn ? "you@example.com" : null, plan: signedIn,
    usage: "https://chatgpt.com/settings/usage",
  });
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  switch (sub) {
    case "account": return JSON.stringify(account());
    case "sign-in": await wait(1500); signedIn = true; return JSON.stringify(account());
    case "sign-out": signedIn = false; return JSON.stringify({ revoked: true });
    case "ask": {
      const bytes = rest.includes("--bytes") ? Number(rest[rest.indexOf("--bytes") + 1]) : null;
      if (rest.includes("--dry-run")) {
        return JSON.stringify({ name: "~\\AppData\\Local\\Example", bytes, windows: "11 24H2", account: account().email });
      }
      await wait(1200);
      const r = await fetch(`${fixtures}/knowledge-answer.json`);
      return r.ok ? r.text() : JSON.stringify({ name: "~\\AppData\\Local\\Example", known: false, by: "GPT", asked: 0 });
    }
    default: return "{}";
  }
}

/** Prevention, in a browser: Windows' tweaks as `mdif tweaks` lists them; applying and undoing one is kept until the page reloads. */
const tweaks = [
  {
    id: "recycle-bin-auto-empty",
    title: "Empty the Recycle Bin automatically after 30 days",
    status: "not applied",
    why: "Files in the Recycle Bin take space until it is emptied.",
    cost: "Files in the Recycle Bin for over 30 days can't be restored. It turns Storage Sense on, which then runs at least once a week and also deletes temporary files apps no longer use, unless that is turned off in Settings → System → Storage.",
    size: 3_400_000_000, applied: null,
    changes: [
      "Storage Sense, 1 if on (HKCU\\…\\StoragePolicy\\01): not set → 1",
      "Storage Sense emptying the Recycle Bin, 1 if on (HKCU\\…\\StoragePolicy\\08): not set → 1",
      "Days in the Recycle Bin before Storage Sense deletes a file (HKCU\\…\\StoragePolicy\\256): not set → 30",
      "Days between Storage Sense's runs, 0 if only when space is low (HKCU\\…\\StoragePolicy\\2048): 0 → 7",
    ],
    next: "Done; it takes effect once you sign out of Windows and back in.",
  },
  {
    id: "gradle-caches",
    title: "Let Gradle clear caches unused for 14 days",
    status: "not applied",
    why: "Gradle keeps downloaded dependencies and Gradle distributions for 30 days after their last use, and every Gradle version has a cache of its own.",
    cost: "A project not built for two weeks downloads its dependencies again.",
    size: 8_800_000_000, applied: params.has("undone") ? 1_790_000_000 : null,
    changes: ["~\\.gradle\\init.d\\mydiskisfull-caches.gradle: absent → a file of 13 lines"],
    next: "Done; it takes effect from now on.",
  },
  {
    id: "claude-transcripts",
    title: "Keep Claude Code transcripts for 14 days",
    status: "applied",
    why: "Claude Code keeps every session's transcript for 30 days; with agents working all day they add up to gigabytes.",
    cost: "Sessions older than 14 days can't be resumed.",
    size: 1_200_000_000, applied: null,
    changes: [],
    next: null,
  },
];
async function tweaksFixture([sub, id, ...rest]) {
  const strip = ({ changes, next, ...t }) => ({ ...t, restart_pending: t.applied ? false : null, sizes: null, trend: null });
  if (!sub) {
    await new Promise((r) => setTimeout(r, 400));
    return JSON.stringify(tweaks.map(strip));
  }
  const t = tweaks.find((x) => x.id === id);
  if (!t) throw new Error(`no tweak "${id}"`);
  if (sub === "apply") {
    const dry = rest.includes("--dry-run");
    const changes = t.status === "applied" ? [] : t.changes;
    if (!dry && changes.length) Object.assign(t, { status: "applied", applied: Math.floor(Date.now() / 1000) });
    return JSON.stringify({ id, title: t.title, why: t.why, cost: t.cost, dry_run: dry, changes, profiles: [], next: dry ? null : t.next });
  }
  if (sub === "revert") {
    const restored = t.changes.map((c) => c.replace(/: (.*) → (.*)$/, ": $2 → $1"));
    Object.assign(t, { status: "not applied", applied: null });
    return JSON.stringify({ id, title: t.title, restored, kept: [], restart: false, sign_out: id === "recycle-bin-auto-empty" });
  }
  return "{}";
}

/**
 * `mdif tidy` in a browser: Windows' three, as on a PC with a big disk,
 * which doesn't record when a file was read (`?last-access`: one that
 * does); `?tidy-on`: the Rust one on, with a last run.
 */
const tidies = (() => {
  const recorded = params.has("last-access");
  const why = (what) =>
    `Windows on this PC doesn't record when a file was last opened, so ${what} from an old one.`;
  const on = params.has("tidy-on");
  return [
    {
      id: "builds", title: "Old build folders",
      detail: "A project's build output (Rust's target, .NET's bin and obj, Flutter's and Gradle's build, .next, dist) that nothing has built into or run from for 14 days. Its next build makes it again, from scratch.",
      on: false, available: recorded,
      unavailable_why: recorded ? null : why("a build still in use (a tool run from it) can't be told"), last: null,
    },
    {
      id: "node-modules", title: "Old node_modules",
      detail: "A project's node_modules that nothing has installed into or run from for 14 days. npm, yarn, pnpm or bun puts it back from the lockfile, downloading what isn't in its cache.",
      on: false, available: recorded,
      unavailable_why: recorded ? null : why("the packages of a project that still runs can't be told"), last: null,
    },
    {
      id: "incremental", title: "Old Rust build caches",
      detail: "A Rust build's incremental caches no build has used for 30 days: the next build of that project's own crates is slower, nothing else.",
      on, available: true, unavailable_why: null,
      last: on ? { at: Date.now() / 1000 - 2 * 86400, freed: 2_140_000_000, done: 3, failed: 0, note: "Deleted the old incremental caches of 3 Rust projects" } : null,
    },
  ];
})();
let tidiedOnce = false;
async function tidyFixture([sub, id]) {
  const t = tidies.find((x) => x.id === id);
  if (sub && !t) throw new Error(`no tidy-up called ${id}: builds, node-modules, incremental`);
  if (sub === "on" || sub === "off") t.on = sub === "on";
  if (sub === "run") {
    await new Promise((done) => setTimeout(done, 1200));
    if (!t.available) {
      return JSON.stringify({ dry_run: false, freed: 0, done: [], failed: [{ id, path: "", why: t.unavailable_why }] });
    }
    // The first run finds two; the next, nothing.
    const done = id === "incremental" && !tidiedOnce ? [
      { id, path: "C:\\Users\\me\\source\\repos\\engine\\target\\debug\\incremental", what: "deleted", bytes: 1_310_000_000 },
      { id, path: "C:\\Users\\me\\devel\\cli\\target\\release\\incremental", what: "deleted", bytes: 420_000_000 },
    ] : [];
    if (done.length) tidiedOnce = true;
    const freed = done.reduce((n, d) => n + d.bytes, 0);
    t.last = {
      at: Date.now() / 1000, freed, done: done.length, failed: 0,
      note: done.length ? "Deleted the old incremental caches of 2 Rust projects" : "Nothing to tidy",
    };
    return JSON.stringify({ dry_run: false, freed, done, failed: [] });
  }
  return JSON.stringify({ tidies });
}

async function fixture(args) {
  const [cmd] = args;
  if (cmd === "agent") {
    // `?stopped`: no watcher.
    const stopped = params.has("stopped");
    return JSON.stringify({ at_sign_in: stopped ? "no" : "yes", running: !stopped, history: null });
  }
  if (cmd === "knowledge") return knowledgeFixture(args.slice(1));
  if (cmd === "tweaks") return tweaksFixture(args.slice(1));
  if (cmd === "tidy") return tidyFixture(args.slice(1));
  if (cmd === "clean") {
    // A file (it has an extension) goes to the Recycle Bin; a folder is deleted.
    const how = (path) => (/\$Recycle\.Bin$/i.test(path) ? "erased" : /\.\w{2,4}$/.test(path) ? "trashed" : "deleted");
    return JSON.stringify({
      done: args.slice(1).map((path) => ({ path, how: how(path) })),
      failed: [],
      freed: 0,
      in_trash: 0,
    });
  }
  const file = files[cmd];
  if (!file) return "{}";
  const r = await fetch(`${fixtures}/${file}`);
  if (!r.ok) {
    if (cmd === "forecast") {
      return JSON.stringify({ forecast: null, writes_24h: [], growth_24h: [], series: [] });
    }
    throw new Error(`no ${file} in ${fixtures}`);
  }
  return r.text();
}

/** The fixtures' `map.json`, as the app gives a map; `?kept-map`: kept from last time. */
async function mapFixture(delay = 0) {
  await new Promise((done) => setTimeout(done, delay));
  const r = await fetch(`${fixtures}/map.json`);
  if (!r.ok) throw new Error(`no map.json in ${fixtures}`);
  return { json: await r.text(), measured: Math.floor(Date.now() / 1000) - (params.has("kept-map") ? 7200 : 0) };
}

const devAnswers = {
  kept_map: () => (params.has("kept-map") ? mapFixture() : null),
  // `?hidden`: started at sign-in, with the window not shown.
  window_shown: () => !params.has("hidden") && document.visibilityState === "visible",
  measure_map: () => mapFixture(2500),
  // `?welcome`: the first window.
  settings: () => ({ notifications: true, welcomed: !params.has("welcome") }),
  finish_welcome: () => null,
  login_item: () => "no",
  set_login_item: ({ on }) => (on ? "yes" : "no"),
  latest: () => null,
  open_chatgpt_usage: () => null,
  // `?update`: a newer version was found.
  updates: () => ({
    on: true,
    checked: Date.now() / 1000 - 3 * 3600,
    found: params.has("update") ? { version: "9.9.9" } : null,
  }),
  set_updates: () => null,
  check_updates: async () => {
    await new Promise((r) => setTimeout(r, 1200));
    return params.has("update") ? { version: "9.9.9" } : null;
  },
  install_update: async () => {
    await new Promise((r) => setTimeout(r, 1500));
    throw new Error("Only in the app.");
  },
};
