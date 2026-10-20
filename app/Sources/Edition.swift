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

/// Where a feature would be: what's missing, why, and the link. Compact
/// where it would repeat (under each suggestion): the title and the link.
struct FullVersionNote: View {
    let missing: Missing
    var title: String?
    var compact = false

    var body: some View {
        if compact {
            HStack(spacing: 8) {
                Image(systemName: "arrow.down.app.fill").foregroundStyle(.blue)
                Text(title ?? missing.title).font(.callout.weight(.medium))
                Text("· in the full version").font(.callout).foregroundStyle(.secondary)
                Button("Get the Full Version…") { Edition.openFullVersion() }
                    .buttonStyle(.link)
                    .font(.callout)
            }
            .help(missing.why + " " + Missing.pitch)
        } else {
            note
        }
    }

    private var note: some View {
        HStack(alignment: .top, spacing: 12) {
            IconTile(symbol: "arrow.down.app.fill", color: .blue, size: 30)
            VStack(alignment: .leading, spacing: 6) {
                Text(title ?? missing.title).font(.headline)
                Text(missing.why + " " + Missing.pitch)
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Get the Full Version…") { Edition.openFullVersion() }
                    .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
#endif
