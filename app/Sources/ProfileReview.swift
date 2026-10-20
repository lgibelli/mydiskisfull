// Before a tweak hands a configuration profile to System Settings: what a
// profile is, everything this one holds and nothing else, what System
// Settings and the app will say, and how to undo it, with the file itself one
// click away. Profiles are what companies use to manage Macs, so nobody
// should be asked to install ours on trust. Not in the App Store edition (see
// Edition.swift).

#if !APP_STORE
import AppKit
import SwiftUI

enum ProfileReviewPanel {
    /// Shows the review in a modal panel; true if they chose to go on.
    @MainActor
    static func run(_ plan: TweakApplied, _ profile: ProfileInfo) -> Bool {
        var go = false
        let panel = NSPanel(contentRect: .zero, styleMask: [.titled],
                            backing: .buffered, defer: false)
        let host = NSHostingController(rootView: ProfileReview(plan: plan, profile: profile) { yes in
            go = yes
            NSApp.stopModal()
        })
        // Grows with the file when it's shown.
        host.sizingOptions = .preferredContentSize
        panel.contentViewController = host
        panel.title = "Configuration Profile"
        panel.setContentSize(host.view.fittingSize)
        panel.center()
        NSApp.activate(ignoringOtherApps: true)
        NSApp.runModal(for: panel)
        panel.orderOut(nil)
        return go
    }
}

private struct ProfileReview: View {
    let plan: TweakApplied
    let profile: ProfileInfo
    let done: @MainActor (Bool) -> Void
    @State private var showFile = false

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 14) {
                IconTile(symbol: "gearshape.2.fill", color: .blue, size: 42)
                VStack(alignment: .leading, spacing: 4) {
                    Text(plan.title).font(.title3.weight(.semibold))
                    Text("\(plan.why) You give up: \(plan.cost)")
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }

            Text("\(profile.app) takes this setting only from a configuration profile: a file of settings, the kind companies use to set up their Macs. MyDiskIsFull makes this one here, on your Mac, and you install it yourself in System Settings. Nothing changes until you do.")
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)

            contents
            next

            Text("To undo it, turn the switch off on the Fixes page, or remove the profile in System Settings.")
                .font(.callout).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)

            HStack {
                Spacer()
                Button("Cancel") { done(false) }
                    .keyboardShortcut(.cancelAction)
                Button("Continue to System Settings") { done(true) }
                    .keyboardShortcut(.defaultAction)
            }
            .controlSize(.large)
        }
        .padding(24)
        .frame(width: 540)
        .background(Theme.canvas)
    }

    /// The profile as it is: its name, the settings it holds, what it
    /// doesn't hold, and the file.
    private var contents: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("WHAT'S IN IT")
                .font(.caption2.weight(.semibold)).tracking(0.6)
                .foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 2) {
                Text(profile.name).font(.headline)
                Text("For \(profile.app) (\(profile.domain)), your user only")
                    .font(.caption).foregroundStyle(.secondary)
            }
            VStack(alignment: .leading, spacing: 4) {
                ForEach(profile.settings, id: \.key) { s in
                    Text("\(s.key) = \(s.value)")
                        .font(.system(.callout, design: .monospaced))
                        .textSelection(.enabled)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8))
            Text(profile.settings.count == 1 ? "That one setting is all it holds:" : "Those settings are all it holds:")
                .font(.callout)
            VStack(alignment: .leading, spacing: 5) {
                Absent(text: "no certificates, accounts or passwords")
                Absent(text: "no VPN or network settings")
                Absent(text: "no remote management: nobody else can change this Mac through it")
                Absent(text: "nothing for other apps or other users")
            }
            Button {
                withAnimation { showFile.toggle() }
            } label: {
                HStack(spacing: 4) {
                    Image(systemName: showFile ? "chevron.down" : "chevron.right")
                        .font(.caption.weight(.semibold))
                    Text(showFile ? "Hide the file" : "Show the file itself")
                }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Color.accentColor)
            .font(.callout.weight(.medium))
            if showFile {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Saved as \(profile.file)")
                        .font(.caption).foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    ScrollView([.horizontal, .vertical]) {
                        Text(profile.contents)
                            .font(.system(size: 11, design: .monospaced))
                            .textSelection(.enabled)
                            .fixedSize()
                            .padding(10)
                    }
                    .frame(height: 220)
                    .background(Color.primary.opacity(0.05), in: RoundedRectangle(cornerRadius: 8))
                }
            }
        }
        .card(padding: 16)
    }

    /// What System Settings will show, in order, and what Chrome will say.
    private var next: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("WHAT HAPPENS NEXT")
                .font(.caption2.weight(.semibold)).tracking(0.6)
                .foregroundStyle(.secondary)
            NextStep(n: 1, text: "System Settings opens at \(profile.place).")
            NextStep(n: 2, text: "Double-click “\(profile.name)”: it shows what the profile holds, as above. Click Install and enter your password. If macOS says the profile is unsigned or unverified, that's because MyDiskIsFull made it on this Mac rather than a company signing it.")
            NextStep(n: 3, text: "Quit and reopen \(profile.app). It will say it's “managed by your organization”: that's this one setting, not someone managing your Mac.")
        }
    }
}

private struct Absent: View {
    let text: String

    var body: some View {
        Label {
            Text(text).font(.callout).fixedSize(horizontal: false, vertical: true)
        } icon: {
            Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
        }
    }
}

private struct NextStep: View {
    let n: Int
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text("\(n)")
                .font(.caption.weight(.bold))
                .foregroundStyle(.white)
                .frame(width: 20, height: 20)
                .background(Color.accentColor, in: Circle())
            Text(text)
                .font(.callout)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
#endif
