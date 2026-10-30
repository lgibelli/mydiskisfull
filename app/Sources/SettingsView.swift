// Settings: a sheet of the main window, like WhatsApp's: the
// first page lists the sections, a few to a group, and can be searched; a
// row opens its page and ‹ comes back. The sections: watching (the
// watcher, opening at login, the Dock), notifications (what's worth one),
// what you keep, disk access (Full Disk Access; in the App Store edition,
// the disk it may see), and About & updates on the app's own row.

import AppKit
import ServiceManagement
import SwiftUI

/// A page of Settings.
enum SettingsPage: String, CaseIterable, Identifiable {
    case watching, notifications, kept, access, about

    var id: String { rawValue }

    var title: String {
        switch self {
        case .watching: "Watching"
        case .notifications: "Notifications"
        case .kept: "What You Keep"
        case .access: Edition.appStore ? "Disk Access" : "Full Disk Access"
        case .about: "About & Updates"
        }
    }

    var symbol: String {
        switch self {
        case .watching: "eye.fill"
        case .notifications: "bell.badge.fill"
        case .kept: "pin.fill"
        case .access: "lock.fill"
        case .about: "info.circle.fill"
        }
    }

    var color: Color {
        switch self {
        case .watching: .blue
        case .notifications: .red
        case .kept: .orange
        case .access: .indigo
        case .about: .gray
        }
    }

    /// What's on the page, for the search.
    var words: [String] {
        switch self {
        case .watching: ["watch", "watcher", "background", "login", "startup", "dock", "menu bar"]
        case .notifications: ["notify", "notification", "alert", "almost full", "filling", "growing",
                              "folder", "wearing", "writes", "threshold"]
        case .kept: ["keep", "kept", "pin", "never suggest", "exclude"]
        case .access: ["full disk access", "privacy", "permission", "disk", "access"]
        case .about: ["about", "version", "update", "check"]
        }
    }

    /// The groups on the first page; About is the app's own row above them.
    static let groups: [[SettingsPage]] = [[.watching, .notifications], [.kept, .access]]
}

struct SettingsView: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var watcher: WatcherService
    @Environment(\.dismiss) private var dismiss
    @State private var page: SettingsPage?
    @State private var search = ""
    @State private var watching = false
    @State private var atLogin = false
    @State private var inDock = Dock.always
    @State private var notify = true
    @State private var checkUpdates = true
    @AppStorage("knowledgeUpdates") private var knowledgeUpdates = true
    @AppStorage("askAI") private var askAI = true
    @State private var loginError: String?

    init(monitor: Monitor, page: SettingsPage? = nil) {
        self.monitor = monitor
        watcher = monitor.watcher
        _page = State(initialValue: page)
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            Group {
                if let page {
                    content(page)
                        .transition(.move(edge: .trailing).combined(with: .opacity))
                } else {
                    home
                        .transition(.move(edge: .leading).combined(with: .opacity))
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(width: 600, height: 640)
        .background(Theme.canvas)
        .animation(.snappy(duration: 0.25), value: page)
        .task { await monitor.loadKept() }
        .onAppear {
            watcher.refresh()
            watching = watcher.isOn
            atLogin = SMAppService.mainApp.status == .enabled
            notify = monitor.settings.notificationsEnabled
            #if !APP_STORE
            checkUpdates = monitor.updates.automaticallyChecksForUpdates
            #endif
        }
    }

    // MARK: - Header and the first page

    private var header: some View {
        ZStack {
            Text(page?.title ?? "Settings").font(.headline)
            HStack {
                if page != nil {
                    Button {
                        page = nil
                    } label: {
                        Label("Back", systemImage: "chevron.left")
                    }
                    .keyboardShortcut(.cancelAction)
                    .help("Back")
                } else {
                    Button {
                        dismiss()
                    } label: {
                        Label("Close", systemImage: "xmark")
                    }
                    .keyboardShortcut(.cancelAction)
                    .help("Close")
                }
                Spacer()
            }
            .labelStyle(.iconOnly)
            .buttonStyle(.borderless)
            .font(.system(size: 14, weight: .semibold))
        }
        .padding(.horizontal, 16)
        .frame(height: 46)
    }

    private var home: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Search settings", text: $search)
                        .textFieldStyle(.plain)
                }
                .padding(.horizontal, 10)
                .padding(.vertical, 7)
                .background(Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
                let q = search.trimmingCharacters(in: .whitespaces).lowercased()
                if q.isEmpty {
                    appRow
                    ForEach(Array(SettingsPage.groups.enumerated()), id: \.offset) { _, g in
                        group(g)
                    }
                } else {
                    let found = SettingsPage.allCases.filter { p in
                        p.title.lowercased().contains(q) || p.words.contains { $0.contains(q) }
                    }
                    if found.isEmpty {
                        Text("No matches.")
                            .foregroundStyle(.secondary)
                            .frame(maxWidth: .infinity)
                            .padding(.top, 30)
                    } else {
                        group(found)
                    }
                }
            }
            .padding(20)
        }
    }

    /// The app itself: its icon, its version, and About & Updates.
    private var appRow: some View {
        Button {
            page = .about
        } label: {
            HStack(spacing: 14) {
                Image(nsImage: NSApp.applicationIconImage)
                    .resizable().frame(width: 46, height: 46)
                VStack(alignment: .leading, spacing: 2) {
                    Text(Edition.name).font(.title3.weight(.semibold))
                    Text("Version \(AboutView.version) · " + (Edition.appStore ? "About" : "About & updates"))
                        .font(.callout).foregroundStyle(.secondary)
                }
                Spacer()
                Image(systemName: "chevron.right").font(.callout.weight(.semibold)).foregroundStyle(.tertiary)
            }
            .padding(12)
            .contentShape(Rectangle())
        }
        .buttonStyle(SettingsRowStyle())
        .background(Theme.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.hairline))
    }

    private func group(_ pages: [SettingsPage]) -> some View {
        VStack(spacing: 0) {
            ForEach(Array(pages.enumerated()), id: \.element) { i, p in
                if i > 0 { Divider().padding(.leading, 54) }
                Button {
                    page = p
                } label: {
                    HStack(spacing: 12) {
                        IconTile(symbol: p.symbol, color: p.color, size: 28)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(p.title)
                            Text(summary(p)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                        }
                        Spacer()
                        if attention(p) {
                            Circle().fill(Color.orange).frame(width: 8, height: 8)
                        }
                        Image(systemName: "chevron.right").font(.caption.weight(.semibold)).foregroundStyle(.tertiary)
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 9)
                    .contentShape(Rectangle())
                }
                .buttonStyle(SettingsRowStyle())
            }
        }
        .background(Theme.card, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.hairline))
    }

    /// Under each row: how it's set now.
    private func summary(_ p: SettingsPage) -> String {
        switch p {
        case .watching:
            var t = [watcher.isOn ? (Edition.appStore ? "Watching while open" : "Watching in the background") : "Not watching"]
            if atLogin { t.append("opens at login") }
            if !inDock { t.append("out of the Dock") }
            return t.joined(separator: ", ")
        case .notifications: return notify ? alertsSummary : "Off"
        case .kept:
            switch monitor.kept?.count {
            case nil, 0: return "Nothing yet"
            case 1: return "1 item never suggested"
            case let n?: return "\(n) items never suggested"
            }
        case .access:
            #if APP_STORE
            return monitor.access.granted ? monitor.access.name : "Free space only"
            #else
            return monitor.status?.access?.fullDiskAccess == false || Privacy.appHasFullDiskAccess == false
                ? "Not granted: some folders go unseen" : "Granted"
            #endif
        case .about: return AboutView.version
        }
    }

    /// A dot on the row when something there needs doing.
    private func attention(_ p: SettingsPage) -> Bool {
        switch p {
        case .watching: watcher.problem != nil || loginError != nil || !watcher.isOn
        case .notifications: notify && Notifier.authorizationProblem != nil
        case .access: monitor.lacksFullDiskAccess
        default: false
        }
    }

    // MARK: - The pages

    @ViewBuilder
    private func content(_ p: SettingsPage) -> some View {
        switch p {
        case .watching: watchingPage
        case .notifications: notificationsPage
        case .kept: KeptList(monitor: monitor)
        case .access: accessPage
        case .about: aboutPage
        }
    }

    private var watchingPage: some View {
        Form {
            Section {
                Toggle(isOn: $watching) {
                    Text(Edition.appStore ? "Watch while it's open" : "Watch in the background")
                    Text(watcher.caption)
                }
                .onChange(of: watching) { _, on in
                    guard on != watcher.isOn else { return }
                    Task {
                        if on { await watcher.start() } else { await watcher.stop() }
                        watching = watcher.isOn
                        await monitor.refresh()
                    }
                }
                .disabled(watcher.busy)
                if watcher.status == .requiresApproval {
                    Button("Open Login Items…") { watcher.openLoginItems() }
                }
            }
            Section {
                Toggle(isOn: $atLogin) {
                    Text("Open at login")
                    Text("So the menu bar shows the forecast and notifications reach you.")
                }
                .onChange(of: atLogin) { _, on in
                    // Also fires when load() or a failed change sets it back.
                    guard on != (SMAppService.mainApp.status == .enabled) else { return }
                    setLogin(on)
                }
                Toggle(isOn: $inDock) {
                    Text("Show in the Dock")
                    Text("Off: in the menu bar, and in the Dock only while a window is open.")
                }
                .onChange(of: inDock) { _, on in Dock.always = on }
            }
            if let e = watcher.problem ?? loginError {
                Label(e, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            }
        }
        .formStyle(.grouped)
        .toggleStyle(.switch)
    }

    private var notificationsPage: some View {
        Form {
            Section {
                Toggle(isOn: $notify) {
                    Text("Notify me")
                    Text(Edition.appStore
                         ? "When something runs away, or the disk will be full soon. Each once."
                         : "When something runs away, the disk will be full soon, or a tweak was undone. Each once.")
                }
                .onChange(of: notify) { _, on in
                    monitor.settings.notificationsEnabled = on
                    if on { Notifier.requestAuthorization() }
                }
                if let p = Notifier.authorizationProblem, notify {
                    LabeledContent {
                        Button("Notification Settings…") { Notifier.openSettings() }
                    } label: {
                        Label(p, systemImage: "bell.slash.fill").foregroundStyle(.orange)
                    }
                }
            }
            AlertChoices(monitor: monitor)
                .disabled(!notify)
        }
        .formStyle(.grouped)
        .toggleStyle(.switch)
    }

    private var accessPage: some View {
        Form {
            #if APP_STORE
            Section {
                LabeledContent {
                    Button(monitor.access.granted ? "Change…" : "Choose Disk…") { monitor.chooseDisk() }
                } label: {
                    Label(monitor.access.granted ? monitor.access.name : "Free space only",
                          systemImage: monitor.access.granted ? "checkmark.circle.fill" : "lock.fill")
                        .foregroundStyle(monitor.access.granted ? .green : .orange)
                }
            } footer: {
                Text("What it may see beyond free space: choose \(DiskAccess.startupDisk) to watch which folders grow and map the whole disk. It reads names, sizes and dates, never what's in a file.")
            }
            #else
            Section {
                AccessLine(who: "The watcher", granted: monitor.status?.access?.fullDiskAccess,
                           unknown: "Checks when it starts and every 30 minutes")
                AccessLine(who: "This app (the disk map and fixes)",
                           granted: Privacy.appHasFullDiskAccess, unknown: "Can't tell")
                LabeledContent("Privacy & Security") {
                    Button("Open Privacy Settings…") { Privacy.openSettings() }
                }
            } footer: {
                Text("Without it, macOS keeps MyDiskIsFull out of other apps' folders and Mail, so what grows there goes unseen. It only ever reads sizes, never what's in a file.")
            }
            #endif
        }
        .formStyle(.grouped)
    }

    private var aboutPage: some View {
        Form {
            Section {
                LabeledContent("Version", value: AboutView.version)
                LabeledContent("More about \(Edition.name)") {
                    Button("About \(Edition.name)…") { AppDelegate.open("about") }
                }
            }
            #if !APP_STORE
            Section("Updates") {
                Toggle(isOn: $checkUpdates) {
                    Text("Check for updates automatically")
                    Text("Once a day, from GitHub. Updates are signed, and nothing else is sent.")
                }
                .onChange(of: checkUpdates) { _, on in
                    // Sparkle owns and keeps this one.
                    monitor.updates.automaticallyChecksForUpdates = on
                }
                LabeledContent("Check now") {
                    Button("Check for Updates…") { monitor.updates.checkForUpdates() }
                }
            }
            Section("What Folders Are") {
                Toggle(isOn: $knowledgeUpdates) {
                    Text("Download what's known about folders")
                    Text("Once a day, from salamacchine.it: what folders are, why they grow and what helps. The list is signed like the updates, and nothing about your Mac is sent.")
                }
                Toggle(isOn: $askAI) {
                    Text("Ask AI about folders nothing's known about")
                    Text("Shows Ask AI… where the list says nothing about a folder. Before anything is sent it shows what would be: the folder's name, its size and the macOS version, never what's in it. Claude, Anthropic's AI, answers through salamacchine.it.")
                }
            }
            #endif
        }
        .formStyle(.grouped)
        .toggleStyle(.switch)
    }

    private var alertsSummary: String {
        guard let a = monitor.status?.alerts else { return "As MyDiskIsFull sees fit" }
        var on: [String] = []
        if a.notifyForecast { on.append("filling up") }
        if a.notifyGrowth { on.append("folders growing") }
        if !Edition.appStore, a.notifyChurn { on.append("apps wearing the disk") }
        return on.isEmpty ? "Only when the disk is almost full" : "Almost full, " + on.joined(separator: ", ")
    }

    private func setLogin(_ on: Bool) {
        do {
            if on { try SMAppService.mainApp.register() } else { try SMAppService.mainApp.unregister() }
            loginError = nil
        } catch {
            loginError = "Couldn't change the login item: \(error.localizedDescription)"
        }
        atLogin = SMAppService.mainApp.status == .enabled
    }
}

/// A row of the first page: it highlights under the pointer and when
/// pressed, like a list's.
private struct SettingsRowStyle: ButtonStyle {
    @State private var hovering = false

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .background(Color.primary.opacity(configuration.isPressed ? 0.1 : hovering ? 0.05 : 0))
            .onHover { hovering = $0 }
    }
}

struct AccessLine: View {
    let who: String
    let granted: Bool?
    let unknown: String

    var body: some View {
        LabeledContent(who) {
            Label(granted == true ? "Has it" : granted == false ? "Doesn't have it" : unknown,
                  systemImage: granted == true ? "checkmark.circle.fill"
                      : granted == false ? "xmark.circle.fill" : "questionmark.circle")
                .foregroundStyle(granted == true ? .green : granted == false ? .orange : .secondary)
        }
    }
}

/// The keep list: each entry with a button to stop keeping it, and Add… for
/// files and folders.
struct KeptList: View {
    @ObservedObject var monitor: Monitor

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("MyDiskIsFull never suggests deleting, cleaning or moving these, or anything in them, however big or old they get. If one grows fast you're still told, since it explains the forecast.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            List {
                if let kept = monitor.kept, !kept.isEmpty {
                    ForEach(kept) { k in
                        HStack(spacing: 8) {
                            Image(systemName: "pin.fill").foregroundStyle(.secondary)
                            VStack(alignment: .leading, spacing: 1) {
                                Text(k.entry).lineLimit(1).truncationMode(.middle)
                                if let n = k.present, n != 1 {
                                    Text(n == 0 ? "Nothing there now" : "\(n) items")
                                        .font(.caption).foregroundStyle(.secondary)
                                }
                            }
                            Spacer()
                            Button {
                                monitor.unkeep(k)
                            } label: {
                                Image(systemName: "minus.circle.fill")
                            }
                            .buttonStyle(.borderless)
                            .foregroundStyle(.secondary)
                            .help("Stop keeping it")
                        }
                    }
                } else {
                    Text("Nothing yet. A suggestion's Keep It button adds what it's about here.")
                        .foregroundStyle(.secondary)
                }
            }
            .listStyle(.bordered)
            .alternatingRowBackgrounds()
            #if !APP_STORE
            Text("Patterns go in with `mdif keep add`: ~/Movies/Clip* keeps every file whose name starts with Clip.")
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            #endif
            HStack {
                Button("Add…") { choose() }
                Spacer()
            }
        }
        .padding(20)
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.title = "Keep Files or Folders"
        panel.message = "MyDiskIsFull won't suggest deleting, cleaning or moving what you choose."
        panel.prompt = "Keep"
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.canCreateDirectories = false
        guard panel.runModal() == .OK else { return }
        let paths = panel.urls.map(\.path)
        if !paths.isEmpty { monitor.keep(paths) }
    }
}

/// What raises a notification: each kind on or off, and how much it takes.
/// Kept in the history (`mdif alerts`), where the watcher reads it too.
private struct AlertChoices: View {
    @ObservedObject var monitor: Monitor

    private static let sizes: [Int64] = [500_000_000, 1_000_000_000, 2_000_000_000, 5_000_000_000, 10_000_000_000]
    private static let floors: [Int64] = [5_000_000_000, 10_000_000_000, 20_000_000_000, 50_000_000_000]

    var body: some View {
        if let a = monitor.status?.alerts {
            Section {
                Picker("Almost full below", selection: Binding(
                    get: { a.lowFree ?? 0 },
                    set: { monitor.setAlert("low_free", $0 == 0 ? "auto" : "\($0)") }
                )) {
                    Text("Automatic (\(Format.bytes(a.lowFreeAuto)))").tag(Int64(0))
                    ForEach(Self.floors, id: \.self) { Text(Format.bytes($0)).tag($0) }
                }
            } footer: {
                Text("Always notified: below this, apps start to fail.")
            }
            Section {
                Toggle("Disk filling up", isOn: Binding(
                    get: { a.notifyForecast },
                    set: { monitor.setAlert("notify_forecast", $0 ? "true" : "false") }
                ))
                Picker("When it's likely full within", selection: Binding(
                    get: { a.fullWithinDays },
                    set: { monitor.setAlert("full_within_days", "\($0)") }
                )) {
                    Text("a day").tag(1)
                    Text("3 days").tag(3)
                    Text("2 weeks").tag(14)
                }
                .disabled(!a.notifyForecast)
            }
            Section {
                Toggle("A folder growing fast", isOn: Binding(
                    get: { a.notifyGrowth },
                    set: { monitor.setAlert("notify_growth", $0 ? "true" : "false") }
                ))
                Picker("Once it has grown", selection: Binding(
                    get: { a.growthMin },
                    set: { monitor.setAlert("growth_min", "\($0)") }
                )) {
                    ForEach(Self.sizes, id: \.self) { Text(Format.bytes($0)).tag($0) }
                }
                .disabled(!a.notifyGrowth)
            } footer: {
                Text("A folder that keeps growing steadily and would fill the disk within a week.")
            }
            #if !APP_STORE
            Section {
                Toggle("An app wearing the disk", isOn: Binding(
                    get: { a.notifyChurn },
                    set: { monitor.setAlert("notify_churn", $0 ? "true" : "false") }
                ))
                Picker("When it writes more than", selection: Binding(
                    get: { a.churnPerHour },
                    set: { monitor.setAlert("churn_per_hour", "\($0)") }
                )) {
                    ForEach(Self.sizes, id: \.self) { Text("\(Format.bytes($0)) an hour").tag($0) }
                }
                .disabled(!a.notifyChurn)
            } footer: {
                Text("Steadily for an hour, while the disk keeps almost none of it: rewriting the same files.")
            }
            #endif
        }
    }
}
