// MyDiskIsFull: the menu bar app. The watching is done by `mdif watch`, a
// LaunchAgent the app registers; the app reads its history, shows the
// forecast and what fills the disk, and applies and undoes the fixes.

import SwiftUI
import UserNotifications

@main
struct MyDiskIsFullApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var monitor = Monitor()

    var body: some Scene {
        MenuBarExtra {
            MenuView(monitor: monitor)
        } label: {
            MenuBarLabel(monitor: monitor)
        }
        // A real NSMenu, as in Idlewild: system highlighting, keyboard
        // navigation and metrics. The detail lives in the windows.
        .menuBarExtraStyle(.menu)

        Window("MyDiskIsFull", id: "overview") {
            OverviewView(monitor: monitor)
        }
        .defaultSize(width: 760, height: 680)
        .defaultPosition(.center)

        Window("Disk Map", id: "map") {
            MapView()
        }
        .defaultSize(width: 760, height: 680)
        .defaultPosition(.center)

        Window("Welcome to MyDiskIsFull", id: "welcome") {
            WelcomeView(monitor: monitor)
        }
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        Window("About MyDiskIsFull", id: "about") {
            AboutView(monitor: monitor)
        }
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        Settings {
            SettingsView(monitor: monitor)
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
            AppDelegate.openWindow = { id in
                openWindow(id: id)
                NSApp.activate(ignoringOtherApps: true)
            }
            if !monitor.settings.welcomed {
                AppDelegate.openWindow?("welcome")
            }
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

final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    static weak var monitor: Monitor?
    @MainActor static var openWindow: ((String) -> Void)?

    func applicationDidFinishLaunching(_ n: Notification) {
        Notifier.configure(delegate: self)
        if UserDefaults.standard.bool(forKey: "welcomed") {
            Notifier.requestAuthorization()
        }
    }

    /// Show the banner even while the app is in front.
    func userNotificationCenter(_ c: UNUserNotificationCenter, willPresent n: UNNotification,
                                withCompletionHandler done: @escaping (UNNotificationPresentationOptions) -> Void) {
        done([.banner, .sound])
    }

    func userNotificationCenter(_ c: UNUserNotificationCenter, didReceive r: UNNotificationResponse,
                                withCompletionHandler done: @escaping () -> Void) {
        let info = r.notification.request.content.userInfo
        let action = r.actionIdentifier
        let tweak = info["tweak"] as? String
        let command = info["command"] as? String
        Task { @MainActor in
            defer { done() }
            switch action {
            case Notifier.Action.applyTweak.rawValue:
                if let tweak { AppDelegate.monitor?.apply(tweak) }
            case Notifier.Action.copyCommand.rawValue:
                if let command { AppDelegate.monitor?.copy(command) }
            default:
                AppDelegate.openWindow?("overview")
            }
        }
    }
}
