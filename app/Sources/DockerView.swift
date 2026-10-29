// Docker's clean-up: what `docker system prune` would remove, item by item,
// and the images no container uses, which only `-a` removes, asked from
// Docker when the sheet opens (`mdif docker`). Each has a checkbox, and the
// sheet is the confirmation. What's ticked goes through `mdif clean`, which
// asks Docker again first and removes each by id: only what you saw goes,
// and Docker refuses what's in use by then. Not in the App Store edition,
// which may only read the disk.

#if !APP_STORE
import AppKit
import SwiftUI

struct DockerReview: View {
    @ObservedObject var monitor: Monitor
    @Environment(\.dismiss) private var dismiss
    @State private var report: DockerReport?
    @State private var failed: String?
    @State private var chosen: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Clean Up Docker").font(.headline)
            if let r = report, r.running {
                lists(r)
            } else if let why = report?.why ?? failed {
                stopped(why)
            } else {
                HStack(spacing: 8) {
                    ProgressView().controlSize(.small)
                    Text("Asking Docker what it can remove…").foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, minHeight: 120)
            }
            buttons
        }
        .padding(20)
        .frame(width: 600)
        .task { await load() }
    }

    private func load() async {
        report = nil
        failed = nil
        do {
            let r = try await Core.json(DockerReport.self, ["docker"], timeout: 120)
            report = r
            // What prune would remove, to start; an image once you tick it.
            chosen = Set(r.pruned.map(\.name))
        } catch {
            failed = error.localizedDescription
        }
    }

    @ViewBuilder private func stopped(_ why: String) -> some View {
        Text(why)
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        HStack {
            if let app = NSWorkspace.shared.urlForApplication(withBundleIdentifier: "com.docker.docker") {
                Button("Open Docker Desktop") {
                    NSWorkspace.shared.openApplication(at: app, configuration: NSWorkspace.OpenConfiguration())
                }
            }
            Button("Look Again") { Task { await load() } }
        }
    }

    @ViewBuilder private func lists(_ r: DockerReport) -> some View {
        Text("Docker has no dry run, so this is what it says it would remove. Only what you tick goes, one at a time, and Docker refuses anything that's in use by then.")
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: false, vertical: true)
        List {
            Section {
                if r.pruned.isEmpty {
                    Text("Nothing: no stopped containers, untagged images, unused networks or build cache.")
                        .foregroundStyle(.secondary)
                } else {
                    ForEach(r.pruned) { row($0) }
                }
            } header: {
                Text("What docker system prune removes")
            }
            if !r.unused.isEmpty {
                Section {
                    ForEach(r.unused) { row($0) }
                } header: {
                    Text("Images no container uses, which docker system prune -a removes too")
                }
            }
        }
        .listStyle(.bordered(alternatesRowBackgrounds: true))
        .frame(height: 330)
        VStack(alignment: .leading, spacing: 4) {
            if !r.containers.isEmpty {
                Text("What a container wrote outside its volumes goes with it.")
            }
            if !r.unused.isEmpty {
                Text("Docker downloads or builds an image again when it needs it: one built here and never pushed is built again from its Dockerfile.")
            }
            if r.cacheShared > 0 {
                Text("The build cache shares \(Format.bytes(r.cacheShared)) with the images: that goes only with them.")
            }
            if r.volumes > 0 {
                Text("\(r.volumes == 1 ? "1 volume" : "\(r.volumes) volumes") no container uses \(r.volumes == 1 ? "is" : "are") left alone: volumes hold data.")
            }
        }
        .font(.callout)
        .foregroundStyle(.secondary)
        .fixedSize(horizontal: false, vertical: true)
    }

    private func row(_ i: DockerReport.Item) -> some View {
        Toggle(isOn: Binding(
            get: { chosen.contains(i.name) },
            set: { if $0 { chosen.insert(i.name) } else { chosen.remove(i.name) } }
        )) {
            HStack {
                VStack(alignment: .leading, spacing: 1) {
                    Text(i.title).lineLimit(1).truncationMode(.middle)
                    Text(i.detail).font(.caption).foregroundStyle(.secondary)
                        .lineLimit(1).truncationMode(.tail)
                }
                Spacer(minLength: 12)
                if i.bytes > 0 {
                    Text(Format.bytes(i.bytes)).monospacedDigit()
                }
            }
        }
        .toggleStyle(.checkbox)
        .help(i.title)
    }

    private var buttons: some View {
        let all = (report?.pruned ?? []) + (report?.unused ?? [])
        let picked = all.filter { chosen.contains($0.name) }
        let size = picked.reduce(0) { $0 + $1.bytes }
        return HStack {
            if !all.isEmpty {
                Button(picked.count == all.count ? "Select None" : "Select All") {
                    chosen = picked.count == all.count ? [] : Set(all.map(\.name))
                }
            }
            Spacer()
            Button("Cancel", role: .cancel) { dismiss() }
                .keyboardShortcut(.cancelAction)
            // An image's size is its own layers: those it shares go with the
            // last image using them, so the total is about that.
            Button(picked.isEmpty ? "Remove" : size > 0 ? "Remove \(picked.count) (about \(Format.bytes(size)))" : "Remove \(picked.count)") {
                monitor.clean(CleanAction(kind: "docker", label: "Remove from Docker",
                                          paths: picked.map(\.name), confirm: nil, done: nil))
                dismiss()
            }
            .keyboardShortcut(.defaultAction)
            .disabled(picked.isEmpty)
        }
    }
}
#endif
