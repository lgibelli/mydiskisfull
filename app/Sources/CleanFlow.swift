// A suggestion's clean-up, done by the app: `mdif clean` on its paths, which
// checks each again first and refuses what changed (a worktree with new
// work, something you keep, an app still running). What can't be undone is
// asked first; what goes to the Trash isn't, and says its space comes back
// once the Trash is emptied. Not in the App Store edition, which may only
// read the disk (see Edition.swift).

#if !APP_STORE
import AppKit

@MainActor
enum CleanFlow {
    /// Whether to go ahead: what can't be undone is asked first.
    static func confirmed(_ a: CleanAction) -> Bool {
        guard let confirm = a.confirm else { return true }
        return TweakFlow.ask("\(a.label)?", confirm, button: a.label)
    }

    /// Once `confirmed`; returns whether anything was done.
    static func run(_ a: CleanAction) async -> Bool {
        let r: Cleaned
        do {
            // Removing build output can be hundreds of thousands of files.
            r = try await Core.json(Cleaned.self, ["clean"] + a.paths, timeout: 1800)
        } catch {
            TweakFlow.show(.warning, "Nothing was cleaned", error.localizedDescription)
            return false
        }
        var lines: [String] = []
        let removed = r.done.filter { $0.how == "removed" }.count
        if removed > 0 {
            lines.append(removed == 1 ? "Removed the worktree; its branch stays."
                                      : "Removed \(removed) worktrees; their branches stay.")
        }
        if r.done.contains(where: { $0.how == "restarted" }) {
            lines.append("Restarted: a fresh one starts when it's needed. If the writes come back, quit and reopen the app.")
        }
        let deleted = r.done.filter { $0.how == "deleted" }.count
        if deleted > 0 {
            lines.append("Deleted. The next build makes it again.")
        }
        let released = r.done.filter { $0.how == "released" }.count
        if released > 0 {
            lines.append(released == 1 ? "Deleted the local snapshot. Your backups on the backup disk are as they were."
                                       : "Deleted \(released) local snapshots. Your backups on the backup disk are as they were.")
        }
        if r.done.contains(where: { $0.how == "erased" }) {
            lines.append("Emptied the Trash.")
        }
        let emptied = r.done.filter { $0.how == "emptied" }.count
        if emptied > 0 {
            lines.append(emptied == 1 ? "Emptied. The app makes what it needs again."
                                      : "Emptied \(emptied) caches. Apps make what they need again.")
        }
        if r.done.contains(where: { $0.how == "trashed" }) {
            lines.append(r.inTrash > 0
                ? "Moved to the Trash: its \(Format.bytes(r.inTrash)) comes back once you empty the Trash."
                : "Moved to the Trash: its space comes back once you empty the Trash.")
        }
        if r.freed > 0 { lines.append("The disk has \(Format.bytes(r.freed)) more free.") }
        if !r.failed.isEmpty {
            lines.append("Not done:\n" + r.failed.map { "• \($0.why)" }.joined(separator: "\n"))
        }
        TweakFlow.show(r.done.isEmpty ? .warning : .informational,
                       r.done.isEmpty ? "Nothing was cleaned" : "Done",
                       lines.joined(separator: "\n\n"))
        return !r.done.isEmpty
    }
}
#endif
