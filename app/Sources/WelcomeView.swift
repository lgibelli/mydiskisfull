// The first launch: start the watcher, give it Full Disk Access, allow
// notifications. Each step says what it's for and shows when it's done.

import ServiceManagement
import SwiftUI
import UserNotifications

struct WelcomeView: View {
    @ObservedObject var monitor: Monitor
    @ObservedObject private var watcher: WatcherService
    @Environment(\.dismiss) private var dismiss
    @State private var appAccess = Privacy.appHasFullDiskAccess
    @State private var notificationsAsked = false
    /// Turned off in System Settings: asking again shows nothing.
    @State private var notificationsDenied = false

    init(monitor: Monitor) {
        self.monitor = monitor
        watcher = monitor.watcher
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 14) {
                if let icon = NSApp.applicationIconImage {
                    Image(nsImage: icon).resizable().frame(width: 56, height: 56)
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("MyDiskIsFull").font(.title2.bold())
                    Text("Watches your disk, tells you when it will be full and what fills it, and helps keep it from filling again.")
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            StepRow(number: 1, done: watcher.isOn, title: "Start the watcher",
                    text: "A small background process that samples free space and disk writes every 5 minutes, and which folders grew every 30. About shows what it costs, measured.") {
                if watcher.status == .requiresApproval {
                    Button("Open Login Items…") { watcher.openLoginItems() }
                } else {
                    Button(watcher.hasCommandLineAgent ? "Move It Into the App" : "Start") {
                        Task { await watcher.start(); await monitor.refresh() }
                    }
                    .disabled(watcher.busy || watcher.isOn)
                }
            }

            StepRow(number: 2, done: appAccess == true && watcherAccess != false,
                    title: "Give it Full Disk Access",
                    text: accessText) {
                Button("Open Privacy Settings…") { Privacy.openSettings() }
            }

            StepRow(number: 3, done: notificationsAsked && !notificationsDenied, title: "Allow notifications",
                    text: notificationsDenied
                        ? "They're turned off for MyDiskIsFull in System Settings, so macOS won't ask again. Turn on Allow Notifications there."
                        : "For a runaway filler, the disk filling within two weeks, or a fix something undid. Each is announced once.") {
                if notificationsDenied {
                    Button("Open Notification Settings…") { Notifier.openSettings() }
                } else {
                    Button("Allow…") {
                        Notifier.requestAuthorization()
                        notificationsAsked = true
                    }
                    .disabled(notificationsAsked)
                }
            }

            HStack {
                if SMAppService.mainApp.status != .enabled {
                    Text("It opens at login, so the forecast stays in the menu bar.")
                        .font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button("Done") { finish() }.keyboardShortcut(.defaultAction)
            }
        }
        .padding(24)
        .frame(width: 560)
        .onAppear {
            watcher.refresh()
            Task { await checkNotifications() }
        }
        // Full Disk Access is granted in System Settings; notice it on return.
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            appAccess = Privacy.appHasFullDiskAccess
            watcher.refresh()
            Task {
                await monitor.refresh()
                await checkNotifications()
            }
        }
    }

    /// The app's own grant shows at once; the watcher's is its own check,
    /// which it repeats at every sample.
    private var watcherAccess: Bool? {
        watcher.isOn ? monitor.status?.access?.fullDiskAccess : nil
    }

    private var accessText: String {
        var t = "Without it, macOS keeps it out of other apps' folders, where caches and backups grow, and out of Desktop, Documents and Downloads. It reads sizes only, never what's in a file. Turn MyDiskIsFull on in the list (add it with + if it isn't there), then come back: that covers the watcher too."
        if appAccess == true && watcherAccess == false {
            t += " The app has it; the watcher checks again within 5 minutes."
        }
        return t
    }

    private func checkNotifications() async {
        let s = await UNUserNotificationCenter.current().notificationSettings()
        notificationsDenied = s.authorizationStatus == .denied
        if s.authorizationStatus == .authorized || s.authorizationStatus == .provisional {
            notificationsAsked = true
        }
    }

    private func finish() {
        monitor.settings.welcomed = true
        if !notificationsAsked && !notificationsDenied { Notifier.requestAuthorization() }
        if SMAppService.mainApp.status != .enabled {
            try? SMAppService.mainApp.register()
        }
        dismiss()
    }
}

private struct StepRow<Action: View>: View {
    let number: Int
    let done: Bool
    let title: String
    let text: String
    @ViewBuilder let action: Action

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: done ? "checkmark.circle.fill" : "\(number).circle")
                .font(.title2)
                .foregroundStyle(done ? .green : .secondary)
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline)
                Text(text).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if !done { action.padding(.top, 2) }
            }
            Spacer(minLength: 0)
        }
    }
}
