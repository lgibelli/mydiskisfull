// The watcher: `mdif watch` as a LaunchAgent that the app registers with
// SMAppService. Its plist is in Contents/Library/LaunchAgents and runs the
// mdif next to the app, so it moves and updates with the app, and shows in
// System Settings → Login Items under the app's name.

import AppKit
import ServiceManagement

@MainActor
final class WatcherService: ObservableObject {
    static let label = "io.github.lgibelli.mydiskisfull.watcher"
    /// What `mdif agent install` uses. A label of its own for the app's:
    /// macOS keeps a record of a command-line agent under its label, and an
    /// app's agent with that label would inherit it, disabled and refused.
    static let commandLineLabel = "io.github.lgibelli.mydiskisfull.watch"

    @Published private(set) var status: SMAppService.Status = .notRegistered
    @Published private(set) var problem: String?
    @Published private(set) var busy = false

    private let service = SMAppService.agent(plistName: "\(label).plist")

    init() { refresh() }

    var isOn: Bool { status == .enabled }

    /// Where `mdif agent install` puts its LaunchAgent. It keeps the same
    /// history, so the app takes over from it.
    var commandLineAgent: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/LaunchAgents/\(Self.commandLineLabel).plist")
    }

    var hasCommandLineAgent: Bool {
        FileManager.default.fileExists(atPath: commandLineAgent.path)
    }

    var caption: String {
        switch status {
        case .enabled: "Running. It samples free space and disk writes every 5 minutes."
        case .requiresApproval: "Waiting for you to allow it in System Settings → General → Login Items."
        case .notFound: "This copy of the app can't register it. Move the app to Applications."
        default:
            hasCommandLineAgent
                ? "The one from `mdif agent install` is running; turning this on moves it into the app."
                : "Off. Nothing is watched while it's off."
        }
    }

    func refresh() {
        status = service.status
    }

    func start() async {
        busy = true
        defer { busy = false; refresh() }
        do {
            if hasCommandLineAgent {
                // Only one watcher may sample: stop the command-line one first.
                _ = try await Core.run(["agent", "uninstall"])
            }
            try service.register()
            problem = nil
            log.notice("watcher registered, status \(self.service.status.rawValue)")
        } catch {
            problem = "Couldn't start the watcher: \(error.localizedDescription)"
            log.error("watcher register failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    func stop() async {
        busy = true
        defer { busy = false; refresh() }
        do {
            try await service.unregister()
            problem = nil
        } catch {
            problem = "Couldn't stop the watcher: \(error.localizedDescription)"
        }
    }

    func openLoginItems() {
        SMAppService.openSystemSettingsLoginItems()
    }
}

/// Full Disk Access: without it macOS keeps the watcher out of other apps'
/// folders, so their growth goes unseen.
enum Privacy {
    /// Whether this app has it; the commands it runs (`mdif map`, `mdif
    /// suggest`) inherit it. Checked as `mdif` does: by opening a privacy
    /// database, which only works with it and never shows a prompt (macOS 27
    /// has no per-user one, so the system's comes first).
    static var appHasFullDiskAccess: Bool? {
        let probes = ["/Library/Application Support/com.apple.TCC/TCC.db",
                      FileManager.default.homeDirectoryForCurrentUser
                          .appendingPathComponent("Library/Application Support/com.apple.TCC/TCC.db").path]
        var refused = false
        for p in probes {
            let fd = open(p, O_RDONLY)
            if fd >= 0 {
                close(fd)
                return true
            }
            if errno == EPERM { refused = true }
        }
        return refused ? false : nil
    }

    static func openSettings() {
        let pane = "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
        if let url = URL(string: pane) { NSWorkspace.shared.open(url) }
    }
}
