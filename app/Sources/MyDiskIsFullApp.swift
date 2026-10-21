// MyDiskIsFull: a main window, and a glance in the menu bar. The watching is
// done by `mdif watch`, a LaunchAgent the app registers; the app reads its
// history, shows the forecast and what fills the disk, and applies and undoes
// the fixes (not in the App Store edition: see Edition.swift).
//
// It's in the menu bar and in the Dock. With the Dock icon hidden (Settings)
// it joins the Dock and the app switcher only while a window is open, and
// leaves when the last one closes.

import SwiftUI
import UserNotifications

@main
struct MyDiskIsFullApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var monitor = Monitor()

    var body: some Scene {
        MenuBarExtra {
            StatusMenu(monitor: monitor)
        } label: {
            MenuBarLabel(monitor: monitor)
        }
        .menuBarExtraStyle(.menu)

        Window(Edition.name, id: "main") {
            MainWindow(monitor: monitor)
        }
        .defaultSize(width: 1120, height: 780)
        .defaultPosition(.center)
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands { AppCommands(monitor: monitor) }

        Window("Welcome to \(Edition.name)", id: "welcome") {
            WelcomeView(monitor: monitor)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultPosition(.center)

        Window("About \(Edition.name)", id: "about") {
            AboutView(monitor: monitor)
        }
        .windowStyle(.hiddenTitleBar)
        .windowResizability(.contentSize)
        .defaultPosition(.center)
    }
}

/// The app's menus: About and updates where macOS puts them, the pages in
/// View (⌘1 to ⌘7) and Refresh (⌘R), Settings (⌘,) as a sheet of the
/// main window, nothing to create, and no Hide
/// Sidebar: the sidebar holds the disk's state.
struct AppCommands: Commands {
    let monitor: Monitor

    var body: some Commands {
        CommandGroup(replacing: .appInfo) {
            Button("About \(Edition.name)") { AppDelegate.open("about") }
            #if !APP_STORE
            UpdateItem(updates: monitor.updates)
            #endif
        }
        CommandGroup(replacing: .newItem) {}
        // Settings is a sheet of the main window, not a window of its own.
        CommandGroup(replacing: .appSettings) {
            Button("Settings…") { AppDelegate.showSettings() }
                .keyboardShortcut(",", modifiers: .command)
        }
        CommandGroup(before: .sidebar) {
            ForEach(Pane.allCases) { p in
                Button(p.title) { AppDelegate.show(p) }
                    .keyboardShortcut(p.key, modifiers: .command)
            }
            Divider()
            Button("Refresh") { monitor.refreshAll() }
                .keyboardShortcut("r", modifiers: .command)
            Divider()
        }
        CommandGroup(replacing: .help) {
            Button("\(Edition.name) Help") {
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
            if Edition.appStore {
                Image(nsImage: Icon.lite(monitor.needsAttention ? Icon.attention : Icon.calm))
            } else {
                Image(systemName: monitor.needsAttention ? Icon.attention : Icon.calm)
            }
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

    @MainActor private static var drawn: [String: NSImage] = [:]

    /// The App Store edition's: the symbol, then LITE cut out of a solid
    /// pill, so the two editions differ in the menu bar too. A template, so
    /// it takes the menu bar's colour like any other item.
    @MainActor static func lite(_ symbol: String) -> NSImage {
        if let done = drawn[symbol] { return done }
        let config = NSImage.SymbolConfiguration(pointSize: 14, weight: .regular)
        let glyph = NSImage(systemSymbolName: symbol, accessibilityDescription: nil)?
            .withSymbolConfiguration(config) ?? NSImage()
        let text = NSAttributedString(string: "LITE", attributes: [
            .font: NSFont.systemFont(ofSize: 8.5, weight: .heavy), .kern: 0.4,
        ])
        let label = text.size()
        let pill = NSSize(width: ceil(label.width) + 6, height: 12)
        let gap: CGFloat = 3
        let size = NSSize(width: ceil(glyph.size.width) + gap + pill.width, height: max(ceil(glyph.size.height), pill.height))
        let image = NSImage(size: size, flipped: false) { _ in
            glyph.draw(in: NSRect(x: 0, y: (size.height - glyph.size.height) / 2,
                                  width: glyph.size.width, height: glyph.size.height))
            let box = NSRect(x: size.width - pill.width, y: (size.height - pill.height) / 2,
                             width: pill.width, height: pill.height)
            NSColor.black.setFill()
            NSBezierPath(roundedRect: box, xRadius: 3, yRadius: 3).fill()
            // The letters are holes in the pill.
            NSGraphicsContext.current?.compositingOperation = .destinationOut
            text.draw(at: NSPoint(x: box.midX - (label.width - 0.4) / 2, y: box.midY - label.height / 2 + 0.5))
            return true
        }
        image.isTemplate = true
        image.accessibilityDescription = Edition.name
        drawn[symbol] = image
        return image
    }
}

/// In the Dock like any app. Someone can hide it there (Settings): then it's
/// in the Dock only while a window is open (or Sparkle shows an update), so a
/// window is never left without a way back to it.
@MainActor
enum Dock {
    /// Sparkle's update alert is up.
    static var showingUpdate = false {
        didSet { update() }
    }

    /// Settings → Show in the Dock.
    static var always: Bool {
        get { UserDefaults.standard.object(forKey: "showInDock") as? Bool ?? true }
        set {
            UserDefaults.standard.set(newValue, forKey: "showInDock")
            update()
        }
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
        let want: NSApplication.ActivationPolicy = always || open || showingUpdate ? .regular : .accessory
        guard NSApp.activationPolicy() != want else { return }
        _ = NSApp.setActivationPolicy(want)
        // A window brought it into the Dock: bring it forward too. Not at
        // login, where it only takes its place there.
        if want == .regular && (open || showingUpdate) { NSApp.activate(ignoringOtherApps: true) }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    static weak var monitor: Monitor?
    @MainActor static var openWindow: ((String) -> Void)?
    /// Opened by the login item rather than by someone: stays in the menu bar.
    @MainActor private static var atLogin = false
    @MainActor private static var steps = 0

    /// Into the Dock before anything shows (Info.plist starts it out of it,
    /// so that it never flashes there when it's hidden).
    func applicationWillFinishLaunching(_ n: Notification) {
        MainActor.assumeIsolated { Dock.update() }
    }

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

    /// Right-clicking the icon in the Dock: a way out of it. Settings → Show
    /// in the Dock brings it back; while a window is open it stays until the
    /// window closes.
    func applicationDockMenu(_ sender: NSApplication) -> NSMenu? {
        let menu = NSMenu()
        let item = NSMenuItem(title: "Hide from Dock", action: #selector(hideFromDock), keyEquivalent: "")
        item.target = self
        menu.addItem(item)
        return menu
    }

    @objc private func hideFromDock() {
        MainActor.assumeIsolated { Dock.always = false }
    }

    /// Opened again from Finder or the Dock while running: show the window.
    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows: Bool) -> Bool {
        if !hasVisibleWindows {
            MainActor.assumeIsolated { AppDelegate.open("main") }
        }
        return true
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func applicationWillTerminate(_ n: Notification) {
        Core.stopAll()
    }

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

    /// The main window with Settings over it, on a page or the first.
    @MainActor static func showSettings(_ page: SettingsPage? = nil) {
        open("main")
        Navigator.shared.openSettings(page)
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
