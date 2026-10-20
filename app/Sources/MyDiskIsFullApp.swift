// MyDiskIsFull: a main window, and a glance in the menu bar. The watching is
// done by `mdif watch`, a LaunchAgent the app registers; the app reads its
// history, shows the forecast and what fills the disk, and applies and undoes
// the fixes (not in the App Store edition: see Edition.swift).
//
// It lives in the menu bar, so it has no Dock icon until a window opens: it
// joins the Dock and the app switcher while one is open, and leaves when the
// last one closes.

import SwiftUI
import UserNotifications

@main
struct MyDiskIsFullApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var monitor = Monitor()

    var body: some Scene {
        MenuBarExtra {
            StatusPanel(monitor: monitor)
        } label: {
            MenuBarLabel(monitor: monitor)
        }
        .menuBarExtraStyle(.window)

        Window("MyDiskIsFull", id: "main") {
            MainWindow(monitor: monitor)
        }
        .defaultSize(width: 1120, height: 780)
        .defaultPosition(.center)
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands { AppCommands(monitor: monitor) }

        Window("Welcome to MyDiskIsFull", id: "welcome") {
            WelcomeView(monitor: monitor)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        Window("About MyDiskIsFull", id: "about") {
            AboutView(monitor: monitor)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        Settings {
            SettingsView(monitor: monitor)
        }
    }
}

/// The app's menus: About and updates where macOS puts them, the pages in
/// View (⌘1 to ⌘4), and nothing to create.
struct AppCommands: Commands {
    let monitor: Monitor

    var body: some Commands {
        CommandGroup(replacing: .appInfo) {
            Button("About MyDiskIsFull") { AppDelegate.open("about") }
            #if !APP_STORE
            UpdateItem(updates: monitor.updates)
            #endif
        }
        CommandGroup(replacing: .newItem) {}
        SidebarCommands()
        CommandGroup(before: .sidebar) {
            ForEach(Pane.allCases) { p in
                Button(p.title) { AppDelegate.show(p) }
                    .keyboardShortcut(p.key, modifiers: .command)
            }
            Divider()
        }
        CommandGroup(replacing: .help) {
            Button("MyDiskIsFull Help") {
                NSWorkspace.shared.open(URL(string: "https://github.com/lgibelli/mydiskisfull#readme")!)
            }
        }
    }
}

/// The icon changes only when the state does, never on a timer.
struct MenuBarLabel: View {
    @ObservedObject var monitor: Monitor
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        HStack(spacing: 3) {
            Image(systemName: monitor.needsAttention ? Icon.attention : Icon.calm)
            if let text = monitor.menuBarText {
                Text(text)
            }
        }
        .task {
            // The label is the one view that exists from launch, so it hands
            // window opening to the code that has no view of its own.
            AppDelegate.openWindow = { id in openWindow(id: id) }
            AppDelegate.launchStep()
        }
    }
}

enum Icon {
    static let calm = firstAvailable(["internaldrive", "opticaldiscdrive"])
    static let attention = firstAvailable(["externaldrive.badge.exclamationmark",
                                           "exclamationmark.triangle"])

    private static func firstAvailable(_ names: [String]) -> String {
        names.first { NSImage(systemSymbolName: $0, accessibilityDescription: nil) != nil } ?? "circle"
    }
}

/// In the Dock while a window is open (or Sparkle shows an update), not
/// otherwise: a menu bar app that someone is using looks like any app.
@MainActor
enum Dock {
    /// Sparkle's update alert is up.
    static var showingUpdate = false {
        didSet { update() }
    }

    private static var observers: [NSObjectProtocol] = []

    static func start() {
        let names: [Notification.Name] = [NSWindow.didBecomeKeyNotification, NSWindow.willCloseNotification,
                                          NSWindow.didMiniaturizeNotification, NSWindow.didDeminiaturizeNotification]
        observers = names.map {
            NotificationCenter.default.addObserver(forName: $0, object: nil, queue: .main) { _ in
                // A closing window is still visible now: look after it's gone.
                DispatchQueue.main.async { MainActor.assumeIsolated { update() } }
            }
        }
    }

    static func update() {
        let open = NSApp.windows.contains { w in
            (w.isVisible || w.isMiniaturized) && w.styleMask.contains(.titled) && !(w is NSPanel)
        }
        let want: NSApplication.ActivationPolicy = open || showingUpdate ? .regular : .accessory
        guard NSApp.activationPolicy() != want else { return }
        _ = NSApp.setActivationPolicy(want)
        if want == .regular { NSApp.activate(ignoringOtherApps: true) }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    static weak var monitor: Monitor?
    @MainActor static var openWindow: ((String) -> Void)?
    /// Opened by the login item rather than by someone: stays in the menu bar.
    @MainActor private static var atLogin = false
    @MainActor private static var steps = 0

    func applicationDidFinishLaunching(_ n: Notification) {
        MainActor.assumeIsolated {
            AppDelegate.atLogin = Self.launchedAsLoginItem()
            Dock.start()
            AppDelegate.launchStep()
        }
        Notifier.configure(delegate: self)
        if UserDefaults.standard.bool(forKey: "welcomed") {
            Notifier.requestAuthorization()
        }
    }

    /// Opened again from Finder or the Dock while running: show the window.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows {
            MainActor.assumeIsolated { AppDelegate.open("main") }
        }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    /// The first window, once both launching and the menu bar label (which
    /// can open windows) are done, in whichever order they finish: the
    /// welcome the first time, the main window when someone opened the app,
    /// nothing at login.
    @MainActor static func launchStep() {
        steps += 1
        guard steps == 2 else { return }
        if UserDefaults.standard.bool(forKey: "welcomed") == false {
            open("welcome")
        } else if !atLogin {
            open("main")
        }
    }

    /// The open-application event says when the login item opened it. Just
    /// after startup counts too, in case it doesn't.
    @MainActor private static func launchedAsLoginItem() -> Bool {
        if let e = NSAppleEventManager.shared().currentAppleEvent, e.eventID == kAEOpenApplication,
           e.paramDescriptor(forKeyword: keyAEPropData)?.enumCodeValue == keyAELaunchedAsLogInItem {
            return true
        }
        return ProcessInfo.processInfo.systemUptime < 180
    }

    @MainActor static func open(_ id: String) {
        openWindow?(id)
        NSApp.activate(ignoringOtherApps: true)
        Dock.update()
    }

    /// The main window, on a page.
    @MainActor static func show(_ pane: Pane) {
        Navigator.shared.pane = pane
        open("main")
    }

    /// Show the banner even while the app is in front.
    func userNotificationCenter(_ c: UNUserNotificationCenter, willPresent n: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound])
    }

    func userNotificationCenter(_ c: UNUserNotificationCenter, didReceive r: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        #if !APP_STORE
        // Sparkle's reminder: clicking it is "show me", so run the check that
        // brings the update alert forward.
        if r.notification.request.identifier == Notifier.updateID {
            Task { @MainActor in
                defer { done() }
                AppDelegate.monitor?.updates.checkForUpdates()
            }
            return
        }
        #endif
        let info = r.notification.request.content.userInfo
        let action = r.actionIdentifier
        #if !APP_STORE
        let tweak = info["tweak"] as? String
        #endif
        let command = info["command"] as? String
        Task { @MainActor in
            defer { done() }
            switch action {
            #if !APP_STORE
            case Notifier.Action.applyTweak.rawValue:
                if let tweak { AppDelegate.monitor?.apply(tweak) }
            #endif
            case Notifier.Action.copyCommand.rawValue:
                if let command { AppDelegate.monitor?.copy(command) }
            default:
                AppDelegate.show(.overview)
            }
        }
    }
}
