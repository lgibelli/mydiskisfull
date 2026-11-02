// Sign in with ChatGPT, for Ask AI: questions go from this Mac to OpenAI,
// on the user's own ChatGPT plan (`mdif knowledge sign-in`, `sign-out`,
// `account`; mdif_core::chatgpt). What's shown is what OpenAI's guide asks
// for: "Continue with ChatGPT" to sign in, the account once signed in,
// "Using ChatGPT plan" with "Manage usage" where questions are asked, and
// once, after the first sign-in, that the plan is in use.

#if !APP_STORE
import AppKit
import SwiftUI

@MainActor
final class ChatGPT: ObservableObject {
    static let shared = ChatGPT()

    @Published private(set) var account: ChatGPTAccount?
    @Published private(set) var signingIn = false
    /// What went wrong last, said once.
    @Published var problem: String?
    private var attempt: Task<Void, Never>?

    static let usage = URL(string: "https://chatgpt.com/settings/usage")!

    var signedIn: Bool { account?.signedIn == true }

    func load() async {
        if let a = try? await Core.json(ChatGPTAccount.self, ["knowledge", "account"], timeout: 10) {
            account = a
        }
    }

    /// In the browser, which `mdif` waits for up to 5 minutes.
    func signIn() {
        guard !signingIn else { return }
        signingIn = true
        problem = nil
        attempt = Task {
            defer {
                signingIn = false
                attempt = nil
            }
            do {
                account = try await Core.json(ChatGPTAccount.self, ["knowledge", "sign-in"], timeout: 330)
                welcome()
            } catch {
                if !Task.isCancelled { problem = AskAI.sentence(error) }
            }
        }
    }

    func cancel() { attempt?.cancel() }

    func signOut() {
        problem = nil
        Task {
            let r = try? await Core.json(SignedOut.self, ["knowledge", "sign-out"], timeout: 60)
            await load()
            if r?.revoked != true {
                problem = "Signed out here, but OpenAI didn't confirm it: you can disconnect MyDiskIsFull in ChatGPT's settings."
            }
        }
    }

    /// Once, after the first sign-in that lets questions use the plan.
    private func welcome() {
        let d = UserDefaults.standard
        guard account?.plan == true, !d.bool(forKey: "chatgptWelcomed") else { return }
        d.set(true, forKey: "chatgptWelcomed")
        let a = NSAlert()
        a.messageText = "You're using your ChatGPT plan"
        a.informativeText = "Eligible usage in this app uses your ChatGPT plan. Manage usage in your ChatGPT settings."
        a.addButton(withTitle: "Got it")
        a.addButton(withTitle: "Manage Usage")
        NSApp.activate(ignoringOtherApps: true)
        if a.runModal() == .alertSecondButtonReturn { NSWorkspace.shared.open(Self.usage) }
    }
}

/// The sign-in button, as OpenAI names it.
struct ContinueWithChatGPT: View {
    @ObservedObject private var gpt = ChatGPT.shared

    var body: some View {
        if gpt.signingIn {
            HStack(spacing: 8) {
                ProgressView().controlSize(.small)
                Text("Finish signing in in your browser…").foregroundStyle(.secondary)
                Button("Cancel") { gpt.cancel() }
            }
        } else {
            Button("Continue with ChatGPT") { gpt.signIn() }
        }
    }
}

/// Beside where a question is asked: whose plan it uses, and where that's
/// managed.
struct UsingChatGPTPlan: View {
    var body: some View {
        HStack(spacing: 4) {
            Text("Using ChatGPT plan ·").foregroundStyle(.secondary)
            Link("Manage usage", destination: ChatGPT.usage)
        }
        .font(.caption)
    }
}

/// Settings: the account Ask AI uses.
struct ChatGPTAccountRow: View {
    @ObservedObject private var gpt = ChatGPT.shared

    var body: some View {
        Group {
            if let a = gpt.account, a.signedIn {
                LabeledContent {
                    Button("Sign Out") { gpt.signOut() }
                } label: {
                    Text("Use your ChatGPT plan")
                    if a.plan {
                        Text("Signed in as \(a.email ?? "your account"). Questions use your plan; with ChatGPT Plus, its five-hour limit is shared with every app that uses it.")
                    } else {
                        Text("Signed in as \(a.email ?? "your account"), but ChatGPT doesn't let MyDiskIsFull use your plan. Sign out, then sign in again and allow it.")
                    }
                }
                if a.plan { UsingChatGPTPlan() }
            } else {
                LabeledContent {
                    ContinueWithChatGPT()
                } label: {
                    Text("Use your ChatGPT plan")
                    Text("Complete eligible AI requests in this app with usage included in your ChatGPT plan or credits balance (Plus or Pro). MyDiskIsFull itself charges nothing.")
                }
            }
            if let p = gpt.problem {
                Text(p).font(.caption).foregroundStyle(.orange)
            }
        }
        .task { await gpt.load() }
    }
}
#endif
