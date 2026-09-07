import Foundation

enum WidgetStatusStore {
    private static let key = "aicc.widget.last-display-snapshot"

    static func load() -> WidgetDisplaySnapshot? {
        guard
            let data = UserDefaults.standard.data(forKey: key),
            let snapshot = try? JSONDecoder().decode(WidgetDisplaySnapshot.self, from: data)
        else { return nil }
        return snapshot
    }

    static func save(_ snapshot: WidgetDisplaySnapshot) {
        guard !snapshot.stale, let data = try? JSONEncoder().encode(snapshot) else { return }
        UserDefaults.standard.set(data, forKey: key)
    }

    static func cachedOrPlaceholder() -> WidgetDisplaySnapshot {
        load()?.evaluated(offline: true) ?? .placeholder
    }

    static func remove() {
        UserDefaults.standard.removeObject(forKey: key)
    }
}

enum WidgetStatusLoader {


    static func snapshot(force: Bool = false, baseURL: String = "http://127.0.0.1:8765", session: URLSession = .shared) async -> WidgetDisplaySnapshot {
        if let fresh = await load(force: force, baseURL: baseURL, session: session) {
            return fresh
        }
        return WidgetStatusStore.cachedOrPlaceholder()
    }

    static func load(force: Bool = false, baseURL: String = "http://127.0.0.1:8765", session: URLSession = .shared) async -> WidgetDisplaySnapshot? {
        guard let endpoint = URL(string: baseURL + (force ? "/api/refresh" : "/api/status")) else { return nil }
        var request = URLRequest(url: endpoint)
        request.httpMethod = force ? "POST" : "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.timeoutInterval = force ? 25 : 10

        do {
            let (data, response) = try await session.data(for: request)
            guard
                let http = response as? HTTPURLResponse,
                (200..<300).contains(http.statusCode)
            else { return nil }

            let payload = try JSONDecoder().decode(WidgetStatusPayload.self, from: data)
            let snapshot = WidgetDisplaySnapshot(payload: payload, fetchedAt: .now)
            await DisplaySnapshotBridge.publish(snapshot, revision: payload.display_revision, baseURL: baseURL, session: session)
            WidgetStatusStore.save(snapshot)
            return snapshot
        } catch {
            return nil
        }
    }
}
