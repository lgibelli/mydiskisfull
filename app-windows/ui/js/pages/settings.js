// Settings, as the Mac app's: watching in the background, opening at
// sign-in and notifications; then what raises a notification, kept in the
// history (`mdif alerts`), where the watcher reads it too; Ask AI, About and
// Updates. On Windows, no
// program's writes are recorded, so `mdif` refuses the write-churn settings
// and they aren't shown.

import { onWindows } from "../core.js";
import { h } from "../dom.js";
import * as F from "../format.js";
import { busy, button, choice, labeled, note, page, row, section, toggle } from "../widgets.js";

const SIZES = [500_000_000, 1_000_000_000, 2_000_000_000, 5_000_000_000, 10_000_000_000];
const FLOORS = [5_000_000_000, 10_000_000_000, 20_000_000_000, 50_000_000_000];

export function settings(store, act) {
  return page("Settings", {}, watching(store, act), alerts(store, act), folders(store, act), about(store),
    updates(store, act));
}

/** Updates, as the Mac app's: once a day from GitHub, and Install and Restart for what's found. */
export function updates(store, act) {
  const u = store.update;
  if (!u) return null;
  const installing = u.busy === "installing";
  // While one thing is under way, no button starts another.
  const working = (text) =>
    h("span", { class: "working secondary" }, h("span", { class: "spinner", "aria-hidden": "true" }), text);
  const checkNow = u.busy ? null : button("Check Now", act.checkUpdates);
  return section("Updates", {},
    toggle({
      title: "Check for updates automatically",
      text: "Once a day, from GitHub. Updates are signed, and nothing else is sent.",
      on: u.on,
      onchange: (on) => act.setUpdates(on),
    }),
    u.found
      ? row({
        glyph: ["download", "var(--accent)"],
        title: `MyDiskIsFull ${u.found.version} is ready`,
        text: installing
          ? "Downloading it. MyDiskIsFull closes while it installs, and opens again after."
          : "MyDiskIsFull closes while it installs, and opens again after.",
        end: u.busy
          ? working(installing ? "Installing…" : "Checking…")
          // Check Now too: an offer can go stale (a release withdrawn).
          : [checkNow, button("Install and Restart", act.installUpdate, { kind: "accent" })],
      })
      : row({
        title: "Check for Updates",
        text: u.checked ? `Last checked ${F.ago(u.checked)}.` : "Not checked yet.",
        end: u.busy ? working(installing ? "Installing…" : "Checking…") : checkNow,
      }));
}

/** Ask AI, and the ChatGPT account it uses (OpenAI's names for the buttons). */
function folders(store, act) {
  return section("What Folders Are", {},
    toggle({
      title: "Ask AI about folders nothing's known about",
      text: "Shows Ask AI… where nothing is known about a folder. Before anything is sent it shows what would be: the folder's name, its size and the Windows version, never what's in it. It goes from this PC to OpenAI, on your ChatGPT plan.",
      on: store.askAI,
      onchange: (on) => act.setAskAI(on),
    }),
    store.askAI ? accountRow(store, act) : null,
    store.askAI && store.gpt.account?.signed_in && store.gpt.account.plan ? usingPlan(act) : null,
    store.askAI && store.gpt.problem ? row({ glyph: ["warning", "var(--orange)"], text: store.gpt.problem }) : null);
}

/** Signed in or not, and the button for each. */
export function accountRow(store, act) {
  const a = store.gpt.account;
  if (!a) return busy("Reading the ChatGPT account…");
  if (a.signed_in) {
    return row({
      title: "Use your ChatGPT plan",
      text: a.plan
        ? `Signed in as ${a.email ?? "your account"}. Questions use your plan; with ChatGPT Plus, its five-hour limit is shared with every app that uses it.`
        : `Signed in as ${a.email ?? "your account"}, but ChatGPT doesn't let MyDiskIsFull use your plan. Continue with ChatGPT and allow it.`,
      end: [a.plan ? null : continueButton(store, act),
        store.gpt.signingIn ? null : button("Sign Out", act.signOut)],
    });
  }
  return row({
    title: "Use your ChatGPT plan",
    text: "Complete eligible AI requests in this app with usage included in your ChatGPT plan or credits balance (Plus or Pro). MyDiskIsFull itself charges nothing.",
    end: continueButton(store, act),
  });
}

/** The sign-in button, as OpenAI names it; while the browser is out, what to do there. */
export function continueButton(store, act) {
  return store.gpt.signingIn
    ? h("span", { class: "working secondary" }, h("span", { class: "spinner", "aria-hidden": "true" }),
      "Finish signing in in your browser…")
    : button("Continue with ChatGPT", act.signIn, { kind: "accent" });
}

/** Beside where questions are asked: whose plan they use, and where that's managed. */
export function usingPlan(act) {
  return h("div", { class: "row note" }, h("div", { class: "row-sub" }, "Using ChatGPT plan · ",
    button("Manage usage", act.openChatGPTUsage, { kind: "link" })));
}

function watching(store, act) {
  const a = store.agent;
  const caption = !a ? "…"
    : a.running ? "A sample every 5 minutes, and at every sign-in. It costs next to nothing."
      : a.at_sign_in === "off" ? "Switched off in Task Manager's Startup apps. Turning it on here switches it on again."
        : "Off: nothing is measured or forecast.";
  return section("Watching", {},
    toggle({
      title: "Watch in the background",
      text: caption,
      on: Boolean(a?.running),
      disabled: !a || store.watcherBusy,
      onchange: (on) => act.setWatching(on),
    }),
    store.login === null || store.login === "unavailable" ? null : toggle({
      title: "Open at sign-in",
      text: store.login === "off"
        ? "Switched off in Task Manager's Startup apps. Turning it on here switches it on again."
        : "So the tray shows what's free and notifications reach you.",
      on: store.login === "yes",
      onchange: (on) => act.setLogin(on),
    }),
    store.settings ? toggle({
      title: "Notify me",
      text: "When the disk is almost full, and as chosen below.",
      on: store.settings.notifications,
      onchange: (on) => act.setNotifications(on),
    }) : null);
}

function alerts(store, act) {
  const a = store.status?.alerts;
  if (!a) return section("Notifications", {}, busy("Reading the settings…"));
  const off = store.settings && !store.settings.notifications;
  return [
    section("Notifications", { footer: "Always notified: below this, programs start to fail." },
      choice({
        title: "Almost full below",
        value: a.low_free ?? 0,
        options: [[0, `Automatic (${F.bytes(a.low_free_auto)})`], ...FLOORS.map((b) => [b, F.bytes(b)])],
        onchange: (v) => act.setAlert("low_free", v === "0" ? "auto" : v),
        disabled: off,
      })),
    section(null, {},
      toggle({
        title: "Disk filling up", on: a.notify_forecast, disabled: off,
        onchange: (on) => act.setAlert("notify_forecast", on),
      }),
      choice({
        title: "When it's likely full within",
        value: a.full_within_days,
        options: [[1, "a day"], [3, "3 days"], [14, "2 weeks"]],
        onchange: (v) => act.setAlert("full_within_days", v),
        disabled: off || !a.notify_forecast,
      })),
    section(null, { footer: "A folder that keeps growing steadily and would fill the disk within a week." },
      toggle({
        title: "A folder growing fast", on: a.notify_growth, disabled: off,
        onchange: (on) => act.setAlert("notify_growth", on),
      }),
      choice({
        title: "Once it has grown",
        value: a.growth_min,
        options: SIZES.map((b) => [b, F.bytes(b)]),
        onchange: (v) => act.setAlert("growth_min", v),
        disabled: off || !a.notify_growth,
      })),
    onWindows ? null : section(null, { footer: "Steadily for an hour, while the disk keeps almost none of it: rewriting the same files." },
      toggle({
        title: "A program wearing the disk", on: a.notify_churn, disabled: off,
        onchange: (on) => act.setAlert("notify_churn", on),
      }),
      choice({
        title: "When it writes more than",
        value: a.churn_per_hour,
        options: SIZES.map((b) => [b, `${F.bytes(b)} an hour`]),
        onchange: (v) => act.setAlert("churn_per_hour", v),
        disabled: off || !a.notify_churn,
      })),
  ];
}

function about(store) {
  const hist = store.agent?.history;
  const cost = store.status?.watcher_cost;
  return section("About", {},
    store.version ? labeled("Version", store.version) : null,
    cost
      ? row({
        title: "The watcher",
        text: costNote(cost),
        end: h("span", { class: "num value" },
          `${(cost.cpu_us_per_hour / 1e6).toFixed(2)} s of CPU, ${F.bytes(cost.written_per_hour)} written an hour`),
      })
      : store.agent?.running ? note("The watcher's cost shows once it has run a while.") : null,
    hist
      ? labeled("History", h("span", { class: "num" }, `${F.bytes(hist.bytes)}, ${hist.rows.toLocaleString()} rows`), hist.path)
      : null,
    note("MyDiskIsFull reads names, sizes and dates, never what's in a file."));
}

/** What the watcher's measured cost means, as the Mac app's About says it. */
function costNote(c) {
  const within = c.cpu_us_per_hour <= 1_000_000 && c.written_per_hour <= 1_000_000;
  let s = `Measured, at one sample every ${F.duration(c.interval_secs)}: ${within ? "within" : "over"} `
    + "its budget of 1 s of CPU and 1 MB of writes an hour.";
  if (c.periods === 0 && c.catch_up_cpu_us > 0) {
    s += " Still counting the first folder update after it started, which catches up on what changed "
      + "meanwhile and costs more than the next ones.";
  }
  return s;
}
