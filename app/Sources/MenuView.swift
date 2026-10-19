// The menu. Only genuine menu items: Text (shown disabled, for status lines),
// Button, Toggle (a checkmark), Divider and Menu for submenus. Sentences are
// split over several lines, since a menu item never wraps.

import SwiftUI

struct MenuView: View {
    @ObservedObject var monitor: Monitor
    @Environment(\.openWindow) private var openWindow
    @Environment(\.openSettings) private var openSettings

    var body: some View {
        if let s = monitor.status {
            Text("\(Format.bytes(s.avail)) free of \(Format.bytes(s.total))")
            Text(trend(s))
        } else if let e = monitor.statusError {
            Text("Can't read the watcher's history")
            Lines(e)
        } else {
            Text("Reading the history…")
        }

        if let p = monitor.watcherProblem {
            Divider()
            Text(p)
            if monitor.watcher.status == .requiresApproval {
                Button("Open Login Items…") { monitor.watcher.openLoginItems() }
            } else {
                Button("Start Watching") { Task { await monitor.watcher.start(); await monitor.refresh() } }
            }
        }
        if monitor.lacksFullDiskAccess {
            Divider()
            Text("The watcher can't see into other apps' folders")
            Button("Give It Full Disk Access…") { Privacy.openSettings() }
        }

        if let s = monitor.status, !s.incidents.isEmpty {
            Divider()
            Text("Filling the disk")
            ForEach(s.incidents) { e in
                Menu(incidentTitle(e)) {
                    Lines(e.sentence)
                    Divider()
                    FixItems(fix: e.fix, monitor: monitor)
                }
            }
        }
        if !monitor.undone.isEmpty {
            Divider()
            ForEach(monitor.undone) { t in
                Menu("Undone since: \(t.title)") {
                    Lines("Something changed its setting back, probably an update.")
                    Button("Apply Again…") { monitor.apply(t.id) }
                }
            }
        }

        Divider()
        SuggestionsMenu(monitor: monitor)
        TweaksMenu(monitor: monitor)

        Divider()
        Button("Show Overview…") { show("overview") }
        Button("Disk Map…") { show("map") }

        Divider()
        Button("Settings…") { showSettings() }
            .keyboardShortcut(",", modifiers: .command)
        Button("About MyDiskIsFull") { show("about") }
        Divider()
        Button("Quit MyDiskIsFull") { NSApp.terminate(nil) }
            .keyboardShortcut("q", modifiers: .command)
    }

    private func trend(_ s: Status) -> String {
        guard let f = s.forecast else { return "No samples yet" }
        guard let r = f.rate else {
            return "Collecting history: \(s.history.samples) samples so far"
        }
        if r.perDay <= 0 { return "Not filling (\(Format.window(r.window)) trend)" }
        if let full = f.fullIn {
            return "Full in about \(Format.days(full.days)), around \(Format.date(at: f.at, inDays: full.days))"
        }
        return "Filling at \(Format.bytes(r.perDay)) a day"
    }

    private func incidentTitle(_ e: Episode) -> String {
        let i = e.incident
        switch i.kind {
        case .growth:
            return "\(Format.path(i.subject)) grows \(Format.bytes(i.perDay)) a day"
        case .churn:
            return "\(i.subject) rewrites \(Format.bytes(i.perDay)) a day"
        }
    }

    private func show(_ id: String) {
        openWindow(id: id)
        NSApp.activate(ignoringOtherApps: true)
    }

    /// An agent app isn't activated by opening a window, so Settings would
    /// open behind whatever is in front. Activate, then bring it forward.
    private func showSettings() {
        openSettings()
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async {
            NSApp.windows.first { $0.identifier?.rawValue == "com_apple_SwiftUI_Settings_window" }?
                .makeKeyAndOrderFront(nil)
        }
    }
}

/// A sentence as menu lines.
struct Lines: View {
    let text: String
    init(_ text: String) { self.text = text }

    var body: some View {
        ForEach(Array(Format.lines(text).enumerated()), id: \.offset) { _, line in
            Text(line)
        }
    }
}

/// What to do about a cause: a tweak to apply, a command to copy (never
/// run), or what to change by hand.
struct FixItems: View {
    let fix: Fix
    @ObservedObject var monitor: Monitor

    var body: some View {
        switch fix {
        case .tweak(let id):
            let t = monitor.tweak(id)
            if t?.isApplied == true {
                Text("Fix: \(t?.title ?? id) (applied)")
            } else {
                Button("Fix: \(t?.title ?? id)…") { monitor.apply(id) }
            }
        case .run(let command, let note):
            Lines(note)
            Button("Copy the Command") { monitor.copy(command) }
            Lines(command)
        case .steps(let text):
            Lines(text)
        case .leave(let text):
            Lines("Leave it: " + text)
        }
    }
}

struct SuggestionsMenu: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Menu("Fix It for Good") {
            if let list = monitor.suggestions {
                if list.isEmpty {
                    Text("Nothing to suggest")
                    Lines("No runaway fillers, and nothing a tweak would stop takes much space.")
                }
                ForEach(Array(list.enumerated()), id: \.offset) { _, s in
                    Menu(title(s)) {
                        Lines(s.cause)
                        Divider()
                        FixItems(fix: s.fix, monitor: monitor)
                    }
                }
            } else {
                Text("Measuring…")
            }
        }
    }

    /// The cause's first sentence: "Gradle's caches take 8.8 GB."
    private func title(_ s: Suggestion) -> String {
        let first = s.cause.components(separatedBy: ". ").first ?? s.cause
        let t = first.hasSuffix(".") ? String(first.dropLast()) : first
        return t.count > 70 ? String(t.prefix(68)) + "…" : t
    }
}

struct TweaksMenu: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        Menu("Tweaks") {
            if let list = monitor.tweaks {
                ForEach(list.filter { !$0.isAbsent }) { t in
                    let byUs = t.applied != nil
                    Toggle(isOn: Binding(
                        get: { t.isApplied || t.status == "waiting for approval" },
                        set: { on in on ? monitor.apply(t.id) : monitor.revert(t.id) }
                    )) {
                        Text(label(t))
                    }
                    // Applied by hand: there is no record of what it replaced.
                    .disabled(t.isApplied && !byUs)
                }
                Divider()
                Text("Each stops something filling the disk again,")
                Text("and can be undone here.")
            } else {
                Text("Measuring…")
            }
        }
    }

    private func label(_ t: TweakInfo) -> String {
        var s = t.title
        if t.status == "waiting for approval" { s += " (waiting for approval)" }
        else if t.status == "partly applied" { s += " (partly applied)" }
        else if t.isApplied && t.applied == nil { s += " (set outside MyDiskIsFull)" }
        else if !t.isApplied, let size = t.size, size >= 100_000_000 { s += " — \(Format.bytes(size))" }
        return s
    }
}
