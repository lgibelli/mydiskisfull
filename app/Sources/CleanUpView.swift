// Clean Up: everything the app can clean itself, in one list, each with a
// checkbox. What comes back by itself and loses nothing used lately is
// ticked to start with (caches, build output, DerivedData, simulators nobody
// used for a month, Android images no emulator uses, what Docker hasn't used
// in a week); what needs a look (the Trash, a runtime or a simulator still in
// use, an old Android emulator) isn't, and says why beside it. Each
// simulator is a row of its own (an item's `parts`). The sheet is the question: what's ticked goes, one after the other,
// each through `mdif clean` (which checks it again first), and what each did
// shows on its row. No alert before, none after. Not in the App Store
// edition, which may only read the disk.

#if !APP_STORE
import AppKit
import SwiftUI

@MainActor
final class CleanUp: ObservableObject {
    static let shared = CleanUp()

    struct Row: Identifiable {
        let id: String
        let symbol: String
        let tint: Color
        let title: String
        let detail: String?
        let bytes: Int64?
        let action: CleanAction

        var safe: Bool { action.safe ?? false }

        /// Inside what `other` cleans: counted there.
        func within(_ other: Row) -> Bool {
            action.paths.allSatisfy { p in other.action.paths.contains { p.hasPrefix($0 + "/") } }
        }
    }

    enum State: Equatable {
        case running
        /// What it did, and what the disk has more free.
        case done(String, Int64)
        case failed(String)
    }

    @Published private(set) var rows: [Row] = []
    @Published var chosen: Set<String> = []
    @Published private(set) var states: [String: State] = [:]
    @Published private(set) var running = false
    @Published private(set) var stopping = false
    /// Since the last run started.
    @Published private(set) var freed: Int64 = 0
    /// On screen: what it did is said on its rows, not in the window.
    var shown = false

    var finished: Bool { !running && !states.isEmpty }

    /// The list as the disk is now, unless a run is under way or its
    /// results are on screen.
    func load(_ m: Monitor, minBytes: Int64, minDays: Int, force: Bool = false) {
        guard !running, force || states.isEmpty else { return }
        states = [:]
        freed = 0
        rows = Self.list(m, minBytes: minBytes, minDays: minDays)
        chosen = Set(rows.filter(\.safe).map(\.id))
    }

    func run(_ m: Monitor) {
        let picked = rows.filter { chosen.contains($0.id) }
        guard !running, !picked.isEmpty else { return }
        running = true
        stopping = false
        freed = 0
        states = [:]
        Task {
            var done: [CleanAction] = []
            var failed = 0
            for row in picked {
                if stopping { break }
                states[row.id] = .running
                if let app = row.action.quit.flatMap(Self.openApp) {
                    states[row.id] = .failed("\(app) is open: quit it first.")
                    failed += 1
                    continue
                }
                m.setCleaning(row.action.paths, true)
                let result = await CleanFlow.perform(row.action)
                m.setCleaning(row.action.paths, false)
                switch result {
                case .failure(let e):
                    states[row.id] = .failed(e.localizedDescription)
                    failed += 1
                case .success(let r) where r.done.isEmpty:
                    states[row.id] = .failed(r.failed.first?.why ?? "Nothing was done.")
                    failed += 1
                case .success(let r):
                    done.append(row.action)
                    freed += r.freed
                    // What it did, without the total: that's at the foot.
                    var said = CleanFlow.said(r).filter { !$0.hasPrefix("The disk has") }
                    said += r.failed.map { "Not done: \($0.why)" }
                    states[row.id] = .done(said.joined(separator: " "), r.freed)
                }
            }
            running = false
            stopping = false
            // Closed meanwhile: said in the window instead.
            if !shown {
                let n = done.count
                Notices.shared.post(ok: failed == 0, n == 0 ? "Nothing was cleaned" : "Cleaned up",
                                    (n == 0 ? "" : "The disk has \(Format.bytes(freed)) more free.")
                                    + (failed == 0 ? "" : " \(failed) couldn't be done: open Clean Up to see why."),
                                    stays: failed > 0)
            }
            if !done.isEmpty { await m.cleaned(done) }
        }
    }

    /// Stops after the one under way.
    func stop() { stopping = true }

    /// The open app `owner` names: a bundle id, or a name.
    static func running(_ owner: String) -> NSRunningApplication? {
        let apps = NSWorkspace.shared.runningApplications
        return owner.contains(".")
            ? apps.first { $0.bundleIdentifier == owner }
            : apps.first { $0.localizedName == owner }
    }

    /// Its name, when it's open.
    static func openApp(_ owner: String) -> String? {
        running(owner).map { $0.localizedName ?? owner }
    }

    /// What Overview recommends that the app does itself, one row each (the
    /// groups opened up), and what an item offers besides its row's button
    /// (Docker's old leftovers, where the row reviews Docker's list). Not a
    /// helper's restart (it frees nothing), nor Docker's list. What's
    /// ticked first, biggest first.
    static func list(_ m: Monitor, minBytes: Int64, minDays: Int) -> [Row] {
        var out: [Row] = []
        var seen = Set<String>()
        func add(_ symbol: String, _ tint: Color, _ title: String, _ detail: String?,
                 _ bytes: Int64?, _ a: CleanAction) {
            guard batched(a), seen.isDisjoint(with: a.paths) else { return }
            seen.formUnion(a.paths)
            // By what it cleans: a suggestion shows on an item it's about,
            // with that item's id.
            out.append(Row(id: a.kind + ":" + a.paths.joined(separator: "\n"), symbol: symbol, tint: tint, title: title, detail: a.note ?? detail,
                           bytes: a.frees ?? bytes, action: a))
        }
        // An item's own clean-up, by the item's name: a suggestion about a
        // folder may offer the clean-up of one in it (a growing worktree,
        // its node_modules).
        let owned = Set((m.space?.kinds ?? []).flatMap(\.items).compactMap { $0.action?.paths })
        for r in Recommendations.all(m, minBytes: minBytes, minDays: minDays, grouped: false) {
            if case .clean(let a) = r.act, !(owned.contains(a.paths) && r.path.map { !a.paths.contains($0) } == true) {
                add(r.symbol, r.tint, r.title, r.detail, r.bytes, a)
            }
        }
        for k in m.space?.kinds ?? [] {
            for i in k.items where !i.has("kept") {
                // One row each: each simulator, instead of all at once.
                if let parts = i.parts, !parts.isEmpty {
                    for p in parts {
                        add(SpaceText.symbol(k.id), SpaceText.color(k.id), p.name, nil, p.action.frees, p.action)
                    }
                    continue
                }
                guard let a = i.action else { continue }
                let bytes = a.frees ?? (i.frees > 0 ? i.frees : nil)
                guard a.kind == "docker-old" || (bytes ?? 0) >= 100_000_000 else { continue }
                add(SpaceText.symbol(k.id), SpaceText.color(k.id), i.name, i.note,
                    a.kind == "docker-old" ? nil : bytes, a)
            }
        }
        // What a ticked row cleans anyway isn't a row of its own; what
        // frees little isn't worth one (Docker's says what it freed after).
        out = out.filter { $0.action.kind == "docker-old" || ($0.bytes ?? .max) >= 100_000_000 }
        out = out.filter { r in !out.contains { $0.id != r.id && $0.safe && r.within($0) } }
        return out.enumerated().sorted { a, b in
            if a.element.safe != b.element.safe { return a.element.safe }
            let (x, y) = (a.element.bytes ?? 0, b.element.bytes ?? 0)
            return x != y ? x > y : a.offset < b.offset
        }.map(\.element)
    }

    private static func batched(_ a: CleanAction) -> Bool {
        !a.paths.isEmpty && a.done == nil && a.kind != "restart" && a.kind != "docker"
    }
}

struct CleanUpSheet: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var model = CleanUp.shared
    @Environment(\.dismiss) private var dismiss
    @AppStorage("cruft.minBytes") private var minBytes: Double = 100_000_000
    @AppStorage("cruft.minDays") private var minDays: Int = 365
    /// Redrawn when an app quits: a row waiting for it may go now.
    @State private var appsChanged = 0

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Clean Up").font(.title2.weight(.semibold))
                Text("What \(Edition.name) can clean up itself. Ticked: what comes back by itself, and nothing you used lately. Each is checked again right before.")
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            content
            footer
        }
        .padding(20)
        .frame(width: 680, height: 600)
        .onAppear {
            model.shown = true
            model.load(monitor, minBytes: Int64(minBytes), minDays: minDays)
        }
        .onDisappear { model.shown = false }
        // The list follows the disk while nothing has run yet.
        .onChange(of: monitor.space?.measured) { _, _ in
            model.load(monitor, minBytes: Int64(minBytes), minDays: minDays)
        }
        .onReceive(NSWorkspace.shared.notificationCenter.publisher(for: NSWorkspace.didTerminateApplicationNotification)) { _ in
            appsChanged += 1
        }
    }

    @ViewBuilder private var content: some View {
        if model.rows.isEmpty {
            VStack(spacing: 10) {
                if monitor.space == nil {
                    ProgressView().controlSize(.small)
                    Text("Measuring the whole disk: about a minute the first time.")
                } else {
                    Image(systemName: "checkmark.circle").font(.largeTitle).foregroundStyle(.green)
                    Text("Nothing to clean up.")
                }
            }
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else {
            let safe = model.rows.filter(\.safe)
            let look = model.rows.filter { !$0.safe }
            List {
                if !safe.isEmpty {
                    Section("Comes back by itself") {
                        ForEach(safe) { CleanUpRow(row: $0, model: model, appsChanged: appsChanged) }
                    }
                }
                if !look.isEmpty {
                    Section("Look first") {
                        ForEach(look) { CleanUpRow(row: $0, model: model, appsChanged: appsChanged) }
                    }
                }
            }
            .listStyle(.inset(alternatesRowBackgrounds: true))
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
            .overlay { RoundedRectangle(cornerRadius: 8, style: .continuous).strokeBorder(Theme.hairline) }
        }
    }

    private var footer: some View {
        let picked = model.rows.filter { model.chosen.contains($0.id) }
        // One inside another that's ticked is counted there.
        let size = picked.filter { r in !picked.contains { $0.id != r.id && r.within($0) } }
            .reduce(Int64(0)) { $0 + ($1.bytes ?? 0) }
        let unsized = picked.contains { $0.bytes == nil }
        return HStack(spacing: 12) {
            if model.running {
                ProgressView().controlSize(.small)
                let at = picked.firstIndex { model.states[$0.id] == .running }.map { $0 + 1 } ?? picked.count
                Text(model.stopping ? "Stopping after this one…" : "Cleaning \(at) of \(picked.count)…")
                    .foregroundStyle(.secondary)
                Spacer()
                Button("Stop") { model.stop() }
                    .disabled(model.stopping)
            } else if model.finished {
                Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                Text("The disk has \(Format.bytes(model.freed)) more free.")
                    .monospacedDigit()
                Spacer()
                Button("Look Again") {
                    model.load(monitor, minBytes: Int64(minBytes), minDays: minDays, force: true)
                }
                Button("Done") { dismiss() }
                    .keyboardShortcut(.defaultAction)
            } else {
                if !model.rows.isEmpty {
                    let all = picked.count == model.rows.count
                    Button(all ? "Select None" : "Select All") {
                        model.chosen = all ? [] : Set(model.rows.map(\.id))
                    }
                }
                if !picked.isEmpty {
                    Text("Frees about \(Format.bytes(size))\(unsized ? " and Docker's" : "")")
                        .foregroundStyle(.secondary)
                        .monospacedDigit()
                }
                Spacer()
                Button("Cancel", role: .cancel) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button(picked.count == 1 ? "Clean Up 1" : "Clean Up \(picked.count)") {
                    model.run(monitor)
                }
                .keyboardShortcut(.defaultAction)
                .disabled(picked.isEmpty)
            }
        }
    }
}

private struct CleanUpRow: View {
    let row: CleanUp.Row
    @ObservedObject var model: CleanUp
    /// Only to redraw when an app quits.
    let appsChanged: Int

    var body: some View {
        let state = model.states[row.id]
        let open = row.action.quit.flatMap(CleanUp.openApp)
        HStack(alignment: .top, spacing: 10) {
            mark(state)
                .frame(width: 18, height: 18)
                .padding(.top, 5)
            IconTile(symbol: row.symbol, color: row.tint, size: 26)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(row.title).lineLimit(1).truncationMode(.middle)
                    Spacer(minLength: 8)
                    if let b = row.bytes {
                        Text(Format.bytes(b)).foregroundStyle(.secondary).monospacedDigit()
                    }
                }
                switch state {
                case .done(let said, _):
                    Text(said.isEmpty ? "Done." : said).font(.subheadline).foregroundStyle(.green)
                        .fixedSize(horizontal: false, vertical: true)
                case .failed(let why):
                    Text(why).font(.subheadline).foregroundStyle(.orange)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                default:
                    if let d = row.detail {
                        Text(Format.inline(d)).font(.subheadline).foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                    // What can't be undone, said before it's ticked.
                    if !row.safe, let warn = row.action.confirm {
                        Text(Format.inline(warn)).font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    if let app = open, state == nil {
                        HStack(spacing: 8) {
                            Label("\(app) is open: quit it first, or this is skipped.",
                                  systemImage: "exclamationmark.triangle.fill")
                                .font(.caption)
                                .foregroundStyle(.orange)
                            if let a = row.action.quit.flatMap(CleanUp.running) {
                                Button("Quit \(app)") { a.terminate() }
                                    .controlSize(.small)
                            }
                        }
                    }
                }
            }
        }
        .padding(.vertical, 3)
        .help(row.action.confirm ?? row.title)
    }

    @ViewBuilder private func mark(_ state: CleanUp.State?) -> some View {
        switch state {
        case .running:
            ProgressView().controlSize(.small)
        case .done:
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        case .failed:
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
        case nil:
            Toggle("", isOn: Binding(
                get: { model.chosen.contains(row.id) },
                set: { if $0 { model.chosen.insert(row.id) } else { model.chosen.remove(row.id) } }
            ))
            .toggleStyle(.checkbox)
            .labelsHidden()
            .disabled(model.running || model.finished)
        }
    }
}
#endif
