// Native notifications: a runaway filler, the forecast getting close, a tweak
// that something undid. Each episode is announced once, and taken back when
// it is over so a Focus mode doesn't deliver old news.

import AppKit
import UserNotifications

enum Notifier {
    @MainActor static var authorizationProblem: String?

    enum Category: String {
        case incidentTweak = "incident.tweak"
        case incidentCommand = "incident.command"
        case incident = "incident"
        case alert = "alert"
        case undone = "undone"
    }

    enum Action: String {
        case applyTweak = "APPLY_TWEAK"
        case copyCommand = "COPY_COMMAND"
    }

    static func configure(delegate: UNUserNotificationCenterDelegate) {
        let c = UNUserNotificationCenter.current()
        c.delegate = delegate
        let apply = UNNotificationAction(identifier: Action.applyTweak.rawValue,
                                         title: "Apply Fix…", options: [.foreground])
        let again = UNNotificationAction(identifier: Action.applyTweak.rawValue,
                                         title: "Apply Again…", options: [.foreground])
        let copy = UNNotificationAction(identifier: Action.copyCommand.rawValue,
                                        title: "Copy Command", options: [])
        c.setNotificationCategories([
            UNNotificationCategory(identifier: Category.incidentTweak.rawValue, actions: [apply],
                                   intentIdentifiers: []),
            UNNotificationCategory(identifier: Category.incidentCommand.rawValue, actions: [copy],
                                   intentIdentifiers: []),
            UNNotificationCategory(identifier: Category.incident.rawValue, actions: [],
                                   intentIdentifiers: []),
            UNNotificationCategory(identifier: Category.alert.rawValue, actions: [],
                                   intentIdentifiers: []),
            UNNotificationCategory(identifier: Category.undone.rawValue, actions: [again],
                                   intentIdentifiers: []),
        ])
    }

    /// Asks once; macOS remembers the answer.
    static func requestAuthorization() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { granted, error in
            Task { @MainActor in
                if let error {
                    log.error("notification authorization: \(error.localizedDescription, privacy: .public)")
                    authorizationProblem = error.localizedDescription
                } else {
                    log.notice("notification authorization granted=\(granted)")
                    authorizationProblem = granted ? nil : "Notifications are turned off for MyDiskIsFull."
                }
            }
        }
    }

    @MainActor static func openSettings() {
        if let u = URL(string: "x-apple.systempreferences:com.apple.preference.notifications") {
            NSWorkspace.shared.open(u)
        }
    }

    static func incident(_ e: Episode, tweakTitle: String?) {
        let n = UNMutableNotificationContent()
        let name = e.incident.kind == .growth
            ? URL(fileURLWithPath: e.incident.subject).lastPathComponent
            : e.incident.subject
        n.title = e.incident.kind == .growth ? "\(name) keeps growing" : "\(name) keeps rewriting the disk"
        n.body = e.sentence
        n.userInfo = ["episode": e.id]
        switch e.fix {
        case .tweak(let id):
            n.subtitle = "Fix: \(tweakTitle ?? e.fixLine)"
            n.categoryIdentifier = Category.incidentTweak.rawValue
            n.userInfo["tweak"] = id
        case .run(let command, _):
            n.subtitle = "Fix: run a command once"
            n.categoryIdentifier = Category.incidentCommand.rawValue
            n.userInfo["command"] = command
        case .steps, .leave:
            n.subtitle = e.fixLine
            n.categoryIdentifier = Category.incident.rawValue
        }
        n.sound = .default
        post(id: "incident-\(e.id)", n)
    }

    static func alert(_ s: Status) {
        guard let sentence = s.alertSentence else { return }
        let n = UNMutableNotificationContent()
        if s.alert == .low {
            n.title = "The disk is almost full"
        } else if let d = s.forecast?.fullIn?.days {
            n.title = "The disk is full in about \(Format.days(d))"
        } else {
            n.title = "The disk is filling up"
        }
        n.body = sentence
        n.categoryIdentifier = Category.alert.rawValue
        n.sound = .default
        post(id: "alert", n)
    }

    static func undone(_ t: AppliedTweak) {
        let n = UNMutableNotificationContent()
        n.title = "A tweak was undone"
        n.body = "\(t.title): something changed the setting back, probably an update. "
            + "Apply it again to keep the space from filling."
        n.categoryIdentifier = Category.undone.rawValue
        n.userInfo = ["tweak": t.id]
        post(id: "undone-\(t.id)", n)
    }

    static func withdraw(_ ids: [String]) {
        guard !ids.isEmpty else { return }
        let c = UNUserNotificationCenter.current()
        c.removePendingNotificationRequests(withIdentifiers: ids)
        c.removeDeliveredNotifications(withIdentifiers: ids)
    }

    private static func post(id: String, _ n: UNMutableNotificationContent) {
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: id, content: n, trigger: nil)) { error in
            if let error {
                log.error("notification \(id, privacy: .public) failed: \(error.localizedDescription, privacy: .public)")
            } else {
                log.notice("posted notification \(id, privacy: .public)")
            }
        }
    }
}
