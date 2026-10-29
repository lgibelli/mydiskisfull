// The App Store edition in the App Sandbox: the watcher runs while the app
// does, and the app sees the disk once someone picks it in an Open dialog.
//
// A sandboxed agent would get a container of its own, apart from the app's
// history, and App Review asks that nothing keep running after the app quits
// without consent (guideline 2.4.5). So `mdif watch` is a child of the app:
// it opens at login if you let it, and the watcher exits when its standard
// input, a pipe from the app, closes, even if the app crashes.
//
// Every `mdif` the app starts inherits its sandbox, and with it access to the
// disk picked in the dialog, kept as a security-scoped bookmark and taken up
// at launch before any of them starts.

#if APP_STORE
import AppKit
import ServiceManagement

@MainActor
final class WatcherService: ObservableObject {
    /// `.enabled` while it runs, as the full edition's agent reports it.
    @Published private(set) var status: SMAppService.Status = .notRegistered
    @Published private(set) var problem: String?
    @Published private(set) var busy = false

    private var process: Process?
    private var input: Pipe?
    /// When it stopped by itself lately: a watcher that keeps stopping isn't
    /// started again and again.
    private var exits: [Date] = []
    private var quitObserver: NSObjectProtocol?

    private static let wantedKey = "watcherOn"
    private var wanted: Bool {
        get { UserDefaults.standard.bool(forKey: Self.wantedKey) }
        set { UserDefaults.standard.set(newValue, forKey: Self.wantedKey) }
    }

    init() {
        quitObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.willTerminateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.halt() }
        }
    }

    var isOn: Bool { status == .enabled }
    /// `mdif agent install` is the full edition's.
    let hasCommandLineAgent = false

    var caption: String {
        isOn
            ? "Running while \(Edition.name) is open. It samples free space every 5 minutes, and which folders grew every 30."
            : "Nothing is watched while it's off."
    }

    func refresh() {
        status = process?.isRunning == true ? .enabled : .notRegistered
    }

    /// At launch, once the disk access is taken up: as it was left.
    func resume() {
        if wanted { launch() }
    }

    func start() async {
        wanted = true
        exits = []
        launch()
    }

    func stop() async {
        wanted = false
        halt()
    }

    /// A running watcher keeps the access it started with: after the disk is
    /// chosen, it starts again to see it.
    func restart() {
        guard wanted else { return }
        halt()
        launch()
    }

    func openLoginItems() {
        SMAppService.openSystemSettingsLoginItems()
    }

    private func launch() {
        guard process?.isRunning != true else { return }
        let p = Process()
        p.executableURL = Core.executable
        p.arguments = ["watch", "--quiet", "--log", "--attached"]
        // As the full edition's agent runs: efficiency cores, throttled I/O.
        p.qualityOfService = .background
        let pipe = Pipe()
        p.standardInput = pipe
        p.standardOutput = FileHandle.nullDevice
        p.standardError = FileHandle.nullDevice
        p.terminationHandler = { [weak self] done in
            let status = done.terminationStatus
            Task { @MainActor in self?.exited(done, status: status) }
        }
        do {
            try p.run()
            process = p
            input = pipe
            problem = nil
            log.notice("watcher started, pid \(p.processIdentifier)")
        } catch {
            problem = "Couldn't start the watcher: \(error.localizedDescription)"
            log.error("watcher start failed: \(error.localizedDescription, privacy: .public)")
        }
        refresh()
    }

    private func halt() {
        guard let p = process else { return }
        process = nil
        p.terminationHandler = nil
        try? input?.fileHandleForWriting.close()
        input = nil
        if p.isRunning { p.terminate() }
        refresh()
    }

    private func exited(_ p: Process, status: Int32) {
        guard p === process else { return }
        process = nil
        input = nil
        refresh()
        log.error("watcher exited with status \(status)")
        guard wanted else { return }
        let now = Date()
        exits = exits.filter { now.timeIntervalSince($0) < 3_600 } + [now]
        guard exits.count <= 3 else {
            problem = "The watcher keeps stopping (status \(status)). Turn it off and on again to retry."
            return
        }
        Task {
            try? await Task.sleep(for: .seconds(10))
            if wanted { launch() }
        }
    }
}

/// What the sandbox lets the app (and every `mdif` it starts) see beyond its
/// own container: the disk, or a folder, picked in an Open dialog.
@MainActor
final class DiskAccess: ObservableObject {
    @Published private(set) var url: URL?

    private static let key = "diskAccessBookmark"

    init() {
        resolve()
    }

    var granted: Bool { url != nil }

    /// The startup disk, as Finder names it ("Macintosh HD").
    static var startupDisk: String { FileManager.default.displayName(atPath: "/") }

    /// The whole disk was chosen, not one folder.
    var wholeDisk: Bool { url?.path == "/" }

    /// What was chosen, by name.
    var name: String {
        guard let url else { return "Nothing" }
        return wholeDisk ? Self.startupDisk : FileManager.default.displayName(atPath: url.path)
    }

    /// What the disk map measures: the whole Data volume (`mdif`'s default)
    /// for the disk, or the folder.
    var mapRoot: String? {
        guard let url, !wholeDisk else { return nil }
        return url.path
    }

    /// The Open dialog, at the top of the startup disk. True if something
    /// was chosen.
    @discardableResult
    func choose() -> Bool {
        let panel = NSOpenPanel()
        panel.title = "Let \(Edition.name) See Your Disk"
        panel.message = "Click Allow to let \(Edition.name) see what fills \(Self.startupDisk). It reads names, sizes and dates, never what's in a file, and nothing leaves your Mac."
        panel.prompt = "Allow"
        panel.directoryURL = URL(fileURLWithPath: "/")
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.canCreateDirectories = false
        NSApp.activate(ignoringOtherApps: true)
        guard panel.runModal() == .OK, let picked = panel.url else { return false }
        do {
            let data = try picked.bookmarkData(options: [.withSecurityScope, .securityScopeAllowOnlyReadAccess],
                                               includingResourceValuesForKeys: nil, relativeTo: nil)
            UserDefaults.standard.set(data, forKey: Self.key)
        } catch {
            // Seen now, but not after a restart of the app.
            log.error("keeping the disk access: \(error.localizedDescription, privacy: .public)")
        }
        url?.stopAccessingSecurityScopedResource()
        _ = picked.startAccessingSecurityScopedResource()
        url = picked
        log.notice("disk access: \(picked.path, privacy: .public)")
        return true
    }

    private func resolve() {
        guard let data = UserDefaults.standard.data(forKey: Self.key) else { return }
        var stale = false
        guard let found = try? URL(resolvingBookmarkData: data, options: .withSecurityScope,
                                   relativeTo: nil, bookmarkDataIsStale: &stale),
              found.startAccessingSecurityScopedResource() else {
            log.error("the kept disk access no longer resolves")
            UserDefaults.standard.removeObject(forKey: Self.key)
            return
        }
        if stale, let fresh = try? found.bookmarkData(options: [.withSecurityScope, .securityScopeAllowOnlyReadAccess],
                                                      includingResourceValuesForKeys: nil, relativeTo: nil) {
            UserDefaults.standard.set(fresh, forKey: Self.key)
        }
        url = found
    }
}
#endif
