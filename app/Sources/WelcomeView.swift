// The first launch: start the watcher, give it Full Disk Access (in the App
// Store edition: choose the disk), allow notifications. Each step says what
// it's for and shows when it's done; Get Started opens the main window.

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
    /// Opening at login is asked, not assumed (App Review 2.4.5).
    @State private var openAtLogin = true

    init(monitor: Monitor) {
        self.monitor = monitor
        watcher = monitor.watcher
    }

    var body: some View {
        VStack(spacing: 0) {
            hero
            VStack(alignment: .leading, spacing: 12) {
                StepRow(number: 1, done: watcher.isOn, title: "Start the watcher",
                        text: watcherText) {
                    if watcher.status == .requiresApproval {
                        Button("Open Login Items…") { watcher.openLoginItems() }
                    } else {
                        Button(watcher.hasCommandLineAgent ? "Move It Into the App" : "Start") {
                            Task { await watcher.start(); await monitor.refresh() }
                        }
                        .buttonStyle(.borderedProminent)
                        .disabled(watcher.busy || watcher.isOn)
                    }
                }

                #if APP_STORE
                StepRow(number: 2, done: monitor.access.granted, title: "Let it see your disk",
                        text: "Choose \(DiskAccess.startupDisk) in the dialog, so it can tell which folders grow and map the disk. Until then it watches free space only. It reads names, sizes and dates, never what's in a file, and nothing leaves your Mac.") {
                    Button("Choose Disk…") { monitor.chooseDisk() }
                        .buttonStyle(.borderedProminent)
                }
                #else
                StepRow(number: 2, done: appAccess == true && watcherAccess != false,
                        title: "Give it Full Disk Access",
                        text: accessText) {
                    Button("Open Privacy Settings…") { Privacy.openSettings() }
                }
                #endif

                StepRow(number: 3, done: notificationsAsked && !notificationsDenied, title: "Allow notifications",
                        text: notificationsDenied
                            ? "They're turned off for \(Edition.name) in System Settings, so macOS won't ask again. Turn on Allow Notifications there."
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
                        #if APP_STORE
                        Toggle("Open at login, so it keeps watching", isOn: $openAtLogin)
                            .font(.callout)
                        #else
                        Text("It opens at login, so the forecast stays in the menu bar.")
                            .font(.caption).foregroundStyle(.secondary)
                        #endif
                    }
                    Spacer()
                    Button("Get Started") { finish() }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.large)
                        .keyboardShortcut(.defaultAction)
                }
                .padding(.top, 8)
            }
            .padding(24)
        }
        .frame(width: 620)
        .background(Theme.canvas)
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

    private var hero: some View {
        VStack(spacing: 10) {
            if let icon = NSApp.applicationIconImage {
                Image(nsImage: icon).resizable().frame(width: 96, height: 96)
                    .shadow(color: .black.opacity(0.35), radius: 14, y: 6)
            }
            Text("Welcome to \(Edition.name)")
                .font(.system(size: 28, weight: .bold))
            Text("Know when your disk will be full and what fills it, and keep it from filling again.")
                .font(.title3)
                .opacity(0.85)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.horizontal, 40)
        }
        .foregroundStyle(.white)
        .padding(.top, 44)
        .padding(.bottom, 28)
        .frame(maxWidth: .infinity)
        .background {
            ZStack {
                LinearGradient(colors: [Color(red: 0.07, green: 0.10, blue: 0.24), Color(red: 0.05, green: 0.24, blue: 0.36)],
                               startPoint: .topLeading, endPoint: .bottomTrailing)
                RadialGradient(colors: [Theme.brand[1].opacity(0.45), .clear], center: .topTrailing,
                               startRadius: 0, endRadius: 360)
                RadialGradient(colors: [Theme.brand[0].opacity(0.4), .clear], center: .bottomLeading,
                               startRadius: 0, endRadius: 320)
            }
            .ignoresSafeArea()
        }
    }

    /// The app's own grant shows at once; the watcher's is its own check,
    /// which it repeats at every sample.
    private var watcherAccess: Bool? {
        watcher.isOn ? monitor.status?.access?.fullDiskAccess : nil
    }

    private var watcherText: String {
        Edition.appStore
            ? "It samples free space every 5 minutes while \(Edition.name) is open, and which folders grew every 30. Its budget is a second of CPU an hour, and About shows what it really uses."
            : "A small background process that samples free space and disk writes every 5 minutes, and which folders grew every 30. Its budget is a second of CPU an hour, and About shows what it really uses."
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
        if openAtLogin, SMAppService.mainApp.status != .enabled {
            try? SMAppService.mainApp.register()
        }
        dismiss()
        AppDelegate.show(.overview)
    }
}

private struct StepRow<Action: View>: View {
    let number: Int
    let done: Bool
    let title: String
    let text: String
    @ViewBuilder let action: Action

    var body: some View {
        HStack(alignment: .top, spacing: 14) {
            ZStack {
                Circle().fill(done ? AnyShapeStyle(Color.green.gradient) : AnyShapeStyle(Color.primary.opacity(0.08)))
                if done {
                    Image(systemName: "checkmark").font(.system(size: 13, weight: .bold)).foregroundStyle(.white)
                } else {
                    Text("\(number)").font(.system(size: 14, weight: .bold, design: .rounded)).foregroundStyle(.secondary)
                }
            }
            .frame(width: 30, height: 30)
            .animation(.spring(duration: 0.4), value: done)
            VStack(alignment: .leading, spacing: 4) {
                Text(title).font(.headline)
                Text(text).font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if !done { action.padding(.top, 4) }
            }
            Spacer(minLength: 0)
        }
        .card(padding: 16)
    }
}
