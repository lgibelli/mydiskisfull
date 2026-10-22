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

    /// The app's name, as the menus and windows say it.
    static let name = appStore ? "MyDiskIsFull Lite" : "MyDiskIsFull"

    /// Where the full version is: its own page, with the download.
    static let fullVersion = URL(string: "https://salamacchine.it/apps/mydiskisfull/")!

    @MainActor static func openFullVersion() {
        NSWorkspace.shared.open(fullVersion)
    }
}

/// "Lite" on the App Store edition, which looks like the full one otherwise
/// (the two can run side by side); nothing on the full one.
struct EditionBadge: View {
    var body: some View {
        if Edition.appStore {
            Text("LITE")
                .font(.system(size: 9.5, weight: .heavy, design: .rounded))
                .tracking(0.8)
                .foregroundStyle(.white)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(LinearGradient(colors: Theme.brand.reversed(), startPoint: .leading, endPoint: .trailing),
                            in: Capsule())
                .help("\(Edition.name), the App Store edition")
                .accessibilityLabel("Lite edition")
        }
    }
}

#if APP_STORE
/// What the App Store edition leaves out, and why.
enum Missing {
    case tweaks, writers, cleanups

    var title: String {
        switch self {
        case .tweaks: "One-click tweaks are in the full version"
        case .writers: "Which apps write the most is in the full version"
        case .cleanups: "One-click clean-ups are in the full version"
        }
    }

    var why: String {
        switch self {
        case .tweaks: "App Store rules don't let an app change other apps' settings, so this version can't do it for you."
        case .writers: "App Store rules don't let an app see what other apps write to the disk."
        case .cleanups: "App Store rules let this version read your disk, not change it, so it can't remove anything for you."
        }
    }

    static let pitch = "The full version is completely free (no ads) and open source."
}

/// Where a feature would be: locked, with what's missing; asked about, why
/// the App Store version can't, and the free full version. Compact where it
/// would repeat (under each suggestion). It never says "unlock": App Store
/// rules forbid unlocking features any way but in-app purchase, and this is
/// another app, not an unlock.
struct FullVersionNote: View {
    let missing: Missing
    var title: String?
    var compact = false
    @State private var explaining = false

    var body: some View {
        Group {
            if compact {
                Button {
                    explaining = true
                } label: {
                    HStack(spacing: 7) {
                        Image(systemName: "lock.fill").foregroundStyle(.secondary)
                        Text(title ?? missing.title).font(.callout.weight(.medium)).foregroundStyle(.primary)
                        Text("FULL VERSION")
                            .font(.system(size: 9.5, weight: .heavy, design: .rounded))
                            .tracking(0.6)
                            .foregroundStyle(.white)
                            .padding(.horizontal, 6).padding(.vertical, 2)
                            .background(Color.secondary, in: Capsule())
                    }
                }
                .buttonStyle(.plain)
                .help(missing.why + " " + Missing.pitch)
            } else {
                note
            }
        }
        .sheet(isPresented: $explaining) { FullVersionSheet(missing: missing) }
    }

    private var note: some View {
        HStack(alignment: .top, spacing: 12) {
            IconTile(symbol: "lock.fill", color: .gray, size: 30)
            VStack(alignment: .leading, spacing: 6) {
                Text(title ?? missing.title).font(.headline)
                Text(missing.why + " " + Missing.pitch)
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Why, and How to Get It…") { explaining = true }
                    .padding(.top, 2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Why the App Store version can't do it, and the full version, which can:
/// free, signed and notarized.
struct FullVersionSheet: View {
    let missing: Missing
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            HStack(spacing: 12) {
                IconTile(symbol: "lock.open.fill", color: .blue, size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text(missing.title).font(.title3.weight(.semibold))
                    Text("MyDiskIsFull, the full version, does it for you.")
                        .font(.callout).foregroundStyle(.secondary)
                }
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("Why the App Store version can't").font(.headline)
                Text("Every app on the Mac App Store has to run in Apple's App Sandbox. It may read only the folders you let it see, and it can't delete files outside its own folder, change other apps' settings, or see what other apps write to the disk. That's what keeps App Store apps safe, and it's why \(Edition.name) can show you where your space goes and explain each fix, but can't do the fix for you.")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: 6) {
                Text("The full version").font(.headline)
                Text("It's completely free, with no ads, and open source, digitally signed with our Developer ID and notarized by Apple, so macOS opens it like any App Store app. It cleans up with one click, keeps the disk tidy by itself, applies the tweaks, and shows which apps wear the disk. You can keep both; your keep list stays with each.")
                    .font(.callout).foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer()
                Button("Not Now") { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Button("Download the Free Full Version") {
                    Edition.openFullVersion()
                    dismiss()
                }
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(22)
        .frame(width: 500)
    }
}
#endif
