import Foundation

/// Stands in for the local Magpie gateway: no vendor is ever contacted here.
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
        let used = Self.generation > 0 ? 90 : 52
        let document: [String: Any] = ["object": "list", "data": [
            ["provider": "codex", "kind": "subscription", "user": "hidden@example.test",
             "readAt": ISO8601DateFormatter().string(from: .now), "last": true,
             "windows": [["name": "7 days", "used": used], ["name": "5 hours", "used": 12]]],
            ["provider": "workbuddy", "kind": "subscription", "user": "second@example.test",
             "readAt": ISO8601DateFormatter().string(from: .now),
             "windows": [["name": "Credits", "used": 1, "remaining": 99, "display": "1 / 100",
                          "limit": 100, "unit": "credits"]]],
            ["provider": "deepseek", "kind": "balance", "user": "key@example.test",
             "readAt": ISO8601DateFormatter().string(from: .now), "balance": "9.99 USD"]
        ]]
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
            print("Live local Magpie direct read passed; providers:", snapshot.providers,
                  "accounts:", snapshot.accounts.count)
            for card in snapshot.accounts {
                let windows = card.items.map { "\($0.name)=\($0.primaryText)" }.joined(separator: ", ")
                print("  \(card.providerName) \(card.accountLabel) [\(card.kind.rawValue)] \(windows.isEmpty ? card.balanceText : windows)")
            }
            return
        }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [SnapshotProtocol.self]
        let session = URLSession(configuration: configuration)
        let port = 13425
        defer {
            WidgetStatusStore.remove(port: port)
            WidgetStatusStore.remove(port: port + 1)
        }
        let fresh = await WidgetStatusLoader.snapshot(force: true, port: port, session: session)
        assert(fresh.accounts.count == 3, "Every provider is read, not only Codex")
        assert(fresh.providers == ["codex", "workbuddy", "deepseek"], "Provider order follows the gateway")
        assert(fresh.accounts[0].primaryItem?.remaining == 10, "A posted refresh updates the reading")
        assert(fresh.accounts[1].primaryItem?.primaryText == "1 / 100", "Credits keep the vendor text")
        assert(fresh.accounts[2].kind == .balance && fresh.accounts[2].balanceText == "9.99 USD", "A balance card survives")
        let redacted = try JSONEncoder().encode(fresh)
        assert(!(String(data: redacted, encoding: .utf8) ?? "").contains("@"), "No identity in the cache")

        let count = SnapshotProtocol.generation
        _ = await WidgetStatusLoader.snapshot(port: port, session: session)
        assert(SnapshotProtocol.generation == count, "Timeline reads take Magpie's kept values, one refresh per policy window")
        assert(SnapshotProtocol.requests.allSatisfy { $0.contains("127.0.0.1:13425/v1/magpie/quotas") },
               "Only the documented quota routes are called")
        assert(SnapshotProtocol.requests.contains { $0.hasPrefix("POST") && $0.contains("/v1/magpie/quotas/refresh") },
               "A refresh asks for every provider, never one hard-coded provider")
        assert(!SnapshotProtocol.requests.contains { $0.contains("provider=") },
               "The widget does not pin a provider when refreshing")

        SnapshotProtocol.offline = true
        let cached = await WidgetStatusLoader.snapshot(port: port, session: session)
        assert(cached.stale && cached.accounts.allSatisfy(\.stale), "Offline falls back to a cached reading")
        assert(cached.accounts[0].primaryItem?.remaining == 10, "The cached value is still shown")
        let other = await WidgetStatusLoader.snapshot(port: port + 1, session: session)
        assert(other.accounts.isEmpty, "A different Magpie port cannot inherit another source cache")
        let invalid = await WidgetStatusLoader.load(port: 0, session: session)
        assert(invalid == nil, "An invalid port is refused before any request")

        SnapshotProtocol.offline = false
        let recovered = await WidgetStatusLoader.snapshot(port: port, session: session)
        assert(!recovered.stale && recovered.accounts.count == 3, "A recovered read is live again")

        let fixture = try Data(contentsOf: URL(fileURLWithPath: "tests/fixtures/magpie_quotas.json"))
        let now = ISO8601DateFormatter().date(from: "2026-10-14T13:32:00+08:00")!
        let mixed = try MagpieQuotaParser.snapshot(data: fixture, now: now)
        assert(mixed.providers.contains("workbuddy") && mixed.providers.contains("deepseek"))
        assert(mixed.accounts.count == 10, "Every fixture row becomes a card")
        assert(mixed.accounts.filter { $0.provider == "workbuddy" }.count == 3)
        assert(mixed.evaluated(at: now.addingTimeInterval(301)).accounts.allSatisfy(\.stale), "Old readings expire")
        let empty = try MagpieQuotaParser.snapshot(data: Data(#"{"object":"list","data":[]}"#.utf8))
        assert(empty.accounts.isEmpty && empty.state == "Empty")
        print("Widget loader: refresh policy, provider coverage, kept reads, redaction, expiry, offline and recovery passed")
    }
}
