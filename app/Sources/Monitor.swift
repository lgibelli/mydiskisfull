// What the menu shows, and when to speak up.
//
// The watcher (a separate process) does all the sampling. The app reads its
// history with `mdif status` every ten minutes and whenever the menu opens,
// which costs a few milliseconds, so the menu bar item stays within the
// budget the watcher keeps. Suggestions and tweaks measure folders and take
// seconds, so they are only computed when the menu is opened.

import AppKit
import Combine
import UserNotifications

@MainActor
final class Monitor: ObservableObject {
    @Published private(set) var status: Status?
    @Published private(set) var statusError: String?
    @Published private(set) var suggestions: [Suggestion]?
    @Published private(set) var tweaks: [TweakInfo]?
    @Published private(set) var measuring = false
    @Published private(set) var lastRefresh: Date?

    let watcher = WatcherService()
    let settings = AppSettings()

    private var timer: Timer?
    private var measuredAt: Date?
    private var menuObserver: NSObjectProtocol?
    private var wakeObserver: NSObjectProtocol?
    private var watcherChanges: AnyCancellable?
    private var refreshing = false
    /// After a wake the watcher's last sample is old until its next one, so
    /// it isn't called stuck until it has had time to take one.
    private var wokeAt = Date()
    /// Whether macOS will show our notifications; until it will, nothing
    /// counts as announced.
    private var notificationsAllowed = false

    static let refreshEvery: TimeInterval = 600
    static let measureEvery: TimeInterval = 600

    init() {
        AppDelegate.monitor = self
        // The menu reads the watcher's state through the monitor.
        watcherChanges = watcher.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
        }
        let t = Timer(timeInterval: Self.refreshEvery, repeats: true) { [weak self] _ in
            Task { @MainActor in await self?.refresh() }
        }
        t.tolerance = 120
        RunLoop.main.add(t, forMode: .common)
        timer = t
        menuObserver = NotificationCenter.default.addObserver(
            forName: NSMenu.didBeginTrackingNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.menuOpened() }
        }
        wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.wokeAt = Date() }
        }
        Task { await refresh() }
    }

    // MARK: - Reading the history

    func refresh() async {
        guard !refreshing else { return }
        refreshing = true
        defer { refreshing = false }
        let n = await UNUserNotificationCenter.current().notificationSettings()
        let allowed = [.authorized, .provisional].contains(n.authorizationStatus)
        if allowed != notificationsAllowed || lastRefresh == nil {
            // 0 not asked yet, 1 denied, 2 allowed, 3 provisional.
            log.notice("notification authorization status \(n.authorizationStatus.rawValue)")
        }
        notificationsAllowed = allowed
        do {
            let s = try await Core.json(Status.self, ["status"], timeout: 60)
            status = s
            statusError = nil
            log.info("status: \(s.avail) free, alert \(s.alert.rawValue, privacy: .public), \(s.incidents.count) incidents, \(s.history.samples) samples")
            speakUp(about: s)
        } catch {
            statusError = error.localizedDescription
        }
        lastRefresh = Date()
        watcher.refresh()
    }

    /// The menu opened: make what it shows current.
    func menuOpened() {
        if lastRefresh.map({ Date().timeIntervalSince($0) > 30 }) ?? true {
            Task { await refresh() }
        }
        if measuredAt.map({ Date().timeIntervalSince($0) > Self.measureEvery }) ?? true {
            Task { await measure() }
        }
    }

    /// Suggestions and tweaks, which size the folders they're about.
    func measure() async {
        guard !measuring else { return }
        measuring = true
        defer { measuring = false }
        async let t = try? Core.json([TweakInfo].self, ["tweaks"])
        async let s = try? Core.json([Suggestion].self, ["suggest"])
        let (tw, sg) = await (t, s)
        if let tw { tweaks = tw }
        if let sg { suggestions = sg }
        measuredAt = Date()
    }

    func tweak(_ id: String) -> TweakInfo? { tweaks?.first { $0.id == id } }

    // MARK: - What the menu bar shows

    var undone: [AppliedTweak] { status?.tweaks.filter(\.undone) ?? [] }

    var watcherProblem: String? {
        if !watcher.isOn && !watcher.hasCommandLineAgent { return "The watcher is off" }
        let settled = Date().timeIntervalSince(wokeAt) > 2 * Double(max(status?.history.intervalSecs ?? 300, 60))
        if let s = status, watcher.isOn, settled, s.history.samples > 0, s.watcherStale() {
            return "The watcher hasn't sampled for \(Format.duration(Int64(Date().timeIntervalSince1970) - (s.history.last ?? 0)))"
        }
        return nil
    }

    var lacksFullDiskAccess: Bool { status?.access?.fullDiskAccess == false }

    /// Something a person should look at.
    var needsAttention: Bool {
        guard let s = status else { return false }
        return !s.incidents.isEmpty || s.alert >= .within3Days || !undone.isEmpty
    }

    /// Days to full, next to the icon, once there's an alert.
    var menuBarText: String? {
        guard let s = status else { return nil }
        if s.alert == .low { return Format.bytes(s.avail) }
        if s.alert > .none, let d = s.forecast?.fullIn?.days { return Format.daysShort(d) }
        return nil
    }

    // MARK: - Notifications

    /// Each incident once, each alert level once per episode, each undone
    /// tweak once. What's over is taken back.
    private func speakUp(about s: Status) {
        // Nothing is recorded as announced unless it could be: what is still
        // open when notifications are allowed is announced then.
        guard settings.notificationsEnabled, notificationsAllowed else { return }
        let d = UserDefaults.standard

        // Incidents.
        let known = Set(d.stringArray(forKey: Keys.incidents) ?? [])
        let open = Set(s.incidents.map(\.id))
        for e in s.incidents where !known.contains(e.id) {
            let title: String? = if case .tweak(let id) = e.fix { tweak(id)?.title } else { nil }
            Notifier.incident(e, tweakTitle: title)
        }
        Notifier.withdraw(known.subtracting(open).map { "incident-\($0)" })
        d.set(Array(open), forKey: Keys.incidents)

        // The forecast: up a level, announced; down a level, only after six
        // hours there, so a forecast hovering at a boundary doesn't repeat.
        let notified = Alert(rank: d.integer(forKey: Keys.level))
        let now = Date().timeIntervalSince1970
        if s.alert > notified {
            Notifier.alert(s)
            d.set(s.alert.rank, forKey: Keys.level)
            d.removeObject(forKey: Keys.lowerSince)
        } else if s.alert < notified {
            let since = d.double(forKey: Keys.lowerSince)
            if since == 0 {
                d.set(now, forKey: Keys.lowerSince)
            } else if now - since > 6 * 3_600 {
                d.set(s.alert.rank, forKey: Keys.level)
                d.removeObject(forKey: Keys.lowerSince)
                if s.alert == .none { Notifier.withdraw(["alert"]) }
            }
        } else {
            d.removeObject(forKey: Keys.lowerSince)
        }

        // Tweaks something undid.
        let undoneKnown = Set(d.stringArray(forKey: Keys.undone) ?? [])
        let undoneNow = Set(undone.map { "\($0.id):\($0.applied)" })
        for t in undone where !undoneKnown.contains("\(t.id):\(t.applied)") {
            Notifier.undone(t)
        }
        let fixed = undoneKnown.subtracting(undoneNow).compactMap { $0.split(separator: ":").first }
        Notifier.withdraw(fixed.map { "undone-\($0)" })
        d.set(Array(undoneNow), forKey: Keys.undone)
    }

    private enum Keys {
        static let incidents = "notified.incidents"
        static let level = "notified.alertLevel"
        static let lowerSince = "notified.alertLowerSince"
        static let undone = "notified.undone"
    }

    // MARK: - Acting on a fix

    func copy(_ command: String) {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(command, forType: .string)
    }

    func apply(_ id: String) {
        Task {
            if await TweakFlow.apply(id) { await afterTweak() }
        }
    }

    func revert(_ id: String) {
        Task {
            if await TweakFlow.revert(id, title: tweak(id)?.title ?? id) { await afterTweak() }
        }
    }

    private func afterTweak() async {
        measuredAt = nil
        await refresh()
        await measure()
    }

    // MARK: - Its own cost

    /// CPU this app and the commands it ran have used since it started, in
    /// milliseconds: the About window shows it next to the watcher's.
    var ownCPUms: Double {
        func ms(_ who: Int32) -> Double {
            var u = rusage()
            getrusage(who, &u)
            let t = { (tv: timeval) in Double(tv.tv_sec) * 1000 + Double(tv.tv_usec) / 1000 }
            return t(u.ru_utime) + t(u.ru_stime)
        }
        return ms(RUSAGE_SELF) + ms(RUSAGE_CHILDREN)
    }

    let launched = Date()
}

/// The app's few settings, in its defaults.
final class AppSettings {
    private let d = UserDefaults.standard

    init() {
        d.register(defaults: ["notificationsEnabled": true])
    }

    var notificationsEnabled: Bool {
        get { d.bool(forKey: "notificationsEnabled") }
        set { d.set(newValue, forKey: "notificationsEnabled") }
    }

    /// The welcome window was shown.
    var welcomed: Bool {
        get { d.bool(forKey: "welcomed") }
        set { d.set(newValue, forKey: "welcomed") }
    }
}
