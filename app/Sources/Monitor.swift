// What the windows and the menu bar panel show, and when to speak up.
//
// The watcher (a separate process) does all the sampling. The app reads its
// history with `mdif status` every ten minutes and whenever someone looks
// (the panel opens, a window comes forward), which costs a few milliseconds,
// so the menu bar item stays within the budget the watcher keeps. The
// free-space series is read only while someone looks. Suggestions and tweaks
// measure folders and take seconds, so they are only computed then too.

import AppKit
import Combine
import UserNotifications

@MainActor
final class Monitor: ObservableObject {
    @Published private(set) var status: Status?
    @Published private(set) var statusError: String?
    /// The free-space series and what grew and wrote in the last day.
    @Published private(set) var detail: ForecastDetail?
    @Published private(set) var suggestions: [Suggestion]?
    @Published private(set) var tweaks: [TweakInfo]?
    /// What is never suggested for deleting, cleaning or moving.
    @Published private(set) var kept: [Kept]?
    /// Clean-ups under way, by their first path.
    @Published private(set) var cleaning: Set<String> = []
    @Published private(set) var measuring = false
    /// Clean-ups the watcher does once a day, when they're on.
    @Published private(set) var tidies: [Tidy]?
    /// Time Machine: the backups, the snapshots, and (once its page was
    /// open) the folders.
    @Published private(set) var timeMachine: TimeMachineReport?
    @Published private(set) var timeMachineLoading = false
    /// Folders being kept out of the backups or let back in.
    @Published private(set) var backupChanging: Set<String> = []
    @Published private(set) var backupStarting = false
    private var timeMachineAt: Date?
    /// Being switched or run, by id.
    @Published private(set) var tidyBusy: Set<String> = []
    /// Where the space goes, from a whole-disk scan `mdif` caches.
    @Published private(set) var space: SpaceReport?
    @Published private(set) var spaceMeasuring = false
    @Published private(set) var spaceError: String?
    #if !APP_STORE
    /// Tweaks applied or undone here that no measurement has read since: the
    /// switch shows the new state meanwhile.
    @Published private(set) var tweaksChanging: [String: Bool] = [:]
    /// When one was last applied or undone here.
    private var tweakChanged = Date.distantPast
    #endif
    private var measureAgain = false
    @Published private(set) var lastRefresh: Date?

    #if APP_STORE
    /// Taken up first: every `mdif`, the watcher's too, inherits it.
    let access = DiskAccess()
    #endif
    let watcher = WatcherService()
    let settings = AppSettings()
    #if !APP_STORE
    lazy var updates = Updater(settings: settings)
    #endif

    private var timer: Timer?
    private var measuredAt: Date?
    private var lookObserver: NSObjectProtocol?
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
        // A window or the menu bar panel came forward: someone is looking.
        lookObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.viewed() }
        }
        wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.wokeAt = Date() }
        }
        #if APP_STORE
        watcher.resume()
        #else
        watcher.restartIfUpdated()
        #endif
        #if !APP_STORE
        // Sparkle's daily schedule starts with it, not when a window asks.
        _ = updates
        #endif
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

    /// Everything read again now, the whole-disk measure too: Refresh (⌘R).
    func refreshAll() {
        Task {
            measuredAt = nil
            await refresh()
            await loadDetail()
            await measure()
            await loadTidies()
            await loadSpace(refresh: true)
        }
    }

    /// Someone is looking: make what they see current.
    func viewed() {
        if lastRefresh.map({ Date().timeIntervalSince($0) > 30 }) ?? true {
            Task {
                await refresh()
                await loadDetail()
            }
        } else if detail == nil {
            Task { await loadDetail() }
        }
        if measuredAt.map({ Date().timeIntervalSince($0) > Self.measureEvery }) ?? true {
            Task { await measure() }
        }
        #if !APP_STORE
        // Whether backups keep failing, for Needs Attention: a second's look.
        if timeMachineAt.map({ Date().timeIntervalSince($0) > Self.measureEvery }) ?? true {
            timeMachineAt = Date()
            Task { await loadTimeMachine(quick: true) }
        }
        #endif
    }

    /// The series behind the charts, from the same history: milliseconds.
    func loadDetail() async {
        if let d = try? await Core.json(ForecastDetail.self, ["forecast", "--series"], timeout: 60) {
            detail = d
        }
    }

    /// Suggestions and tweaks, which size the folders they're about.
    func measure() async {
        // Asked for while one runs, which may have started before a change:
        // it runs again once that one is done.
        guard !measuring else {
            measureAgain = true
            return
        }
        measuring = true
        defer { measuring = false }
        repeat {
            measureAgain = false
            #if APP_STORE
            // Its tweaks are only advice: no need to read other apps' settings.
            if let sg = try? await Core.json([Suggestion].self, ["suggest"]) { suggestions = notKept(sg) }
            #else
            let started = Date()
            async let t = try? Core.json([TweakInfo].self, ["tweaks"])
            async let s = try? Core.json([Suggestion].self, ["suggest"])
            let (tw, sg) = await (t, s)
            // Read before a tweak changed, it would put back the old state.
            if let tw, started > tweakChanged {
                tweaks = tw
                tweaksChanging = [:]
            }
            if let sg { suggestions = notKept(sg) }
            #endif
            measuredAt = Date()
        } while measureAgain
    }

    // MARK: - Where the space goes

    /// A scan younger than this is shown as it is.
    static let spaceMaxAge: Int = 6 * 3600

    /// The breakdown, from the cache when it's recent; `refresh` measures
    /// again (about a minute for a full disk).
    func loadSpace(refresh: Bool = false) async {
        guard !spaceMeasuring, spaceReadable else { return }
        spaceMeasuring = true
        defer { spaceMeasuring = false }
        let args = ["space"] + (refresh ? ["--refresh"] : ["--max-age", "\(Self.spaceMaxAge)"])
        do {
            space = try await Core.json(SpaceReport.self, args, timeout: 900)
            spaceError = nil
        } catch {
            spaceError = error.localizedDescription
        }
    }

    /// Changes what raises a notification (`mdif alerts set`), then reads
    /// the status, which carries it.
    func setAlert(_ key: String, _ value: String) {
        Task {
            do {
                _ = try await Core.json(AlertSettings.self, ["alerts", "set", key, value], timeout: 30)
            } catch {
                log.error("alerts set \(key, privacy: .public): \(error.localizedDescription, privacy: .public)")
            }
            await refresh()
        }
    }

    /// `quick`: the backups and the snapshots only, in a second; the
    /// folders keep what the last full look found.
    func loadTimeMachine(quick: Bool = false) async {
        guard !timeMachineLoading else { return }
        timeMachineLoading = true
        defer { timeMachineLoading = false }
        let args = ["timemachine"] + (quick ? ["--quick"] : [])
        guard let r = try? await Core.json(TimeMachineReport.self, args, timeout: 180) else { return }
        if quick, let full = timeMachine, !full.rebuildable.isEmpty || !full.changes.isEmpty {
            timeMachine = r.with(changes: full.changes, rebuildable: full.rebuildable)
        } else {
            timeMachine = r
        }
    }

    #if !APP_STORE
    /// Keeps folders out of Time Machine's backups, or lets them back in
    /// (`mdif timemachine exclude|include`, which checks each again).
    func setBackedUp(_ paths: [String], _ backedUp: Bool) {
        let paths = paths.filter { !backupChanging.contains($0) }
        guard !paths.isEmpty else { return }
        backupChanging.formUnion(paths)
        Task {
            defer { backupChanging.subtract(paths) }
            do {
                let r = try await Core.json(BackupExclusion.self,
                                            ["timemachine", backedUp ? "include" : "exclude"] + paths, timeout: 120)
                if !r.refused.isEmpty {
                    TweakFlow.show(.warning, backedUp ? "Not backed up again" : "Not kept out",
                                   r.refused.map { "• \($0.why)" }.joined(separator: "\n"))
                }
                timeMachine = timeMachine?.marking(r.done, backedUp: backedUp)
            } catch {
                TweakFlow.show(.warning, "Nothing was changed", error.localizedDescription)
            }
        }
    }

    /// Back Up Now, as in Time Machine's menu.
    func backUpNow() {
        guard !backupStarting else { return }
        backupStarting = true
        Task {
            defer { backupStarting = false }
            let r = try? await Core.json(BackupStarted.self, ["timemachine", "back-up"], timeout: 60)
            if r?.started != true {
                TweakFlow.show(.warning, "Time Machine didn't start", r?.why ?? "mdif couldn't ask it.")
            }
            await loadTimeMachine(quick: true)
        }
    }
    #endif

    func loadTidies() async {
        if let list = try? await Core.json(TidyList.self, ["tidy"], timeout: 30) {
            tidies = list.tidies
        }
    }

    #if !APP_STORE
    /// Turns one on or off; `mdif tidy` prints the list as it is then.
    func setTidy(_ id: String, on: Bool) {
        guard !tidyBusy.contains(id) else { return }
        tidyBusy.insert(id)
        Task {
            defer { tidyBusy.remove(id) }
            do {
                tidies = try await Core.json(TidyList.self, ["tidy", on ? "on" : "off", id], timeout: 30).tidies
            } catch {
                TweakFlow.show(.warning, "Couldn't change it", error.localizedDescription)
            }
        }
    }

    /// Runs one now, instead of waiting for the watcher's daily turn.
    func tidyNow(_ id: String) {
        guard !tidyBusy.contains(id) else { return }
        tidyBusy.insert(id)
        Task {
            defer { tidyBusy.remove(id) }
            let r: TidyRun
            do {
                r = try await Core.json(TidyRun.self, ["tidy", "run", id], timeout: 1800)
            } catch {
                TweakFlow.show(.warning, "Nothing was tidied", error.localizedDescription)
                return
            }
            var lines: [String] = []
            if id == "backups" {
                // Nothing is freed: folders are kept out of the backups.
                lines.append(r.done.isEmpty ? "Every build folder was kept out already."
                             : "Kept \(r.done.count) folder\(r.done.count == 1 ? "" : "s") out of Time Machine.")
            } else {
                lines.append(r.done.isEmpty ? "Nothing needed tidying." : "The disk has \(Format.bytes(r.freed)) more free.")
            }
            if !r.failed.isEmpty {
                lines.append("Not done:\n" + r.failed.map { "• \($0.why)" }.joined(separator: "\n"))
            }
            TweakFlow.show(.informational, r.done.isEmpty ? "All tidy" : "Tidied", lines.joined(separator: "\n\n"))
            await loadTidies()
            if id == "backups" { await loadTimeMachine() }
            if !r.done.isEmpty {
                measuredAt = nil
                await refresh()
                await loadSpace()
            }
        }
    }
    #endif

    /// Whether `mdif space` can see the disk. In the App Sandbox, only when
    /// the whole disk was chosen, not one folder.
    var spaceReadable: Bool {
        #if APP_STORE
        access.wholeDisk
        #else
        true
        #endif
    }

    /// Each suggestion goes with the first item it's about (its folder, or
    /// one in it or around it), biggest kinds first, and shows there.
    var placements: [String: Suggestion] {
        guard let r = space, let list = suggestions else { return [:] }
        var out: [String: Suggestion] = [:]
        var placed = Set<String>()
        for k in r.kinds {
            // System data has no path: it's about nothing.
            for i in k.items where !i.path.isEmpty {
                let p = Self.dataless(i.path)
                if let s = list.first(where: { s in
                    guard let sp = s.path.map(Self.dataless), !placed.contains(s.cause) else { return false }
                    return sp == p || p.hasPrefix(sp + "/") || sp.hasPrefix(p + "/")
                }) {
                    out[i.path] = s
                    placed.insert(s.cause)
                }
            }
        }
        return out
    }

    func suggestion(about item: SpaceReport.Item) -> Suggestion? { placements[item.path] }

    /// The suggestion made from an incident: it carries the incident's
    /// button (Restart, Delete).
    func suggestion(for e: Episode) -> Suggestion? {
        suggestions?.first { $0.cause == e.sentence }
    }

    /// Suggestions shown neither with an item nor under Needs Attention.
    var unplacedSuggestions: [Suggestion] {
        let placed = Set(placements.values.map(\.cause))
        let incidents = Set((status?.incidents ?? []).map(\.sentence))
        return (suggestions ?? []).filter { !placed.contains($0.cause) && !incidents.contains($0.cause) }
    }

    /// A path as the home folder sees it: the Data volume's firmlink prefix off.
    private static func dataless(_ p: String) -> String {
        let data = "/System/Volumes/Data"
        return p.hasPrefix(data + "/") ? String(p.dropFirst(data.count)) : p
    }

    // MARK: - What you keep

    /// Kept since the app started: a measurement under way when it was
    /// kept doesn't know yet, so its suggestions are left out here.
    private var keptNow: [String] = []

    private func notKept(_ list: [Suggestion]) -> [Suggestion] {
        list.filter { s in
            !keptNow.contains { k in s.path == k || s.path?.hasPrefix(k + "/") == true }
        }
    }

    func loadKept() async {
        if let k = try? await Core.json([Kept].self, ["keep"]) { kept = k }
    }

    /// Never suggest deleting, cleaning or moving these again.
    func keep(_ paths: [String]) {
        keptNow += paths
        suggestions = suggestions.map(notKept)
        Task {
            do {
                kept = try await Core.json([Kept].self, ["keep", "add"] + paths)
            } catch {
                keptNow.removeAll { paths.contains($0) }
                log.error("keep: \(error.localizedDescription, privacy: .public)")
            }
            measuredAt = nil
            await measure()
        }
    }

    func unkeep(_ k: Kept) {
        keptNow.removeAll { $0 == k.path }
        Task {
            if let list = try? await Core.json([Kept].self, ["keep", "remove", k.entry]) { kept = list }
            measuredAt = nil
            await measure()
        }
    }

    func tweak(_ id: String) -> TweakInfo? { tweaks?.first { $0.id == id } }

    // MARK: - What the menu bar and the windows show

    var health: Health { Health(status) }

    /// How many things the overview asks someone to look at.
    var attentionCount: Int {
        (status?.incidents.count ?? 0) + undone.count + (watcherProblem == nil ? 0 : 1)
            + (lacksFullDiskAccess ? 1 : 0) + (timeMachine?.backlog == nil ? 0 : 1)
    }

    /// Tweaks this app applied that something undid (none in the App Store
    /// edition, which can't apply them).
    var undone: [AppliedTweak] { Edition.appStore ? [] : status?.tweaks.filter(\.undone) ?? [] }

    var watcherProblem: String? {
        if !watcher.isOn && !watcher.hasCommandLineAgent { return "The watcher is off" }
        let settled = Date().timeIntervalSince(wokeAt) > 2 * Double(max(status?.history.intervalSecs ?? 300, 60))
        if let s = status, watcher.isOn, settled, s.history.samples > 0, s.watcherStale() {
            return "The watcher hasn't sampled for \(Format.duration(Int64(Date().timeIntervalSince1970) - (s.history.last ?? 0)))"
        }
        return nil
    }

    #if APP_STORE
    /// In the App Sandbox: no disk chosen yet, so only free space is watched.
    var lacksFullDiskAccess: Bool { !access.granted }

    /// The disk (or a folder) was chosen: everything that reads it starts
    /// again to see it.
    func chooseDisk() {
        guard access.choose() else { return }
        objectWillChange.send()
        watcher.restart()
        measuredAt = nil
        space = nil
        Task {
            await refresh()
            await measure()
            await MapModel.shared.measure()
            await loadSpace(refresh: true)
        }
    }
    #else
    var lacksFullDiskAccess: Bool { status?.access?.fullDiskAccess == false }
    #endif

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
        // What Settings → Notifications turned off is still shown, not
        // announced.
        let wanted: (Episode) -> Bool = { e in
            e.incident.kind == .growth ? (s.alerts?.notifyGrowth ?? true) : (s.alerts?.notifyChurn ?? true)
        }
        for e in s.incidents where !known.contains(e.id) && wanted(e) {
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
            // Almost full is always said; the forecast only if wanted.
            if s.alert == .low || (s.alerts?.notifyForecast ?? true) {
                Notifier.alert(s)
            }
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

    #if !APP_STORE
    func apply(_ id: String) {
        Task { await afterTweak(id, await TweakFlow.apply(id), on: true) }
    }

    func revert(_ id: String) {
        Task { await afterTweak(id, await TweakFlow.revert(id, title: tweak(id)?.title ?? id), on: false) }
    }

    /// Does a suggestion's clean-up, then measures again.
    func clean(_ a: CleanAction) {
        guard !a.paths.isEmpty, cleaning.isDisjoint(with: a.paths) else { return }
        // Busy only once it's confirmed: not "Moving to the Trash…" beside
        // the question.
        guard CleanFlow.confirmed(a) else { return }
        cleaning.formUnion(a.paths)
        Task {
            let done = await CleanFlow.run(a)
            cleaning.subtract(a.paths)
            if done {
                suggestions = suggestions?.filter { $0.action?.paths != a.paths }
                measuredAt = nil
                await refresh()
                await measure()
                // `mdif clean` measured again what it changed: the breakdown
                // follows at once, without a whole-disk scan.
                await loadSpace()
            }
        }
    }

    /// The switch shows the new state at once; the measurement that follows
    /// says how it really is.
    private func afterTweak(_ id: String, _ outcome: TweakFlow.Outcome, on: Bool) async {
        guard outcome != .unchanged else { return }
        tweakChanged = Date()
        if outcome == .done { tweaksChanging[id] = on }
        measuredAt = nil
        await refresh()
        await measure()
    }
    #endif

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
