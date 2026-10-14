import Foundation

enum WidgetStatusStore {
    private static func key(_ port: Int) -> String { "aicc.widget.magpie.\(port)" }

    static func shouldRefresh(port: Int, force: Bool) -> Bool {
        let key = "aicc.widget.magpie.last-refresh.\(port)"
        let now = Date.now.timeIntervalSince1970
        let last = UserDefaults.standard.double(forKey: key)
        guard force || now - last >= 300 || last > now else { return false }
        UserDefaults.standard.set(now, forKey: key)
        return true
    }

    static func load(port: Int = 3425) -> WidgetDisplaySnapshot? {
        guard let data = UserDefaults.standard.data(forKey: key(port)),
              let snapshot = try? JSONDecoder().decode(WidgetDisplaySnapshot.self, from: data),
              snapshot.codexSource == "Magpie" else { return nil }
        return snapshot
    }

    static func save(_ snapshot: WidgetDisplaySnapshot, port: Int = 3425) {
        guard !snapshot.stale, let data = try? JSONEncoder().encode(snapshot) else { return }
        UserDefaults.standard.set(data, forKey: key(port))
    }

    static func cachedOrPlaceholder(port: Int = 3425) -> WidgetDisplaySnapshot {
        if let cached = load(port: port) { return cached.evaluated(offline: true) }
        var placeholder = WidgetDisplaySnapshot.placeholder
        placeholder.codexSource = "Magpie"
        return placeholder
    }

    static func remove(port: Int = 3425) { UserDefaults.standard.removeObject(forKey: key(port)) }
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

    static func snapshot(force: Bool = false, port: Int = 3425, session: URLSession = session) async -> WidgetDisplaySnapshot {
        await load(force: force, port: port, session: session) ?? WidgetStatusStore.cachedOrPlaceholder(port: port)
    }

    static func load(force: Bool = false, port: Int = 3425, session: URLSession = session) async -> WidgetDisplaySnapshot? {
        guard (1...65535).contains(port) else { return nil }
        let base = "http://127.0.0.1:\(port)"
        do {
            if WidgetStatusStore.shouldRefresh(port: port, force: force) {
                var refresh = URLRequest(url: URL(string: base + "/v1/magpie/quotas/refresh?provider=codex")!)
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
            let snapshot = try MagpieQuotaReport.snapshot(data: data)
            WidgetStatusStore.save(snapshot, port: port)
            return snapshot
        } catch { return nil }
    }
}
