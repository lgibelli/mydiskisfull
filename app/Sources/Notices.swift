// What an action did (a clean-up, a tweak, Tidy Now, Time Machine), said in
// the main window instead of in an alert over it: a banner at the foot of
// the page. What went as asked goes by itself after a while (not while the
// pointer is on it); what wasn't done stays until it's closed, so its
// reasons can be read. The questions asked before what can't be undone are
// still alerts (CleanFlow, TweakFlow).

import SwiftUI

struct Notice: Identifiable, Equatable {
    let id = UUID()
    /// It did what was asked; else nothing, or not all of it, was done.
    let ok: Bool
    let title: String
    let text: String
    /// Stays until it's closed.
    let stays: Bool

    /// How long it shows: long enough to read.
    var seconds: Int {
        let words = (title + " " + text).split(whereSeparator: \.isWhitespace).count
        return max(8, words / 3 + 4)
    }
}

@MainActor
final class Notices: ObservableObject {
    static let shared = Notices()
    @Published private(set) var current: Notice?

    /// `stays` defaults to whether it went wrong.
    func post(ok: Bool, _ title: String, _ text: String, stays: Bool? = nil) {
        current = Notice(ok: ok, title: title, text: text, stays: stays ?? !ok)
    }

    func dismiss(_ id: Notice.ID) {
        if current?.id == id { current = nil }
    }
}

/// The banner, over the foot of the main window's page.
struct NoticeBanner: View {
    let notice: Notice
    @ObservedObject private var notices = Notices.shared
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: notice.ok ? "checkmark.circle.fill" : "exclamationmark.triangle.fill")
                .font(.title3)
                .foregroundStyle(notice.ok ? Color.green : Color.orange)
            VStack(alignment: .leading, spacing: 3) {
                Text(notice.title).font(.headline)
                if !notice.text.isEmpty {
                    Text(notice.text)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .lineLimit(14)
                        .fixedSize(horizontal: false, vertical: true)
                        .textSelection(.enabled)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                withAnimation { notices.dismiss(notice.id) }
            } label: {
                Image(systemName: "xmark")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.secondary)
                    .frame(width: 18, height: 18)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help("Close")
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 11)
        .frame(maxWidth: 560)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 12, style: .continuous).strokeBorder(Theme.hairline)
        }
        .shadow(color: .black.opacity(0.14), radius: 14, y: 4)
        .onHover { hovering = $0 }
        .accessibilityElement(children: .combine)
        .task(id: notice.id) {
            guard !notice.stays else { return }
            var left = notice.seconds
            while left > 0 {
                try? await Task.sleep(for: .seconds(1))
                if Task.isCancelled { return }
                if !hovering { left -= 1 }
            }
            withAnimation { notices.dismiss(notice.id) }
        }
    }
}
