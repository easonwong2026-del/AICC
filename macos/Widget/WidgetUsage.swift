import Foundation
import WidgetKit

/// The board widget's own cache: usage is a separate read from quotas, with its
/// own freshness, so it never waits on the quota refresh path.
enum WidgetUsageStore {
    private static func key(_ port: Int) -> String { "aicc.widget.usage.\(port)" }

    static func load(port: Int = 3425) -> UsageSnapshot? {
        guard let data = UserDefaults.standard.data(forKey: key(port)),
              let snapshot = try? JSONDecoder().decode(UsageSnapshot.self, from: data) else { return nil }
        return snapshot
    }

    static func save(_ snapshot: UsageSnapshot, port: Int = 3425) {
        guard let data = try? JSONEncoder().encode(snapshot) else { return }
        UserDefaults.standard.set(data, forKey: key(port))
    }

    static func remove(port: Int = 3425) { UserDefaults.standard.removeObject(forKey: key(port)) }
}

/// Reads the same three endpoints `metrics.py` reads, in parallel, and falls
/// back to the last good reading.  A failed read never blanks the board.
enum WidgetUsageLoader {
    /// Usage moves faster than quota, but a desktop tile does not need 15s.
    static let freshSeconds: TimeInterval = 60

    static func load(port: Int = 3425, force: Bool = false) async -> UsageSnapshot {
        guard (1...65535).contains(port) else { return WidgetUsageStore.load(port: port) ?? .placeholder(port: port) }
        if !force, let cached = WidgetUsageStore.load(port: port),
           Date.now.timeIntervalSince1970 - cached.generatedAt < freshSeconds {
            return cached.evaluated()
        }

        let base = "http://127.0.0.1:\(port)"
        async let quotas = get(base + "/v1/magpie/quotas?kept=1")
        async let usage = get(base + "/v1/magpie/usage")
        async let requests = get(base + "/v1/magpie/usage/requests?limit=3")
        let (quotaData, usageData, requestData) = await (quotas, usage, requests)

        guard usageData != nil || requestData != nil || quotaData != nil else {
            if let cached = WidgetUsageStore.load(port: port) {
                WidgetStatusStore.note("usage unchanged (offline)", port: port)
                return cached.evaluated(offline: true)
            }
            return UsageSnapshot.placeholder(port: port).evaluated(offline: true)
        }

        let snapshot = UsageParser.snapshot(usage: usageData, requests: requestData,
                                            quotas: quotaData, port: port)
        // Keep the last good numbers rather than a half-filled page.
        var merged = snapshot
        if let cached = WidgetUsageStore.load(port: port), snapshot.available {
            if snapshot.topModel.isEmpty { merged.topModel = cached.topModel; merged.topModelCalls = cached.topModelCalls }
            if snapshot.sessionCount == 0 { merged.sessionCount = cached.sessionCount }
            if snapshot.costMicros == 0 { merged.costMicros = cached.costMicros }
            if snapshot.remainingWindow.isEmpty {
                merged.remainingPercent = cached.remainingPercent
                merged.remainingWindow = cached.remainingWindow
                merged.resetSeconds = cached.resetSeconds
            }
        }
        WidgetUsageStore.save(merged, port: port)
        WidgetStatusStore.note("usage \(merged.callsToday)c/\(merged.errorsToday)e", port: port)
        return merged
    }

    private static func get(_ url: String) async -> Data? {
        guard let target = URL(string: url) else { return nil }
        var request = URLRequest(url: target)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 8
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        do {
            let (data, response) = try await WidgetStatusLoader.session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200,
                  data.count <= 4 * 1024 * 1024, response.url == request.url else { return nil }
            return data
        } catch {
            return nil
        }
    }
}
