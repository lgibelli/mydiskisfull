// The two editions. The full one, from GitHub, does everything. The App
// Store one (`app/build.sh --app-store`, which defines APP_STORE) must run in
// the App Sandbox, which keeps it from changing other apps' settings (the
// tweaks) and from seeing what other apps write to the disk. Where those
// would be, it says why and links the full version; their code isn't
// compiled into it at all.

import AppKit
import SwiftUI

enum Edition {
    #if APP_STORE
    static let appStore = true
    #else
    static let appStore = false
    #endif

    /// Where the full version is.
    static let fullVersion = URL(string: "https://github.com/lgibelli/mydiskisfull/releases/latest")!

    @MainActor static func openFullVersion() {
        NSWorkspace.shared.open(fullVersion)
    }
}

#if APP_STORE
/// What the App Store edition leaves out, and why.
enum Missing {
    case tweaks, writers

    var title: String {
        switch self {
        case .tweaks: "One-click tweaks are in the full version"
        case .writers: "Which apps write the most is in the full version"
        }
    }

    var why: String {
        switch self {
        case .tweaks: "App Store rules don't let an app change other apps' settings, so this version can't do it for you."
        case .writers: "App Store rules don't let an app see what other apps write to the disk."
        }
    }

    static let pitch = "The full version is free and open source."
}

/// The note in a menu: lines that don't wrap, and the link.
struct FullVersionItems: View {
    let missing: Missing

    var body: some View {
        Text(missing.title)
        Lines(missing.why + " " + Missing.pitch)
        Button("Get the Full Version…") { Edition.openFullVersion() }
    }
}

/// The note in a window.
struct FullVersionNote: View {
    let missing: Missing
    var title: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title ?? missing.title).font(.headline)
            Text(missing.why + " " + Missing.pitch)
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Button("Get the Full Version…") { Edition.openFullVersion() }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
#endif
