// Applying and undoing a tweak: what it changes and what you give up first,
// from a dry run, then what happened, in the window (Notices.swift) rather
// than in another alert. A tweak that installs a configuration
// profile shows everything the profile holds first (ProfileReview.swift). The
// Rust core records every value it replaces before changing anything, so Undo
// puts back exactly what was there. Not in the App Store edition (see
// Edition.swift).

#if !APP_STORE
import AppKit

@MainActor
enum TweakFlow {
    enum Outcome {
        /// Cancelled, or nothing to change.
        case unchanged
        case done
        /// It failed; what it had changed was put back.
        case failed
    }

    static func apply(_ id: String) async -> Outcome {
        let plan: TweakApplied
        do {
            plan = try await Core.json(TweakApplied.self, ["tweaks", "apply", id, "--dry-run"])
        } catch {
            Notices.shared.post(ok: false, "Can't apply this tweak", error.localizedDescription)
            return .unchanged
        }
        guard !plan.changes.isEmpty else {
            Notices.shared.post(ok: true, plan.title, "Already applied: there is nothing to change.")
            return .unchanged
        }
        if let profile = plan.profiles.first {
            guard ProfileReviewPanel.run(plan, profile) else { return .unchanged }
        } else {
            let text = """
                \(plan.why)

                You give up: \(plan.cost)

                It changes:
                \(plan.changes.map { "• \($0)" }.joined(separator: "\n"))

                MyDiskIsFull keeps what it replaces, so Undo puts it back.
                """
            guard ask(plan.title, text, button: "Apply") else { return .unchanged }
        }
        do {
            let done = try await Core.json(TweakApplied.self, ["tweaks", "apply", id])
            // The review said what to do in System Settings, which is open
            // now.
            if plan.profiles.isEmpty {
                Notices.shared.post(ok: true, done.title, done.next ?? "Done.")
            }
            return .done
        } catch {
            Notices.shared.post(ok: false, "The tweak wasn't applied", error.localizedDescription
                                + "\n\nAnything it had changed was put back.")
            return .failed
        }
    }

    static func revert(_ id: String, title: String) async -> Outcome {
        guard ask("Undo “\(title)”?",
                  "MyDiskIsFull puts back the settings it replaced. A setting changed since then is left as it is.",
                  button: "Undo") else { return .unchanged }
        do {
            let r = try await Core.json(TweakReverted.self, ["tweaks", "revert", id])
            var text = r.restored.isEmpty
                ? "Nothing to put back: its settings were already as they were."
                : "Put back:\n" + r.restored.map { "• \($0)" }.joined(separator: "\n")
            if !r.kept.isEmpty {
                text += "\n\nLeft as they are (changed since the tweak):\n"
                    + r.kept.map { "• \($0)" }.joined(separator: "\n")
            }
            if r.restart { text += "\n\nIt takes effect after a restart." }
            Notices.shared.post(ok: true, "Undone: \(r.title)", text)
            return .done
        } catch {
            Notices.shared.post(ok: false, "Couldn't undo it", error.localizedDescription)
            return .unchanged
        }
    }

    static func ask(_ title: String, _ text: String, button: String) -> Bool {
        let a = NSAlert()
        a.messageText = title
        a.informativeText = text
        a.addButton(withTitle: button)
        a.addButton(withTitle: "Cancel")
        NSApp.activate(ignoringOtherApps: true)
        return a.runModal() == .alertFirstButtonReturn
    }
}
#endif
