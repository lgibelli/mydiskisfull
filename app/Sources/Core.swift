// Runs the `mdif` inside the bundle. The Rust core is the only place that
// measures, forecasts or changes a setting; the app asks it and shows what
// it says, so the command line and the menu can't disagree.

import Foundation
import os

let log = Logger(subsystem: "io.github.lgibelli.mydiskisfull", category: "app")

/// Set once, read from another thread.
private final class Flag: @unchecked Sendable {
    private let lock = NSLock()
    private var value = false
    func set() { lock.withLock { value = true } }
    var isSet: Bool { lock.withLock { value } }
}

struct CoreError: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

enum Core {
    /// Contents/MacOS/mdif, next to the app's own executable.
    static var executable: URL {
        Bundle.main.url(forAuxiliaryExecutable: "mdif")
            ?? Bundle.main.bundleURL.appendingPathComponent("Contents/MacOS/mdif")
    }

    /// Runs `mdif args…` off the main thread and returns what it printed.
    /// Fails with its error message when it exits non-zero, and stops it
    /// after `timeout` seconds, so a stuck command can't leave the menu
    /// "Measuring…" for good.
    static func run(_ args: [String], timeout: TimeInterval = 120) async throws -> Data {
        let exe = executable
        return try await Task.detached(priority: .utility) {
            let p = Process()
            p.executableURL = exe
            p.arguments = args
            p.qualityOfService = .utility
            let out = Pipe(), err = Pipe()
            p.standardOutput = out
            p.standardError = err
            p.standardInput = FileHandle.nullDevice
            try p.run()
            let timedOut = Flag()
            let stop = DispatchWorkItem {
                guard p.isRunning else { return }
                timedOut.set()
                p.terminate()
            }
            DispatchQueue.global(qos: .utility).asyncAfter(deadline: .now() + timeout, execute: stop)
            defer { stop.cancel() }
            // Both pipes are drained at once: a child that fills one while
            // we wait on the other would never exit.
            async let errData = Task.detached { err.fileHandleForReading.readDataToEndOfFile() }.value
            let data = out.fileHandleForReading.readDataToEndOfFile()
            let errText = String(decoding: await errData, as: UTF8.self)
            p.waitUntilExit()
            if timedOut.isSet {
                log.error("mdif \(args.joined(separator: " "), privacy: .public) stopped after \(Int(timeout)) s")
                throw CoreError(message: "mdif \(args.first ?? "") took longer than \(Int(timeout)) seconds and was stopped.")
            }
            guard p.terminationStatus == 0 else {
                let msg = errText.trimmingCharacters(in: .whitespacesAndNewlines)
                    .replacingOccurrences(of: "Error: ", with: "")
                log.error("mdif \(args.joined(separator: " "), privacy: .public) failed: \(msg, privacy: .public)")
                throw CoreError(message: msg.isEmpty ? "mdif exited with status \(p.terminationStatus)" : msg)
            }
            return data
        }.value
    }

    static func json<T: Decodable>(_ type: T.Type, _ args: [String],
                                   timeout: TimeInterval = 120) async throws -> T {
        let data = try await run(args + ["--json"], timeout: timeout)
        do {
            return try Wire.decode(type, from: data)
        } catch {
            log.error("mdif \(args.joined(separator: " "), privacy: .public): unexpected output: \(String(describing: error), privacy: .public)")
            throw CoreError(message: "The bundled mdif printed something this version of the app can't read.")
        }
    }
}
