import Foundation

enum WidgetStatusStore {
    private static func key(_ port: Int) -> String { "aicc.widget.quota.\(port)" }
    private static func refreshKey(_ port: Int) -> String { "aicc.widget.magpie.last-refresh.\(port)" }

    /// Magpie enforces its own per-card gap; this only keeps a widget timeline
    /// reload from pressing refresh more often than the policy asks for.
    static func shouldRefresh(port: Int, force: Bool, minimumGap: TimeInterval = 60) -> Bool {
        let now = Date.now.timeIntervalSince1970
        let last = UserDefaults.standard.double(forKey: refreshKey(port))
        let elapsed = now - last
        guard force || elapsed >= QuotaPolicy.refreshSeconds || elapsed < 0 else { return false }
        guard force || elapsed >= minimumGap || elapsed < 0 else { return false }
        UserDefaults.standard.set(now, forKey: refreshKey(port))
        return true
    }

    static func load(port: Int = 3425) -> QuotaSnapshot? {
        guard let data = UserDefaults.standard.data(forKey: key(port)),
              let snapshot = try? JSONDecoder().decode(QuotaSnapshot.self, from: data) else { return nil }
        return snapshot
    }

    static func save(_ snapshot: QuotaSnapshot, port: Int = 3425) {
        guard let data = try? JSONEncoder().encode(snapshot) else { return }
        UserDefaults.standard.set(data, forKey: key(port))
    }

    static func cachedOrPlaceholder(port: Int = 3425) -> QuotaSnapshot {
        if let cached = load(port: port) { return cached.evaluated(offline: true) }
        return .placeholder()
    }

    static func remove(port: Int = 3425) { UserDefaults.standard.removeObject(forKey: key(port)) }
}

enum QuotaPolicy {
    /// Shared with `collectors/magpie.py` (REFRESH_SECONDS) and the APK.
    static let refreshSeconds: TimeInterval = 300
}

final class MagpieSessionDelegate: NSObject, URLSessionTaskDelegate {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest,
                    completionHandler: @escaping (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

enum WidgetStatusLoader {
    static let session: URLSession = {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.connectionProxyDictionary = [:]
        configuration.httpMaximumConnectionsPerHost = 2
        return URLSession(configuration: configuration, delegate: MagpieSessionDelegate(), delegateQueue: nil)
    }()

    static func snapshot(force: Bool = false, port: Int = 3425, session: URLSession = session) async -> QuotaSnapshot {
        await load(force: force, port: port, session: session) ?? WidgetStatusStore.cachedOrPlaceholder(port: port)
    }

    /// Reads Magpie directly: the widget never needs AICC running.
    ///
    /// A forced read asks Magpie to refresh every provider (no `provider`
    /// parameter means all of them, which is what the host app's refresh link
    /// presses too).  A normal read only takes what Magpie last kept, so a
    /// timeline reload never turns into a burst of vendor requests.
    static func load(force: Bool = false, port: Int = 3425, session: URLSession = session) async -> QuotaSnapshot? {
        guard (1...65535).contains(port) else { return nil }
        let base = "http://127.0.0.1:\(port)"
        do {
            if WidgetStatusStore.shouldRefresh(port: port, force: force) {
                var refresh = URLRequest(url: URL(string: base + "/v1/magpie/quotas/refresh")!)
                refresh.httpMethod = "POST"
                refresh.timeoutInterval = 35
                let (_, response) = try await session.data(for: refresh)
                guard (response as? HTTPURLResponse)?.statusCode == 200 else { return nil }
            }
            var request = URLRequest(url: URL(string: base + "/v1/magpie/quotas?kept=1")!)
            request.cachePolicy = .reloadIgnoringLocalCacheData
            request.timeoutInterval = 10
            let (data, response) = try await session.data(for: request)
            guard (response as? HTTPURLResponse)?.statusCode == 200, data.count <= 1024 * 1024,
                  response.url == request.url else { return nil }
            let snapshot = try MagpieQuotaParser.snapshot(data: data, refreshSeconds: Int(QuotaPolicy.refreshSeconds))
            WidgetStatusStore.save(snapshot, port: port)
            return snapshot
        } catch { return nil }
    }
}
