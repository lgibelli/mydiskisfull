// Settings: the watcher, opening at login, notifications, and Full Disk
// Access. Laid out with GroupBox, not Form: settings this small shouldn't
// scroll.

import ServiceManagement
import SwiftUI

struct SettingsView: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var watcher: WatcherService
    @State private var watching = false
    @State private var atLogin = false
    @State private var notify = true
    @State private var loginError: String?

    init(monitor: Monitor) {
        self.monitor = monitor
        watcher = monitor.watcher
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            GroupBox {
                VStack(spacing: 10) {
                    SwitchRow(title: "Watch in the background", caption: watcher.caption, isOn: $watching)
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
                            .frame(maxWidth: .infinity, alignment: .trailing)
                    }
                    Divider()
                    SwitchRow(title: "Open at login",
                              caption: "So the menu bar shows the forecast and notifications reach you.",
                              isOn: $atLogin)
                        .onChange(of: atLogin) { _, on in
                            // Also fires when load() or a failed change sets it back.
                            guard on != (SMAppService.mainApp.status == .enabled) else { return }
                            setLogin(on)
                        }
                    Divider()
                    SwitchRow(title: "Notifications",
                              caption: "A runaway filler, the disk filling within two weeks, a tweak something undid. Each once.",
                              isOn: $notify)
                        .onChange(of: notify) { _, on in
                            monitor.settings.notificationsEnabled = on
                            if on { Notifier.requestAuthorization() }
                        }
                }
                .padding(.vertical, 4)
            }

            GroupBox {
                VStack(alignment: .leading, spacing: 8) {
                    Text("Full Disk Access").font(.headline)
                    AccessLine(who: "The watcher", granted: monitor.status?.access?.fullDiskAccess,
                               unknown: "It checks when it starts and every 30 minutes.")
                    AccessLine(who: "This app (the disk map and suggestions)",
                               granted: Privacy.appHasFullDiskAccess, unknown: "Can't tell.")
                    Text("Without it, macOS keeps MyDiskIsFull out of other apps' folders and Mail, so what grows there goes unseen. It only ever reads sizes, never what's in a file.")
                        .font(.caption).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Button("Open Privacy Settings…") { Privacy.openSettings() }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.vertical, 4)
            }

            if let e = watcher.problem ?? loginError {
                Label(e, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange)
            }
            if let p = Notifier.authorizationProblem, notify {
                HStack {
                    Label(p, systemImage: "bell.slash").font(.caption).foregroundStyle(.orange)
                    Button("Notification Settings…") { Notifier.openSettings() }.controlSize(.small)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(20)
        .frame(width: 540)
        .fixedSize(horizontal: false, vertical: true)
        .onAppear {
            watcher.refresh()
            watching = watcher.isOn
            atLogin = SMAppService.mainApp.status == .enabled
            notify = monitor.settings.notificationsEnabled
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
        HStack(spacing: 6) {
            Image(systemName: granted == true ? "checkmark.circle.fill"
                  : granted == false ? "xmark.circle.fill" : "questionmark.circle")
                .foregroundStyle(granted == true ? .green : granted == false ? .orange : .secondary)
            Text(who)
            Text(granted == true ? "has it" : granted == false ? "doesn't have it" : unknown)
                .foregroundStyle(.secondary)
        }
        .font(.callout)
    }
}

/// A switch with a one-line explanation under it, text at the leading edge
/// and the switch at the trailing one, so rows line up.
struct SwitchRow: View {
    let title: String
    let caption: String
    @Binding var isOn: Bool

    var body: some View {
        HStack(alignment: .center, spacing: 16) {
            VStack(alignment: .leading, spacing: 1) {
                Text(title)
                Text(caption)
                    .font(.caption).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Toggle("", isOn: $isOn).labelsHidden().toggleStyle(.switch)
        }
    }
}
