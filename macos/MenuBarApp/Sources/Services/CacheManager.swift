import Foundation

enum AICCPaths {
    static var logsDirectory: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Logs/AICC-Dashboard", isDirectory: true)
    }
}

enum CacheManager {
    private static let cacheNames = [
        "status.json",
        "codex_last_success.json",
        "workbuddy_last_success.json",
        "deepseek_history.json"
    ]

    static func clear(in directory: URL) throws -> Int {
        var removed = 0
        for name in cacheNames {
            let url = directory.appendingPathComponent(name)
            guard FileManager.default.fileExists(atPath: url.path) else { continue }
            try FileManager.default.removeItem(at: url)
            removed += 1
        }
        return removed
    }
}
