// Settings: the watcher, opening at login, notifications, updates and Full
// Disk Access, as a grouped form like System Settings. Small enough not to
// scroll.

import ServiceManagement
import SwiftUI

struct SettingsView: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var watcher: WatcherService
    @State private var watching = false
    @State private var atLogin = false
    @State private var notify = true
    @State private var checkUpdates = true
    @State private var loginError: String?

    init(monitor: Monitor) {
        self.monitor = monitor
        watcher = monitor.watcher
    }

    var body: some View {
        Form {
            Section("Monitoring") {
                Toggle(isOn: $watching) {
                    Text("Watch in the background")
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
                Toggle(isOn: $atLogin) {
                    Text("Open at login")
                    Text("So the menu bar shows the forecast and notifications reach you.")
                }
                .onChange(of: atLogin) { _, on in
                    // Also fires when load() or a failed change sets it back.
                    guard on != (SMAppService.mainApp.status == .enabled) else { return }
                    setLogin(on)
                }
                if let e = watcher.problem ?? loginError {
                    Label(e, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                }
            }

            Section("Notifications") {
                Toggle(isOn: $notify) {
                    Text("Notify me")
                    Text(Edition.appStore
                         ? "When something runs away, or the disk will be full within two weeks. Each once."
                         : "When something runs away, the disk will be full within two weeks, or a tweak was undone. Each once.")
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
                LabeledContent("Version \(AboutView.version)") {
                    Button("Check Now") { monitor.updates.checkForUpdates() }
                }
            }
            #endif

            Section {
                AccessLine(who: "The watcher", granted: monitor.status?.access?.fullDiskAccess,
                           unknown: "Checks when it starts and every 30 minutes")
                AccessLine(who: "This app (the disk map and fixes)",
                           granted: Privacy.appHasFullDiskAccess, unknown: "Can't tell")
                LabeledContent {
                    Button("Open Privacy Settings…") { Privacy.openSettings() }
                } label: {
                    Text("Without it, macOS keeps MyDiskIsFull out of other apps' folders and Mail, so what grows there goes unseen. It only ever reads sizes, never what's in a file.")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } header: {
                Text("Full Disk Access")
            }
        }
        .formStyle(.grouped)
        .toggleStyle(.switch)
        .scrollDisabled(true)
        .frame(width: 560, height: Edition.appStore ? 520 : 640)
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
