// Updates, via Sparkle: the full edition only (the App Store updates its own
// apps). As in Idlewild.
//
// The trust anchor isn't the feed (the appcast attached to the latest GitHub
// release): an archive installs only if it carries an Ed25519 signature made
// with the private key that only the maintainer and the release job hold, and
// the app inside is signed by the same team as the running one. Whoever takes
// over the feed can't get anything installed.
//
// The feed URL, the public key and the daily check live in Info.plist, and
// Sparkle keeps the user's choice about automatic checks in the defaults:
// there's no second copy of that setting here.

#if !APP_STORE
import AppKit
import Combine
import Sparkle

@MainActor
final class Updater: ObservableObject {
    /// False while a check or an installation runs: the menu follows it, so
    /// "Check for Updates" can't be started twice.
    @Published private(set) var canCheckForUpdates = false

    private let reminders: UpdateReminders
    private let controller: SPUStandardUpdaterController

    init(settings: AppSettings) {
        reminders = UpdateReminders(settings: settings)
        controller = SPUStandardUpdaterController(startingUpdater: true, updaterDelegate: nil,
                                                  userDriverDelegate: reminders)
        controller.updater.publisher(for: \.canCheckForUpdates)
            .assign(to: &$canCheckForUpdates)
    }

    /// What the Settings switch reads and writes, so it and Sparkle's own
    /// alerts can't disagree.
    var automaticallyChecksForUpdates: Bool {
        get { controller.updater.automaticallyChecksForUpdates }
        set { controller.updater.automaticallyChecksForUpdates = newValue }
    }

    func checkForUpdates() {
        controller.checkForUpdates(nil)
    }
}

/// Gentle reminders, which Sparkle asks a menu bar app for. Such an app may
/// not take the focus, so a scheduled update alert would sit behind whatever
/// is in front, unseen. While an update is shown the app joins the Dock (so
/// the alert can come forward and Cmd-Tab reaches it) and a notification
/// says so; both go when the alert has been seen or the session ends.
final class UpdateReminders: NSObject, SPUStandardUserDriverDelegate {
    private let settings: AppSettings

    init(settings: AppSettings) {
        self.settings = settings
        super.init()
    }

    var supportsGentleScheduledUpdateReminders: Bool { true }

    func standardUserDriverWillHandleShowingUpdate(_ handleShowingUpdate: Bool,
                                                   forUpdate update: SUAppcastItem,
                                                   state: SPUUserUpdateState) {
        let version = update.displayVersionString
        let userInitiated = state.userInitiated
        // Sparkle calls these on the main thread; the protocol can't say so.
        MainActor.assumeIsolated {
            _ = NSApp.setActivationPolicy(.regular)
            // A check the user asked for already has their attention.
            guard !userInitiated, settings.notificationsEnabled else { return }
            Notifier.update(version: version)
        }
    }

    func standardUserDriverDidReceiveUserAttention(forUpdate update: SUAppcastItem) {
        MainActor.assumeIsolated {
            Notifier.withdraw([Notifier.updateID])
        }
    }

    func standardUserDriverWillFinishUpdateSession() {
        MainActor.assumeIsolated {
            _ = NSApp.setActivationPolicy(.accessory)
        }
    }
}
#endif
