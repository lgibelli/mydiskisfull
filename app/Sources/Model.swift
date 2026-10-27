// What `mdif … --json` prints, as the app reads it.
//
// Foundation only: `app/check-contract.sh` compiles this file on its own and
// decodes the real command's output with it, so a change on either side that
// breaks the other fails CI instead of the menu.

import Foundation

enum Wire {
    static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        let d = JSONDecoder()
        // Our own snake_case to camelCase, not .convertFromSnakeCase: that
        // one capitalizes after a digit ("writes_24h" becomes "writes24H"),
        // and how depends on the Foundation version.
        d.keyDecodingStrategy = .custom { path in
            let parts = path[path.count - 1].stringValue.split(separator: "_")
            let camel = parts.enumerated().map { i, p in
                i == 0 ? String(p) : p.prefix(1).uppercased() + p.dropFirst()
            }
            return Key(camel.joined())
        }
        return try d.decode(type, from: data)
    }

    private struct Key: CodingKey {
        let stringValue: String
        var intValue: Int? { nil }
        init(_ s: String) { stringValue = s }
        init?(stringValue: String) { self.stringValue = stringValue }
        init?(intValue: Int) { nil }
    }
}

// MARK: - mdif status --json

struct Status: Decodable {
    let volume: String
    let total: Int64
    let avail: Int64
    /// Below this much free space is an alert whatever the trend.
    let floor: Int64
    let alert: Alert
    let alertSentence: String?
    let forecast: Forecast?
    let history: History
    let incidents: [Episode]
    /// What raises a notification, as set in Settings.
    let alerts: AlertSettings?
    /// Tweaks MyDiskIsFull applied and hasn't reverted.
    let tweaks: [AppliedTweak]
    let access: Access?
    let watcherCost: WatcherCost?

    /// The watcher's last sample is older than three intervals.
    func watcherStale(now: Date = Date()) -> Bool {
        guard let last = history.last else { return true }
        return now.timeIntervalSince1970 - Double(last) > 3 * Double(max(history.intervalSecs, 60))
    }
}

/// From least to most urgent; each level is announced once per episode.
enum Alert: String, Decodable, Comparable {
    case none
    case within14Days = "14_days"
    case within3Days = "3_days"
    case withinADay = "1_day"
    case low

    var rank: Int {
        switch self {
        case .none: 0
        case .within14Days: 1
        case .within3Days: 2
        case .withinADay: 3
        case .low: 4
        }
    }

    init(rank: Int) {
        self = [.none, .within14Days, .within3Days, .withinADay, .low][max(0, min(rank, 4))]
    }

    static func < (a: Alert, b: Alert) -> Bool { a.rank < b.rank }
}

struct Forecast: Decodable {
    let at: Int64
    let avail: Int64
    /// What macOS frees by itself when it needs to (purgeable), counted in
    /// the time to full (`nil` from an older `mdif`).
    let purgeable: Int64?
    /// The longest window with enough data.
    let rate: Rate?
    /// The shortest window, when it differs from `rate`.
    let recent: Rate?
    let fullIn: FullIn?
    /// When the recent rate fills the disk much sooner.
    let recentFullIn: FullIn?
    let steps: [Step]
    /// Space taken in one go (a big download, a macOS update) and left out
    /// as a one-off.
    let arrivals: [Arrival]?
    /// "simulation" (futures drawn from the past days) or "trend" (a
    /// straight line, with less than a day of history).
    let method: String?
    /// The straight-line trend, for comparison.
    let trend: Rate?
    /// The chance of being full within 1, 3 and 14 days (simulation only).
    let fullBy: FullBy?
    /// How fast it filled over the last hour, day and week, bytes a day.
    let paces: Paces?

    struct FullBy: Decodable {
        let d1: Double
        let d3: Double
        let d14: Double
    }

    struct Paces: Decodable {
        let hour: Double?
        let day: Double?
        let week: Double?
    }
}

struct Rate: Decodable {
    /// Seconds.
    let window: Int64
    let points: Int
    /// Bytes a day; negative when the disk is getting emptier.
    let perDay: Double
    let low: Double
    let high: Double
    let stepsRemoved: Int
}

struct FullIn: Decodable {
    let days: Double
    let earliest: Double
    let latest: Double?
}

struct Step: Decodable {
    let ts: Int64
    let freed: Int64
}

struct Arrival: Decodable {
    let ts: Int64
    let bytes: Int64
}

struct History: Decodable {
    let samples: Int
    let first: Int64?
    let last: Int64?
    let intervalSecs: Int64
    let bytes: Int64
}

struct Episode: Decodable, Identifiable {
    let opened: Int64
    let updated: Int64
    let closed: Int64?
    let incident: Incident
    let sentence: String
    /// The fix in one line, as the command line prints it.
    let fixLine: String
    let fix: Fix
    /// The app a churning process works for, when it's an app's helper
    /// ("Safari" for com.apple.WebKit.Networking).
    let app: String?

    /// Stable across refreshes: one per episode.
    var id: String { "\(incident.kind.rawValue):\(incident.subject):\(opened)" }

    /// "target keeps growing", "Safari keeps rewriting the disk".
    var title: String {
        switch incident.kind {
        case .growth: "\(URL(fileURLWithPath: incident.subject).lastPathComponent) keeps growing"
        case .churn: "\(app ?? incident.subject) keeps rewriting the disk"
        }
    }
}

struct Incident: Decodable {
    enum Kind: String, Decodable {
        case growth, churn
    }

    let kind: Kind
    /// The owner folder (growth) or the process name (churn).
    let subject: String
    let amount: Int64
    let perDay: Double
    let size: Int64?
    let fullInDays: Double?
}

/// How to stop something coming back.
enum Fix: Decodable {
    /// A tweak from the catalogue, by id.
    case tweak(String)
    /// A command to run once (copied, never run by the app), and a note.
    case run(command: String, note: String)
    /// What to change in an app or in System Settings.
    case steps(String)
    /// Nothing worth doing: it refills by itself.
    case leave(String)

    private enum Keys: String, CodingKey { case kind, detail }
    private struct Command: Decodable {
        let command: String
        let note: String
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Keys.self)
        let kind = try c.decode(String.self, forKey: .kind)
        switch kind {
        case "tweak": self = .tweak(try c.decode(String.self, forKey: .detail))
        case "run":
            let r = try c.decode(Command.self, forKey: .detail)
            self = .run(command: r.command, note: r.note)
        case "steps": self = .steps(try c.decode(String.self, forKey: .detail))
        case "leave": self = .leave(try c.decode(String.self, forKey: .detail))
        default:
            throw DecodingError.dataCorruptedError(forKey: .kind, in: c,
                                                   debugDescription: "unknown fix \(kind)")
        }
    }
}

struct AppliedTweak: Decodable, Identifiable {
    let id: String
    let title: String
    let status: String
    let applied: Int64
    /// Undone since, by an update or the app itself.
    let undone: Bool
}

struct Access: Decodable {
    let checked: Int64
    /// Whether the watcher has Full Disk Access; nil when it couldn't tell.
    let fullDiskAccess: Bool?
}

struct WatcherCost: Decodable {
    let samples: Int64
    let intervalSecs: Int64
    let cpuUsPerSample: Int64
    let writtenPerSample: Int64
    let cpuUsPerHour: Int64
    let writtenPerHour: Int64
    let ownersEverySecs: Int64
    let periods: Int64
    let cpuUsPerPeriod: Int64?
    let catchUpCpuUs: Int64
}

// MARK: - mdif forecast --json --series

struct ForecastDetail: Decodable {
    let forecast: Forecast?
    let writes24h: [Writer]
    let growth24h: [Growth]
    /// The 25 that grew and the 10 that shrank the most over 3 days and 2
    /// weeks (`nil` from an older `mdif`).
    let growth3d: [Growth]?
    let growth14d: [Growth]?
    /// [unix seconds, bytes free]; only with `--series`.
    let series: [[Int64]]?
    /// What was written each local day, oldest first; only with `--series`.
    let writesDaily: [DayWrites]?
    /// What the disk was made of at each whole measurement, oldest first;
    /// only with `--series`.
    let strata: [Strata]?

    struct Strata: Decodable {
        let ts: Int64
        /// Bytes per layer: a kind's id, "trash", or "unseen" (what the scan
        /// couldn't see: snapshots, macOS's own files, metadata).
        let layers: [String: Int64]
    }

    struct DayWrites: Decodable {
        /// The day's local midnight, unix seconds.
        let day: Int64
        let bytes: Int64
        /// The biggest writers that day.
        let top: [Writer]
    }

    struct Writer: Decodable {
        let name: String
        let bytes: Int64
    }

    struct Growth: Decodable {
        let owner: String
        let bytes: Int64
    }

}

// MARK: - mdif suggest --json

struct Suggestion: Decodable {
    let cause: String
    let fix: Fix
    let space: Int64?
    let perDay: Double?
    /// The file or folder it would delete, clean or move, which can be kept
    /// instead.
    let path: String?
    /// A clean-up the app can do itself (`mdif clean`).
    let action: CleanAction?
    /// A short title, when the cause doesn't start with one.
    let title: String?
    /// What doing it frees at once, an estimate.
    let frees: Int64?
    /// How long ago its files were changed, on average by size, in seconds.
    let age: Int64?
    /// Its folder's size at the end of each of the last days, oldest first,
    /// the last one now; empty when unknown.
    let sizes: [SizePoint]?

    struct SizePoint: Decodable {
        let ts: Int64
        let bytes: Int64
    }
}

/// What a suggestion's button does: `mdif clean` on its paths, which checks
/// each again first.
struct CleanAction: Decodable {
    /// "worktrees", "trash", "delete", "empty", "restart" or "snapshots".
    let kind: String
    /// "Move to Trash", "Remove 8 Worktrees".
    let label: String
    let paths: [String]
    /// Asked first, when it can't be undone.
    let confirm: String?
    /// When it was done, while the watcher waits to see whether it worked.
    let done: Int64?

    var symbol: String {
        switch kind {
        case "worktrees": "arrow.triangle.branch"
        case "restart": "arrow.clockwise"
        default: "trash"
        }
    }

    /// The same done by hand in Terminal, for the App Store edition, which
    /// may only read the disk: plain macOS commands, paths quoted. Nil where
    /// there's none to give (a helper's restart: no process name goes into
    /// a command).
    var shellCommand: String? {
        let q = paths.map(Self.quote).joined(separator: " ")
        switch kind {
        case "delete":
            return "rm -rf \(q)"
        case "empty":
            // What's in each, not the folder, and no link followed.
            return paths.map { "find \(Self.quote($0)) -mindepth 1 -delete" }.joined(separator: " && ")
        case "empty-trash":
            return #"osascript -e 'tell application "Finder" to empty trash'"#
        case "trash":
            return "mv -n \(q) ~/.Trash/"
        case "finder-trash":
            // Finder asks for the password these need.
            return #"osascript -e 'on run argv' -e 'repeat with p in argv' -e 'tell application "Finder" to delete (POSIX file (p as text) as alias)' -e 'end repeat' -e 'end run' "# + q
        default:
            return nil
        }
    }

    static func quote(_ s: String) -> String {
        "'" + s.replacingOccurrences(of: "'", with: #"'\''"#) + "'"
    }

    /// While it runs.
    var busy: String {
        switch kind {
        case "trash": "Moving to the Trash…"
        case "delete": "Deleting…"
        case "empty": "Emptying…"
        case "empty-trash": "Emptying the Trash…"
        case "finder-trash": "Waiting for Finder…"
        case "restart": "Restarting…"
        case "snapshots": "Deleting the snapshots…"
        default: "Removing…"
        }
    }
}

// MARK: - mdif clean --json

struct Cleaned: Decodable {
    struct Item: Decodable {
        let path: String
        /// "removed", "trashed", "deleted", "emptied", "erased" (the Trash),
        /// "restarted" or "released" (a local snapshot).
        let how: String
        /// What went to the Trash, which still takes its space there.
        let size: Int64?
    }
    struct Refused: Decodable {
        let path: String
        let why: String
    }

    let done: [Item]
    let failed: [Refused]
    /// How much more the disk has free.
    let freed: Int64
    let inTrash: Int64
}

// MARK: - mdif keep --json

/// On the keep list: never suggested for deleting, cleaning or moving.
struct Kept: Decodable, Identifiable {
    /// As written in the list ("~/Movies/Clip*").
    let entry: String
    let path: String
    /// How many files or folders it names now; nil when that can't be told.
    let present: Int?

    var id: String { entry }
}

// MARK: - mdif tweaks --json

struct TweakInfo: Decodable, Identifiable {
    let id: String
    let title: String
    /// "applied", "not applied", "partly applied", "waiting for approval", "absent".
    let status: String
    let why: String
    let cost: String
    let size: Int64?
    let applied: Int64?
    let restartPending: Bool?

    var isApplied: Bool { status == "applied" }
    var isAbsent: Bool { status == "absent" }
}

/// `mdif tweaks apply <id> [--dry-run] --json`.
struct TweakApplied: Decodable {
    let id: String
    let title: String
    let why: String
    let cost: String
    let dryRun: Bool
    /// "setting: before → after".
    let changes: [String]
    /// The configuration profiles it hands to System Settings, as they are.
    let profiles: [ProfileInfo]
    /// What to expect now; nil for a dry run or when nothing changed.
    let next: String?
}

/// A configuration profile a tweak installs: everything it holds, so it can
/// be shown before anyone is asked to install it.
struct ProfileInfo: Decodable {
    /// What System Settings lists it as.
    let name: String
    /// What System Settings shows with it.
    let description: String
    /// The app it sets ("Google Chrome") and that app's preferences domain.
    let app: String
    let domain: String
    let settings: [ProfileSetting]
    /// Where it's saved, "~/…".
    let file: String
    /// The file itself, an XML property list.
    let contents: String
    /// Where System Settings installs it ("General → Device Management").
    let place: String
}

struct ProfileSetting: Decodable {
    let key: String
    /// As JSON writes it: `1`, `true`, `"text"`.
    let value: String
}

/// `mdif tweaks revert <id> --json`.
struct TweakReverted: Decodable {
    let id: String
    let title: String
    let restored: [String]
    /// Changed since the tweak, so left as they are.
    let kept: [String]
    let restart: Bool
}

// MARK: - mdif map --json

struct DiskMap: Decodable {
    let root: String
    let container: Container
    let volume: Volume
    let snapshots: [String]
    let scan: Scan

    struct Container: Decodable {
        let reference: String
        let capacity: Int64
        let free: Int64
        let volumes: [Volume]

        private enum CodingKeys: String, CodingKey {
            case reference = "ContainerReference"
            case capacity = "CapacityCeiling"
            case free = "CapacityFree"
            case volumes = "Volumes"
        }
    }

    struct Volume: Decodable {
        let name: String
        let roles: [String]
        let inUse: Int64

        var label: String { roles.first ?? name }

        private enum CodingKeys: String, CodingKey {
            case name = "Name"
            case roles = "Roles"
            case inUse = "CapacityInUse"
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            roles = try c.decodeIfPresent([String].self, forKey: .roles) ?? []
            inUse = try c.decode(Int64.self, forKey: .inUse)
        }
    }

    struct Scan: Decodable {
        let root: Node
        /// Every block once, however many clones or snapshots share it.
        let real: Int64
        let sharedOnce: Int64
        let sharedUncertain: Int64
    }

    /// Space in use on the volume that the scan didn't find: root-only files,
    /// file-system metadata, blocks only snapshots hold.
    var unmeasured: Int64 { volume.inUse - scan.real }
}

final class Node: Decodable, Identifiable {
    let path: String
    let totals: Totals
    /// Largest first; nil for a leaf, so an outline shows no disclosure arrow.
    let children: [Node]?

    var id: String { path }

    private enum CodingKeys: String, CodingKey { case path, totals, children }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        path = try c.decode(String.self, forKey: .path)
        totals = try c.decode(Totals.self, forKey: .totals)
        let kids = try c.decode([Node].self, forKey: .children)
        children = kids.isEmpty ? nil : kids
    }
}

struct Totals: Decodable {
    let allocated: Int64
    /// Freed if everything here were deleted.
    let privateBytes: Int64
    let files: Int64
    let datalessFiles: Int64
    let datalessLogical: Int64
    let purgeable: Int64
    let unreadableDirs: Int64

    private enum CodingKeys: String, CodingKey {
        case allocated, files, datalessFiles, datalessLogical, purgeable, unreadableDirs
        case privateBytes = "private"
    }
}

// MARK: - mdif space --json

/// Where the space goes: every kind of thing on the disk, biggest first,
/// with what may not be needed marked. Measured with a whole-disk scan,
/// which `mdif` caches.
struct SpaceReport: Decodable {
    let measured: Int64
    let tookSecs: Double
    let volume: Volume
    /// Space that returns without deleting anything.
    let comesBack: [ComesBack]
    let kinds: [Kind]

    struct Volume: Decodable {
        let total: Int64
        let used: Int64
        let free: Int64
        /// What macOS frees by itself when it needs room (Finder counts it).
        let purgeable: Int64
    }

    struct ComesBack: Decodable {
        /// "purgeable", "swap", "update" (a macOS update waiting for a
        /// restart), "trash".
        let what: String
        let bytes: Int64
        /// "after a restart".
        let how: String
        /// The kind that counts it, when known ("other", "macos").
        let kind: String?
    }

    struct Kind: Decodable, Identifiable {
        let id: String
        let title: String
        let bytes: Int64
        /// What deleting all of it would free.
        let frees: Int64
        /// In items marked unused, cold, build output or cache, not kept.
        let reclaimable: Int64
        let items: [Item]
    }

    struct Item: Decodable, Identifiable {
        let path: String
        let name: String
        let bytes: Int64
        let frees: Int64
        /// How long ago its files were changed, on average by size.
        let age: Int64?
        /// Apps: when it was last opened.
        let lastUsed: Int64?
        /// "unused", "cold", "build", "cache", "kept", "regrows".
        let flags: [String]
        let note: String?
        let action: CleanAction?
        /// What an app keeps in it, and where to make it smaller.
        let about: About?

        var id: String { path }
        func has(_ flag: String) -> Bool { flags.contains(flag) }
    }

    /// From an app's handler in `mdif`: the biggest folder in an item it
    /// knows.
    struct About: Decodable {
        let path: String
        let bytes: Int64
        /// "Signal's attachments".
        let title: String
        let what: String
        let how: String
        let open: Open?
        /// Emptying what in it only caches (Teams', Slack's).
        let action: CleanAction?

        /// An app (bundle id) or a page of System Settings.
        struct Open: Decodable {
            let label: String
            let app: String?
            let url: String?
        }
    }

    /// Big files of yours nothing has opened or changed for months (Cruft);
    /// absent from a scan made before it was measured.
    let oldFiles: [OldFile]?

    struct OldFile: Decodable, Identifiable {
        let path: String
        let name: String
        let bytes: Int64
        let frees: Int64
        /// Last opened, as far as macOS records it.
        let accessed: Int64?
        let modified: Int64?
        let kept: Bool
        /// Move to Trash, as `mdif clean` would do it now.
        let action: CleanAction?

        var id: String { path }
        /// When it was last opened or changed, whichever is later.
        var lastUsed: Int64 { max(accessed ?? 0, modified ?? 0) }
    }

    var comesBackTotal: Int64 { comesBack.reduce(0) { $0 + $1.bytes } }

    /// What's in the Trash: free once it's emptied.
    var trash: Int64 { comesBack.first { $0.what == "trash" }?.bytes ?? 0 }

    /// Purgeable, swap and a waiting macOS update: counted as used, freed
    /// without a clean-up (not the Trash, which someone empties).
    var byItself: [ComesBack] { comesBack.filter { $0.what != "trash" } }
}

// MARK: - mdif tidy --json

/// Clean-ups the watcher does by itself once a day, when they're on.
struct TidyList: Decodable {
    let tidies: [Tidy]
}

struct Tidy: Decodable, Identifiable {
    /// "simulators", "builds", "node-modules", "incremental", "caches".
    let id: String
    let title: String
    let detail: String
    let on: Bool
    /// False where it can't run (no Xcode for simulators), with why.
    let available: Bool
    let unavailableWhy: String?
    let last: Last?

    struct Last: Decodable {
        let at: Int64
        let freed: Int64
        let done: Int
        let failed: Int
        /// "Emptied 4 caches", "Nothing to tidy".
        let note: String?
    }
}

/// `mdif tidy run --json`.
struct TidyRun: Decodable {
    let dryRun: Bool
    let freed: Int64
    let done: [Done]
    let failed: [Failed]

    struct Done: Decodable {
        let id: String
        let path: String
        let what: String
        let bytes: Int64?
    }

    struct Failed: Decodable {
        let id: String
        let path: String
        let why: String
    }
}

// MARK: - mdif alerts --json

/// What raises a notification: set in Settings, kept in the history.
struct AlertSettings: Decodable {
    /// Below this much free space; nil for the automatic level.
    let lowFree: Int64?
    /// What the automatic level is on this disk.
    let lowFreeAuto: Int64
    /// Warn when the disk is likely full within this many days: 1, 3 or 14.
    let fullWithinDays: Int
    /// How much a folder grows before it's a runaway filler.
    let growthMin: Int64
    /// How much a process writes in an hour before it wears the disk.
    let churnPerHour: Int64
    let notifyForecast: Bool
    let notifyGrowth: Bool
    let notifyChurn: Bool
}
