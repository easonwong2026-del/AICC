import Foundation

final class SnapshotProtocol: URLProtocol {
    static var offline = false
    static var requests: [String] = []
    static var generation = 0
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func stopLoading() {}
    override func startLoading() {
        Self.requests.append(request.httpMethod! + " " + request.url!.absoluteString)
        if Self.offline {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        if request.httpMethod == "POST" { Self.generation += 1 }
        let document: [String: Any] = ["object": "list", "data": [
            ["provider": "codex", "kind": "subscription", "user": "hidden@example.test",
             "readAt": ISO8601DateFormatter().string(from: .now), "windows": [
                ["name": "7 days", "used": Self.generation > 0 ? 90 : 52], ["name": "5 hours", "used": 12]]]]]
        let data = try! JSONSerialization.data(withJSONObject: document)
        client?.urlProtocol(self, didReceive: HTTPURLResponse(url: request.url!, statusCode: 200,
                           httpVersion: nil, headerFields: nil)!, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: data)
        client?.urlProtocolDidFinishLoading(self)
    }
}

@main
struct DisplaySnapshotRegressionMain {
    static func main() async throws {
        if CommandLine.arguments.contains("--live") {
            guard let snapshot = await WidgetStatusLoader.load(force: true) else { throw URLError(.cannotConnectToHost) }
            print("Live local Magpie direct read passed; Codex accounts:", snapshot.codexAccounts?.count ?? 0)
            return
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [SnapshotProtocol.self]
        let session = URLSession(configuration: configuration)
        let port = 13425
        defer { WidgetStatusStore.remove(port: port) }
        let fresh = await WidgetStatusLoader.snapshot(force: true, port: port, session: session)
        assert(fresh.codexWeeklyRemaining == 10 && fresh.codexFiveHourRemaining == 88)
        assert(fresh.codexSource == "Magpie" && fresh.codexActiveAccountID == nil)
        let redacted = try JSONEncoder().encode(fresh)
        assert(!String(data: redacted, encoding: .utf8)!.contains("@"))
        let count = SnapshotProtocol.generation
        _ = await WidgetStatusLoader.snapshot(port: port, session: session)
        assert(SnapshotProtocol.generation == count, "Automatic reads are limited to one refresh per five minutes")
        assert(SnapshotProtocol.requests.allSatisfy { $0.contains("127.0.0.1:13425/v1/magpie/quotas") })
        assert(SnapshotProtocol.requests.contains { $0.contains("POST") && $0.contains("provider=codex") })
        SnapshotProtocol.offline = true
        let cached = await WidgetStatusLoader.snapshot(port: port, session: session)
        assert(cached.stale && cached.codexAccounts?.first?.stale == true && cached.codexWeeklyRemaining == 10)
        let other = await WidgetStatusLoader.snapshot(port: port + 1, session: session)
        assert(other.codexAccounts == nil, "A different Magpie port cannot inherit another source cache")
        let invalid = await WidgetStatusLoader.load(port: 0, session: session)
        assert(invalid == nil)
        SnapshotProtocol.offline = false
        let recovered = await WidgetStatusLoader.snapshot(port: port, session: session)
        assert(!recovered.stale && recovered.codexWeeklyRemaining == 10)
        let fixture = try Data(contentsOf: URL(fileURLWithPath: "tests/fixtures/magpie_quotas.json"))
        let now = ISO8601DateFormatter().date(from: "2026-10-14T13:32:00+08:00")!
        let mixed = try MagpieQuotaReport.snapshot(data: fixture, now: now)
        assert(mixed.codexAccounts?.count == 2 && mixed.codexAccounts?[0].weekly?.remaining == 88)
        assert(mixed.codexAccounts?[1].weekly?.remaining == 59 && mixed.codexWeeklyRemaining == nil)
        assert(mixed.evaluated(at: now.addingTimeInterval(301)).codexAccounts?.allSatisfy { $0.stale == true } == true)
        let empty = try MagpieQuotaReport.snapshot(data: Data(#"{"object":"list","data":[]}"#.utf8))
        assert(empty.codexAccounts?.isEmpty == true && empty.codexWeeklyRemaining == nil)
        print("Independent Magpie Widget: refresh limits, source isolation, redaction, expiry, offline and recovery passed")
    }
}
