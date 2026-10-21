// Decodes what `mdif` printed with the app's own models: `kind=path` per
// argument. Built and run by app/check-contract.sh.

import Foundation

func check<T: Decodable>(_ type: T.Type, _ path: String) throws -> T {
    let data = try Data(contentsOf: URL(fileURLWithPath: path))
    return try Wire.decode(type, from: data)
}

@main
enum Contract {
    static func main() {
        var failed = false
        for arg in CommandLine.arguments.dropFirst() {
            let parts = arg.split(separator: "=", maxSplits: 1).map(String.init)
            guard parts.count == 2 else {
                print("usage: contract kind=path ..."); exit(2)
            }
            let (kind, path) = (parts[0], parts[1])
            do {
                let summary: String
                switch kind {
                case "status":
                    let s = try check(Status.self, path)
                    summary = "alert \(s.alert.rawValue), \(s.incidents.count) incidents, \(s.tweaks.count) tweaks"
                case "forecast":
                    let f = try check(ForecastDetail.self, path)
                    summary = "\(f.series?.count ?? 0) points"
                case "suggest":
                    summary = "\(try check([Suggestion].self, path).count) suggestions"
                case "clean":
                    let c = try check(Cleaned.self, path)
                    summary = "\(c.done.count) done, \(c.failed.count) refused"
                case "keep":
                    summary = "\(try check([Kept].self, path).count) kept"
                case "tweaks":
                    summary = "\(try check([TweakInfo].self, path).count) tweaks"
                case "apply":
                    summary = "\(try check(TweakApplied.self, path).changes.count) changes"
                case "revert":
                    summary = "\(try check(TweakReverted.self, path).restored.count) restored"
                case "space":
                    let r = try check(SpaceReport.self, path)
                    summary = "\(r.kinds.count) kinds, \(r.kinds.reduce(0) { $0 + $1.items.count }) items"
                case "tidy":
                    summary = "\(try check(TidyList.self, path).tidies.count) tidy-ups"
                case "alerts":
                    _ = try check(AlertSettings.self, path)
                    summary = "alert settings"
                case "tidyrun":
                    let r = try check(TidyRun.self, path)
                    summary = "\(r.done.count) done, \(r.failed.count) failed"
                case "map":
                    let m = try check(DiskMap.self, path)
                    summary = "\(m.scan.root.children?.count ?? 0) top folders"
                default:
                    print("unknown kind \(kind)"); exit(2)
                }
                print("ok    \(kind): \(summary)")
            } catch {
                failed = true
                print("FAIL  \(kind) (\(path)): \(error)")
            }
        }
        exit(failed ? 1 : 0)
    }
}
