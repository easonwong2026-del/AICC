import Foundation
import WidgetKit
import os

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

    private static let diagnostics = Logger(subsystem: "com.aieink.dashboard", category: "widget")

    /// Why the last read ended the way it did.  Only status codes and counts:
    /// never an account, a body or a credential.  Kept in the widget's own
    /// container next to the cache so a blank widget can be diagnosed without
    /// a debugger attached to WidgetKit's process.
    static func note(_ message: String, port: Int?) {
        diagnostics.notice("\(message, privacy: .public)")
        let stamp = Int(Date.now.timeIntervalSince1970)
        UserDefaults.standard.set("\(stamp) \(message)", forKey: "aicc.widget.diag")
    }

    static func diagnosticsText() -> String? {
        UserDefaults.standard.string(forKey: "aicc.widget.diag")
    }
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
    /// The kept report is read first and saved before anything else, because a
    /// WidgetKit timeline has a short budget: a slow or refused refresh must
    /// never cost the widget the numbers Magpie already has.  A refresh is a
    /// bonus that runs afterwards — awaited when there is nothing to show yet
    /// or when the host asked for it, otherwise in the background.
    static func load(force: Bool = false, port: Int = 3425, session: URLSession = session) async -> QuotaSnapshot? {
        guard (1...65535).contains(port) else {
            WidgetStatusStore.note("port \(port) refused", port: port)
            return nil
        }
        let base = "http://127.0.0.1:\(port)"
        let allowed = WidgetStatusStore.shouldRefresh(port: port, force: force)
        let kept = await read(base: base, session: session)
        if let kept { WidgetStatusStore.save(kept, port: port) }
        guard allowed else { return kept }

        let empty = kept.map { $0.accounts.isEmpty } ?? true
        if force || empty {
            _ = await refresh(base: base, session: session, timeout: force ? 30 : 15)
            if let refreshed = await read(base: base, session: session) {
                WidgetStatusStore.save(refreshed, port: port)
                WidgetStatusStore.note("refreshed \(refreshed.accounts.count) cards", port: port)
                return refreshed
            }
            WidgetStatusStore.note("refresh failed, kept \(kept?.accounts.count ?? 0)", port: port)
            return kept
        }
        // Something is already on screen: refresh behind it.
        Task.detached { [base] in
            _ = await refresh(base: base, session: session, timeout: 20)
            if let refreshed = await read(base: base, session: session) {
                WidgetStatusStore.save(refreshed, port: port)
                WidgetStatusStore.note("background refresh \(refreshed.accounts.count) cards", port: port)
                WidgetCenter.shared.reloadTimelines(ofKind: AICCWidgetMetadata.kind)
            }
        }
        return kept
    }

    static func read(base: String, session: URLSession) async -> QuotaSnapshot? {
        var request = URLRequest(url: URL(string: base + "/v1/magpie/quotas?kept=1")!)
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = 8
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse, http.statusCode == 200,
                  data.count <= 1024 * 1024, response.url == request.url else {
                WidgetStatusStore.note("kept read HTTP \((response as? HTTPURLResponse)?.statusCode ?? -1)", port: nil)
                return nil
            }
            return try MagpieQuotaParser.snapshot(data: data, refreshSeconds: Int(QuotaPolicy.refreshSeconds))
        } catch {
            WidgetStatusStore.note("kept read \((error as NSError).code)", port: nil)
            return nil
        }
    }

    @discardableResult
    static func refresh(base: String, session: URLSession, timeout: TimeInterval) async -> Bool {
        var request = URLRequest(url: URL(string: base + "/v1/magpie/quotas/refresh")!)
        request.httpMethod = "POST"
        request.timeoutInterval = timeout
        do {
            let (_, response) = try await session.data(for: request)
            return (response as? HTTPURLResponse)?.statusCode == 200
        } catch {
            WidgetStatusStore.note("refresh \((error as NSError).code)", port: nil)
            return false
        }
    }
}
