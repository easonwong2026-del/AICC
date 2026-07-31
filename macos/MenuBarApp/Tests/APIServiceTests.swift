import Foundation
import XCTest
@testable import AICCCore

// MARK: - URLProtocol stub

final class MockURLProtocol: URLProtocol {
    static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?
    static var requests: [URLRequest] = []

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        MockURLProtocol.requests.append(request)
        guard let handler = MockURLProtocol.handler else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        do {
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}

// MARK: - APIService provider snapshot behavior (0.3)

@MainActor
final class APIServiceTests: XCTestCase {
    override func setUp() {
        super.setUp()
        MockURLProtocol.handler = nil
        MockURLProtocol.requests = []
    }

    override func tearDown() {
        MockURLProtocol.handler = nil
        super.tearDown()
    }

    private func makeService() -> APIService {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [MockURLProtocol.self]
        return APIService(
            baseURL: "http://127.0.0.1:8765",
            session: URLSession(configuration: config)
        )
    }

    private static let providersJSON = """
    {
      "schema_version": 1,
      "updated_at": "2026-07-31 20:00:00",
      "providers": [
        {
          "id": "codex",
          "display_name": "Codex",
          "category": "quota",
          "state": "connected",
          "available": true,
          "metrics": [
            {"key": "weekly", "label": "Weekly", "value": 92, "value_type": "number", "format": "percent", "unit": "%", "primary": true}
          ],
          "actions": []
        }
      ]
    }
    """

    private static let statusJSON = """
    {
      "system": {"status": "Online"},
      "codex": {"weekly": {"remaining": 92}},
      "workbuddy": {"points": 12},
      "deepseek": {"status": "Online"}
    }
    """

    private func respond(_ body: String, status: Int = 200) throws -> (HTTPURLResponse, Data) {
        let response = try XCTUnwrap(
            HTTPURLResponse(
                url: URL(string: "http://127.0.0.1:8765")!,
                statusCode: status,
                httpVersion: nil,
                headerFields: nil
            )
        )
        return (response, Data(body.utf8))
    }

    func testFirstProviderFailureKeepsSafeEmptyState() async {
        let service = makeService()
        MockURLProtocol.handler = { _ in throw URLError(.cannotConnectToHost) }
        await service.fetchProviders()
        XCTAssertNil(service.providers)
        XCTAssertNotNil(service.providerErrorMessage)
        XCTAssertNil(service.providerLastSuccess)
        // A provider failure must never touch the dashboard connection state.
        guard case .loading = service.state else {
            return XCTFail("expected loading state, got \(service.state)")
        }
    }

    func testProviderFailureKeepsLastKnownSnapshot() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] _ in try respond(Self.providersJSON) }
        await service.fetchProviders()
        let snapshot = try XCTUnwrap(service.providers)
        XCTAssertEqual(snapshot.providers.first?.id, "codex")
        XCTAssertNil(service.providerErrorMessage)
        XCTAssertNotNil(service.providerLastSuccess)

        MockURLProtocol.handler = { _ in throw URLError(.timedOut) }
        await service.fetchProviders()
        XCTAssertEqual(service.providers, snapshot)
        XCTAssertNotNil(service.providerErrorMessage)
        XCTAssertNotNil(service.providerLastSuccess)
        guard case .loading = service.state else {
            return XCTFail("expected loading state, got \(service.state)")
        }
    }

    func testProviderRecoveryReplacesSnapshotAndClearsError() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] _ in try respond(Self.providersJSON) }
        await service.fetchProviders()
        XCTAssertNil(service.providerErrorMessage)

        MockURLProtocol.handler = { _ in throw URLError(.cannotConnectToHost) }
        await service.fetchProviders()
        XCTAssertNotNil(service.providerErrorMessage)

        let recoveredJSON = Self.providersJSON.replacingOccurrences(
            of: "\"state\": \"connected\"",
            with: "\"state\": \"cached\""
        )
        MockURLProtocol.handler = { [self] _ in try respond(recoveredJSON) }
        await service.fetchProviders()
        XCTAssertEqual(service.providers?.providers.first?.state, "cached")
        XCTAssertNil(service.providerErrorMessage)
        XCTAssertNotNil(service.providerLastSuccess)
    }

    func testProviderFailureDoesNotFlipDashboardStateAfterStatusSuccess() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] request in
            if request.url?.path == "/api/status" {
                return try respond(Self.statusJSON)
            }
            throw URLError(.cannotConnectToHost)
        }
        await service.fetchStatus()
        guard case .ready = service.state else {
            return XCTFail("expected ready state, got \(service.state)")
        }
        XCTAssertNil(service.providers)
        XCTAssertNotNil(service.providerErrorMessage)
    }

    func testActionRequestUsesKindInPath() async throws {
        let service = makeService()
        MockURLProtocol.handler = { request in
            switch request.url?.path {
            case "/api/status":
                return try self.respond(Self.statusJSON)
            case "/api/providers":
                // performProviderAction reloads the snapshot afterwards via
                // fetchStatus -> fetchProviders.
                return try self.respond(Self.providersJSON)
            default:
                XCTAssertEqual(
                    request.url?.path,
                    "/api/providers/workbuddy/actions/reconnect"
                )
                return try self.respond(#"{"id":"workbuddy"}"#)
            }
        }
        let result = await service.performProviderAction(
            providerId: "workbuddy",
            kind: "reconnect"
        )
        XCTAssertEqual(result, #"{"id":"workbuddy"}"#)
        // `actionID` from a manifest such as "reconnect_workbuddy" must never
        // leak into the route.
        let actionPaths = MockURLProtocol.requests
            .compactMap { $0.url?.path }
            .filter { $0.contains("/actions/") }
        XCTAssertEqual(actionPaths, ["/api/providers/workbuddy/actions/reconnect"])
        XCTAssertFalse(actionPaths.contains("/api/providers/workbuddy/actions/reconnect_workbuddy"))
    }
}
