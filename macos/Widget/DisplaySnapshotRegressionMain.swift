import Foundation

final class SnapshotProtocol: URLProtocol {
    static var generation = 0
    static var offline = false
    static var deepseekFailed = false
    static var requests: [String] = []
    static var published: [String: Any]?
    static let fetched = Date.now.timeIntervalSince1970
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}
    override func startLoading() {
        let path = request.url!.path
        Self.requests.append("\(request.httpMethod!) \(path)")
        if Self.offline {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        var result: [String: Any] = [:]
        if path == "/api/display-snapshot" {
            var data = request.httpBody ?? Data()
            if data.isEmpty, let stream = request.httpBodyStream {
                stream.open()
                defer { stream.close() }
                var buffer = [UInt8](repeating: 0, count: 4096)
                while stream.hasBytesAvailable {
                    let count = stream.read(&buffer, maxLength: buffer.count)
                    if count <= 0 { break }
                    data.append(buffer, count: count)
                }
            }
            let body = try! JSONSerialization.jsonObject(with: data) as! [String: Any]
            Self.published = body["snapshot"] as? [String: Any]
            result = ["ok": true]
        } else {
            if path == "/api/refresh" { Self.generation += 1; Self.published = nil }
            let fresh = Self.generation > 0
            result = [
                "fetched_at": Self.fetched,
                "display_revision": "\(Self.generation)",
                "codex": ["weekly": ["remaining": fresh ? 10 : 48, "reset": "2026-09-07 12:02"],
                          "five_hour": ["remaining": fresh ? 88 : 97]],
                "workbuddy": ["points": fresh ? 5947.0 : 5852.96],
                "deepseek": ["status": "Online", "balances": [["currency": "CNY", "total_balance": "58.39"]]]
            ]
            if Self.deepseekFailed {
                result["deepseek"] = ["status": "Connection error", "error_code": "connection_error", "stale": true,
                                      "balances": [["currency": "CNY", "total_balance": "58.39"]]]
            }
            if let published = Self.published { result["display_snapshot"] = published }
        }
        let data = try! JSONSerialization.data(withJSONObject: result)
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
}

@main
struct DisplaySnapshotRegressionMain {
    @MainActor static func main() async throws {
        if CommandLine.arguments.contains("--live") {
            let api = APIService(baseURL: "http://127.0.0.1:8765")
            await api.fetchStatus(force: true)
            guard let dashboard = api.displaySnapshot else { fatalError("Live dashboard fetch failed") }
            let widget = await WidgetStatusLoader.snapshot(force: true)
            // Reopening the Dashboard performs GET /api/status after a Widget refresh.
            await api.fetchStatus()
            let current = api.displaySnapshot!
            assert(current.codexWeeklyRemaining == widget.codexWeeklyRemaining)
            assert(current.codexFiveHourRemaining == widget.codexFiveHourRemaining)
            assert(current.codexWeeklyReset == widget.codexWeeklyReset)
            assert(current.workbuddyPoints == widget.workbuddyPoints)
            assert(current.deepseekBalanceText == widget.deepseekBalanceText)
            assert(current.deepseekState == widget.deepseekState)
            print("Dashboard force refresh:", String(data: try JSONEncoder().encode(dashboard), encoding: .utf8)!)
            print("Widget force refresh:", String(data: try JSONEncoder().encode(widget), encoding: .utf8)!)
            print("Live Dashboard/Widget display fields match after refresh")
            return
        }
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [SnapshotProtocol.self]
        let session = URLSession(configuration: config)
        let api = APIService(baseURL: "http://127.0.0.1:8765", session: session)
        await api.fetchStatus()
        let old = api.displaySnapshot!
        WidgetStatusStore.save(old)
        assert(old.codexWeeklyNumber == "48" && old.workbuddyPoints == 5852.96)
        await api.fetchStatus(force: true)
        let dashboard = api.displaySnapshot!
        let widget = await WidgetStatusLoader.snapshot(session: session)
        assert(dashboard == widget, "Dashboard and Widget must consume identical normalized snapshots")
        assert(widget.codexWeeklyNumber == "10" && widget.codexFiveHourRemaining == 88)
        assert(widget.workbuddyPointsText == "5,947" && widget.workbuddyPoints == 5947)
        assert(widget.codexWeeklyReset == "2026-09-07 12:02")
        assert(widget.deepseekBalanceText == "58.39" && widget.deepseekStatusText == "Online")
        assert(WidgetStatusStore.load() == widget, "host refresh must replace the widget's old values")
        let before = SnapshotProtocol.generation
        _ = await WidgetStatusLoader.snapshot(force: true, session: session)
        assert(SnapshotProtocol.generation == before + 1, "Widget refresh must force backend collection")
        assert(SnapshotProtocol.requests.contains("POST /api/refresh"))
        assert(SnapshotProtocol.requests.contains("POST /api/display-snapshot"))
        SnapshotProtocol.deepseekFailed = true
        await api.fetchStatus(force: true)
        let providerFailure = await WidgetStatusLoader.snapshot(session: session)
        assert(api.displaySnapshot == providerFailure)
        assert(providerFailure.deepseekState == "stale" && providerFailure.deepseekBalanceText == "58.39")
        assert(providerFailure.deepseekStatusText == "缓存 / connection_error")
        SnapshotProtocol.offline = true
        await api.fetchStatus()
        let offlineWidget = await WidgetStatusLoader.snapshot(session: session)
        assert(api.displaySnapshot == offlineWidget)
        assert(offlineWidget.deepseekState == "stale" && offlineWidget.codexState == "stale")
        assert(!offlineWidget.deepseekIsOnline && offlineWidget.deepseekBalanceText == "58.39")
        WidgetStatusStore.remove()
        print("Dashboard/Widget shared snapshot, force refresh and offline regression smoke passed")
    }
}
