// Numbers and dates the way the command line prints them.

import Foundation

enum Format {
    /// Decimal units, like Finder and `mdif`: "24.8 GB", "23 MB".
    static func bytes(_ n: Int64) -> String {
        let v = Double(n.magnitude)
        let sign = n < 0 ? "-" : ""
        if v >= 1e9 { return sign + String(format: "%.1f GB", v / 1e9) }
        if v >= 1e6 { return sign + String(format: "%.0f MB", v / 1e6) }
        if v >= 1e3 { return sign + String(format: "%.0f KB", v / 1e3) }
        return "\(sign)\(Int64(v)) B"
    }

    static func bytes(_ n: Double) -> String { bytes(Int64(n)) }

    /// "12 days", "5 hours", "3 months".
    static func days(_ d: Double) -> String {
        let (n, unit): (Double, String)
        if d < 1 { (n, unit) = (max(1, (d * 24).rounded()), "hour") }
        else if d < 100 { (n, unit) = (d.rounded(), "day") }
        else { (n, unit) = ((d / 30.4).rounded(), "month") }
        return n == 1 ? "1 \(unit)" : "\(Int(n)) \(unit)s"
    }

    /// The short form for the menu bar: "12d", "5h".
    static func daysShort(_ d: Double) -> String {
        d < 1 ? "\(max(1, Int((d * 24).rounded())))h" : "\(Int(d.rounded()))d"
    }

    /// The day `in_days` from `at`: "Oct 31", or "Tue 14:00" when close.
    static func date(at: Int64, inDays d: Double) -> String {
        let when = Date(timeIntervalSince1970: TimeInterval(at) + d * 86_400)
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate(d < 2 ? "EEE HH:mm" : (d > 300 ? "MMM yyyy" : "MMM d"))
        return f.string(from: when)
    }

    static func when(_ ts: Int64) -> String {
        let f = DateFormatter()
        f.setLocalizedDateFormatFromTemplate("MMM d HH:mm")
        return f.string(from: Date(timeIntervalSince1970: TimeInterval(ts)))
    }

    /// "4 min", "3 hours", "2 days".
    static func duration(_ secs: Int64) -> String {
        if secs >= 2 * 86_400 { return "\(secs / 86_400) days" }
        if secs >= 2 * 3_600 { return "\(secs / 3_600) hours" }
        if secs >= 120 { return "\(secs / 60) min" }
        return "\(max(secs, 0)) s"
    }

    /// "3-day" or "6-hour", for a trend window.
    static func window(_ secs: Int64) -> String {
        secs >= 86_400 ? "\(secs / 86_400)-day" : "\(secs / 3_600)-hour"
    }

    /// `/System/Volumes/Data/Users/me/x` as `~/x`.
    static func path(_ p: String) -> String {
        var p = p
        let data = "/System/Volumes/Data"
        if p == data { return "/" }
        if p.hasPrefix(data + "/") { p.removeFirst(data.count) }
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        if p == home { return "~" }
        if p.hasPrefix(home + "/") { return "~" + p.dropFirst(home.count) }
        return p
    }

    /// Splits `text` into lines of at most `width` characters, at spaces, for
    /// menu items, which never wrap by themselves.
    static func lines(_ text: String, width: Int = 64) -> [String] {
        var out: [String] = []
        var line = ""
        for word in text.split(separator: " ") {
            if !line.isEmpty && line.count + 1 + word.count > width {
                out.append(line)
                line = ""
            }
            line += line.isEmpty ? String(word) : " " + word
        }
        if !line.isEmpty { out.append(line) }
        return out
    }
}
