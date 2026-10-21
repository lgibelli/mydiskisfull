// The menu bar menu: a plain menu, as most Mac apps have. A few lines of
// text (how full the disk is, where it's heading, the last day), what needs
// attention, then the way into the main window, where everything else is,
// updates, About, Settings and Quit.

import AppKit
import SwiftUI

struct StatusMenu: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        if let s = monitor.status {
            let used = Int((Double(s.total - s.avail) / Double(max(s.total, 1)) * 100).rounded())
            Text("\(Format.bytes(s.avail)) free of \(Format.bytes(s.total)) · \(used)% used")
            Text(Headline.title(s))
            if let day = s.forecast?.paces?.day {
                Text("Last 24 hours: \(Format.change(day).lowercasedFirst)")
            }
        } else if let e = monitor.statusError {
            Text(e)
        } else {
            Text("Reading the history…")
        }
        let rows = attention
        if !rows.isEmpty {
            Divider()
            Section("Needs attention") {
                ForEach(rows.prefix(4), id: \.self) { t in
                    Button(t) { AppDelegate.show(.overview) }
                }
                if rows.count > 4 {
                    Text("and \(rows.count - 4) more")
                }
            }
        }
        Divider()
        Button("Open \(Edition.name)") { AppDelegate.show(Navigator.shared.pane) }
            .keyboardShortcut("o")
        Divider()
        #if !APP_STORE
        UpdateItem(updates: monitor.updates)
        #endif
        Button("About \(Edition.name)") { AppDelegate.open("about") }
        Button("Settings…") { AppDelegate.showSettings() }
            .keyboardShortcut(",")
        Divider()
        Button("Quit \(Edition.name)") { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }

    /// What needs attention, a line each.
    private var attention: [String] {
        var rows: [String] = []
        if let p = monitor.watcherProblem { rows.append(p) }
        if monitor.lacksFullDiskAccess {
            rows.append(Edition.appStore ? "Needs to see your disk" : "Needs Full Disk Access")
        }
        for e in monitor.status?.incidents ?? [] {
            let i = e.incident
            rows.append(i.kind == .growth
                ? "\(URL(fileURLWithPath: i.subject).lastPathComponent) grows \(Format.bytes(i.perDay)) a day"
                : "\(i.subject) rewrites \(Format.bytes(i.perDay)) a day")
        }
        for t in monitor.undone {
            rows.append("Undone: \(t.title)")
        }
        return rows
    }
}

private extension String {
    /// "Filled 3 GB" as "filled 3 GB", after a colon.
    var lowercasedFirst: String { prefix(1).lowercased() + dropFirst() }
}

#if !APP_STORE
struct UpdateItem: View {
    @ObservedObject var updates: Updater

    var body: some View {
        Button("Check for Updates…") { updates.checkForUpdates() }
            .disabled(!updates.canCheckForUpdates)
    }
}
#endif
