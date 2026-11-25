// The first window, as the Mac app's (WelcomeView.swift): start the watcher,
// each step saying what it's for and showing when it's done; Get Started
// opens the pages, and from then on the app opens at sign-in. Windows asks no
// leave to read the disk or to notify, so the Mac's other two steps aren't
// here.

import { h, icon } from "../dom.js";
import { button } from "../widgets.js";

export function welcome(store, act) {
  const a = store.agent;
  const watching = Boolean(a?.running);
  return h("div", { class: "welcome-card", role: "dialog", "aria-modal": "true", "aria-labelledby": "welcome-title" },
    h("div", { class: "welcome-hero" },
      h("img", { src: "icon-128.png", alt: "", width: 96, height: 96 }),
      h("h1", { id: "welcome-title" }, "Welcome to MyDiskIsFull"),
      h("p", {}, "Know when your disk will be full and what fills it, and keep it from filling again.")),
    h("div", { class: "welcome-body" },
      step(1, watching, "Start the watcher",
        "A small background process that samples free space every 5 minutes, and which folders grew "
          + "every 30. Its budget is a second of CPU an hour.",
        store.watcherBusy
          ? h("span", { class: "working secondary" }, h("span", { class: "spinner", "aria-hidden": "true" }), "Starting…")
          : button("Start", () => act.setWatching(true), { kind: "accent", focus: "welcome:start" })),
      h("div", { class: "welcome-foot" },
        h("p", { class: "secondary" },
          "It opens at sign-in, so the tray shows what's free. It reads names, sizes and dates, never "
            + "what's in a file, and nothing leaves your PC."),
        button("Get Started", () => act.finishWelcome(), { kind: "accent large", focus: "welcome:done" }))));
}

/** A numbered step: its number, or a tick once it's done; its action until then. */
function step(n, done, title, text, action) {
  return h("div", { class: `card step${done ? " step-done" : ""}` },
    h("span", { class: "step-mark", "aria-hidden": "true" }, done ? icon("check", 14) : String(n)),
    h("div", { class: "step-text" },
      h("div", { class: "step-title" }, title, done ? h("span", { class: "visually-hidden" }, ", done") : null),
      h("div", { class: "secondary" }, text),
      done ? null : h("div", { class: "step-action" }, action)));
}
