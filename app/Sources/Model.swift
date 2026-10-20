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
    /// The longest window with enough data.
    let rate: Rate?
    /// The shortest window, when it differs from `rate`.
    let recent: Rate?
    let fullIn: FullIn?
    /// When the recent rate fills the disk much sooner.
    let recentFullIn: FullIn?
    let steps: [Step]
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

    /// Stable across refreshes: one per episode.
    var id: String { "\(incident.kind.rawValue):\(incident.subject):\(opened)" }
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
    /// [unix seconds, bytes free]; only with `--series`.
    let series: [[Int64]]?

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
