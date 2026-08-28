import Foundation

enum LogFileLimiter {
    static let maxBytes: UInt64 = 1_048_576
    static let retainedBytes: UInt64 = 262_144

    // ponytail: retain only recent output; add full rotation only if diagnostics need more history.
    static func trimIfNeeded(_ url: URL) {
        do {
            guard
                let number = try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber,
                number.uint64Value > maxBytes
            else { return }

            let data = try Data(contentsOf: url)
            let tail = Data(data.suffix(Int(retainedBytes)))
            try tail.write(to: url, options: .atomic)
        } catch {
            // Log maintenance must never prevent the server from starting.
        }
    }
}
