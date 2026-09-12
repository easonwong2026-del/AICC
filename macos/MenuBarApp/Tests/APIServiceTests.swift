import Foundation
import XCTest
@testable import AICCCore

// MARK: - URLProtocol stub

final class MockURLProtocol: URLProtocol {
    static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?
    static var requests: [URLRequest] = []
    static var onStart: ((MockURLProtocol) -> Void)?

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        MockURLProtocol.requests.append(request)
        if let onStart = Self.onStart { onStart(self); return }
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

    func complete(_ body: String) {
        let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

@MainActor
final class APIServiceTests: XCTestCase {
    override func setUp() {
        super.setUp()
        MockURLProtocol.onStart = nil
        MockURLProtocol.handler = nil
        MockURLProtocol.requests = []
    }

    override func tearDown() {
        MockURLProtocol.onStart = nil
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

    private static let statusJSON = """
    {
      "system": {"label": "System", "status": "Online", "platform": "macOS"},
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

    func testFetchStatusUsesOnlyStatusEndpoint() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] _ in try respond(Self.statusJSON) }

        await service.fetchStatus()

        guard case .ready = service.state else {
            return XCTFail("expected ready state, got \(service.state)")
        }
        XCTAssertEqual(service.status?.system?.status, "Online")
        XCTAssertEqual(MockURLProtocol.requests.map { $0.url?.path }, ["/api/status"])
        XCTAssertEqual(MockURLProtocol.requests.first?.httpMethod, "GET")
    }

    func testForceFetchUsesRefreshEndpoint() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] _ in try respond(Self.statusJSON) }

        await service.fetchStatus(force: true)

        XCTAssertEqual(MockURLProtocol.requests.map { $0.url?.path }, ["/api/refresh"])
        XCTAssertEqual(MockURLProtocol.requests.first?.httpMethod, "POST")
    }

    func testConcurrentForcesShareOneRequest() async {
        let service = makeService()
        let started = expectation(description: "force started")
        var held: MockURLProtocol?
        MockURLProtocol.onStart = { connection in
            Task { @MainActor in
                if held == nil {
                    held = connection
                    started.fulfill()
                } else {
                    connection.complete(Self.statusJSON)
                }
            }
        }
        let first = Task { await service.fetchStatus(force: true) }
        await fulfillment(of: [started], timeout: 2)
        let entered = expectation(description: "joined force")
        entered.expectedFulfillmentCount = 10
        var completed = 0
        let followers = (0..<10).map { _ in Task { @MainActor in
            entered.fulfill()
            await service.fetchStatus(force: true)
            completed += 1
        } }
        await fulfillment(of: [entered], timeout: 2)
        XCTAssertEqual(completed, 0)
        XCTAssertEqual(MockURLProtocol.requests.count, 1)
        held?.complete(Self.statusJSON)
        await first.value
        for task in followers { await task.value }
        XCTAssertEqual(completed, 10)
        XCTAssertEqual(MockURLProtocol.requests.map { $0.url?.path }, ["/api/refresh"])
        XCTAssertEqual(service.status?.codex?.weekly?.remaining, 92)
    }

    func testNormalFetchAppendsOnlyOnePendingForce() async {
        let service = makeService()
        let normalStarted = expectation(description: "normal started")
        let forceStarted = expectation(description: "pending force started")
        var normal: MockURLProtocol?
        var forced: MockURLProtocol?
        MockURLProtocol.onStart = { connection in
            Task { @MainActor in
                if connection.request.url?.path == "/api/status" {
                    normal = connection
                    normalStarted.fulfill()
                } else {
                    if forced == nil {
                        forced = connection
                        forceStarted.fulfill()
                    } else {
                        connection.complete(Self.statusJSON)
                    }
                }
            }
        }
        let first = Task { await service.fetchStatus() }
        await fulfillment(of: [normalStarted], timeout: 2)
        let entered = expectation(description: "pending callers entered")
        entered.expectedFulfillmentCount = 10
        let followers = (0..<10).map { _ in Task { @MainActor in
            entered.fulfill()
            await service.fetchStatus(force: true)
        } }
        await fulfillment(of: [entered], timeout: 2)
        normal?.complete(Self.statusJSON)
        await fulfillment(of: [forceStarted], timeout: 2)
        let joined = expectation(description: "late force caller joined")
        let late = Task { @MainActor in
            joined.fulfill()
            await service.fetchStatus(force: true)
        }
        await fulfillment(of: [joined], timeout: 2)
        forced?.complete(#"{"codex":{"weekly":{"remaining":17}}}"#)
        await first.value
        for task in followers { await task.value }
        await late.value
        XCTAssertEqual(MockURLProtocol.requests.map { $0.url?.path }, ["/api/status", "/api/refresh"])
        XCTAssertEqual(service.status?.codex?.weekly?.remaining, 17)
        XCTAssertEqual(service.displaySnapshot?.codexWeeklyRemaining, 17)
    }

    func testLocalResponseIsCommittedBeforePublishCompletes() async {
        let service = makeService()
        MockURLProtocol.handler = { [self] _ in try respond(Self.statusJSON) }
        await service.fetchStatus()
        let oldRefresh = service.lastRefresh
        service.errorMessage = "old error"
        service.state = .stale
        let publishing = expectation(description: "publish held")
        var publication: MockURLProtocol?
        MockURLProtocol.onStart = { connection in
            if connection.request.url?.path == "/api/display-snapshot" {
                Task { @MainActor in
                    publication = connection
                    publishing.fulfill()
                }
            } else {
                connection.complete(#"{"display_revision":"new","codex":{"weekly":{"remaining":17}}}"#)
            }
        }
        let refresh = Task { await service.fetchStatus(force: true) }
        await fulfillment(of: [publishing], timeout: 2)
        XCTAssertEqual(service.status?.codex?.weekly?.remaining, 17)
        XCTAssertEqual(service.displaySnapshot?.codexWeeklyRemaining, 17)
        XCTAssertNotNil(service.lastRefresh)
        XCTAssertGreaterThanOrEqual(service.lastRefresh!, oldRefresh!)
        XCTAssertNil(service.errorMessage)
        if case .ready = service.state {} else { XCTFail("local response must be ready before publish completes") }
        publication?.complete(#"{"ok":true}"#)
        await refresh.value
    }

    func testDeepSeekFreshnessIsIndependentOfAccountAvailability() throws {
        for (status, balance, stale, code, expectedState, expectedText) in [
            ("Online", "58.39", false, "", "live", "Online"),
            ("No balance", "0.00", false, "", "live", "No balance"),
            ("Not configured", "", false, "", "unavailable", "Not configured"),
            ("HTTP 401", "58.39", true, "http_401", "stale", "缓存 / http_401"),
            ("Request timed out", "58.39", true, "timeout", "stale", "缓存 / timeout")
        ] {
            let object: [String: Any] = ["deepseek": [
                "status": status, "stale": stale, "error_code": code.isEmpty ? NSNull() : code as Any,
                "balances": balance.isEmpty ? [] : [["currency": "CNY", "total_balance": balance]]
            ]]
            let payload = try JSONDecoder().decode(WidgetStatusPayload.self, from: JSONSerialization.data(withJSONObject: object))
            let snapshot = WidgetDisplaySnapshot(payload: payload, fetchedAt: .now)
            XCTAssertEqual(snapshot.deepseekState, expectedState, status)
            XCTAssertEqual(snapshot.deepseekStatusText, expectedText, status)
            if !stale { XCTAssertFalse(snapshot.deepseekStale, status) }
            let decoded = try JSONDecoder().decode(WidgetDisplaySnapshot.self, from: JSONEncoder().encode(snapshot))
            XCTAssertEqual(decoded, snapshot)
        }
    }

    func testWidgetDisplaySignatureTracksVisibleValues() throws {
        let initial = WidgetDisplaySignature(
            codexWeeklyRemaining: 85,
            codexFiveHourRemaining: 85,
            codexReset: "2026-09-04 08:01",
            workbuddyPoints: 5760,
            deepseekBalance: "58.70",
            deepseekCurrency: "CNY",
            deepseekIsOnline: true
        )
        let same = WidgetDisplaySignature(
            codexWeeklyRemaining: 85,
            codexFiveHourRemaining: 85,
            codexReset: "2026-09-04 08:01",
            workbuddyPoints: 5760,
            deepseekBalance: "58.70",
            deepseekCurrency: "CNY",
            deepseekIsOnline: true
        )

        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(previous: nil, current: initial, force: false))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(previous: initial, current: same, force: true))
        XCTAssertFalse(WidgetDisplaySignature.shouldReloadWidget(previous: initial, current: same, force: false))
        let fiveHourOnly = WidgetDisplaySignature(
            codexWeeklyRemaining: nil,
            codexFiveHourRemaining: 85,
            codexReset: "2026-09-04 08:01",
            workbuddyPoints: 5760,
            deepseekBalance: "58.70",
            deepseekCurrency: "CNY",
            deepseekIsOnline: true
        )
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: fiveHourOnly,
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: fiveHourOnly,
            current: initial,
            force: false
        ))
        XCTAssertFalse(WidgetDisplaySignature.shouldReloadWidget(
            previous: fiveHourOnly,
            current: WidgetDisplaySignature(
                codexWeeklyRemaining: nil,
                codexFiveHourRemaining: 85,
                codexReset: "2026-09-04 08:01",
                workbuddyPoints: 5760,
                deepseekBalance: "58.70",
                deepseekCurrency: "CNY",
                deepseekIsOnline: true
            ),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 82, codexFiveHourRemaining: 85, codexReset: "2026-09-04 08:01", workbuddyPoints: 5760, deepseekBalance: "58.70", deepseekCurrency: "CNY", deepseekIsOnline: true),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 85, codexFiveHourRemaining: 80, codexReset: "2026-09-04 08:01", workbuddyPoints: 5760, deepseekBalance: "58.70", deepseekCurrency: "CNY", deepseekIsOnline: true),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 85, codexFiveHourRemaining: 85, codexReset: "2026-09-05 08:01", workbuddyPoints: 5760, deepseekBalance: "58.70", deepseekCurrency: "CNY", deepseekIsOnline: true),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 85, codexFiveHourRemaining: 85, codexReset: "2026-09-04 08:01", workbuddyPoints: 5759, deepseekBalance: "58.70", deepseekCurrency: "CNY", deepseekIsOnline: true),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 85, codexFiveHourRemaining: 85, codexReset: "2026-09-04 08:01", workbuddyPoints: 5760, deepseekBalance: "58.71", deepseekCurrency: "CNY", deepseekIsOnline: true),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 85, codexFiveHourRemaining: 85, codexReset: "2026-09-04 08:01", workbuddyPoints: 5760, deepseekBalance: "58.70", deepseekCurrency: "USD", deepseekIsOnline: true),
            force: false
        ))
        XCTAssertTrue(WidgetDisplaySignature.shouldReloadWidget(
            previous: initial,
            current: WidgetDisplaySignature(codexWeeklyRemaining: 85, codexFiveHourRemaining: 85, codexReset: "2026-09-04 08:01", workbuddyPoints: 5760, deepseekBalance: "58.70", deepseekCurrency: "CNY", deepseekIsOnline: false),
            force: false
        ))

        let onlineResponse = try JSONDecoder().decode(StatusResponse.self, from: Data("""
        {
            "codex": {"five_hour": {"remaining": 85, "reset": "2026-09-04 08:01"}, "weekly": {"remaining": 83, "reset": "2026-09-04 08:01"}},
            "workbuddy": {"points": 5760},
            "deepseek": {"status": "Online", "balances": [{"currency": "CNY", "total_balance": "58.70"}]},
            "system": {"status": "Online"}
        }
        """.utf8))
        let degradedResponse = try JSONDecoder().decode(StatusResponse.self, from: Data("""
        {
            "codex": {"five_hour": {"remaining": 85, "reset": "2026-09-04 08:01"}, "weekly": {"remaining": 83, "reset": "2026-09-04 08:01"}},
            "workbuddy": {"points": 5760},
            "deepseek": {"status": "Online", "balances": [{"currency": "CNY", "total_balance": "58.70"}]},
            "system": {"status": "Degraded"}
        }
        """.utf8))
        XCTAssertFalse(WidgetDisplaySignature.shouldReloadWidget(
            previous: WidgetDisplaySignature(from: onlineResponse),
            current: WidgetDisplaySignature(from: degradedResponse),
            force: false
        ))
    }

    func testMalformedStatusSetsDecodingError() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] _ in try respond(#"{"codex":{"weekly":}}"#) }

        await service.fetchStatus()

        guard case .error = service.state else {
            return XCTFail("expected decoding error, got \(service.state)")
        }
        XCTAssertNotNil(service.errorMessage)
    }

    func testStatusFailureSetsUnavailableState() async {
        let service = makeService()
        MockURLProtocol.handler = { _ in throw URLError(.cannotConnectToHost) }

        await service.fetchStatus()

        guard case .unavailable = service.state else {
            return XCTFail("expected unavailable state, got \(service.state)")
        }
        XCTAssertEqual(service.errorMessage, "Cannot connect to AICC server")
    }

    func testReconnectWorkBuddyUsesFixedEndpointAndReloadsStatus() async throws {
        let service = makeService()
        MockURLProtocol.handler = { [self] request in
            switch request.url?.path {
            case "/api/workbuddy/reconnect":
                XCTAssertEqual(request.httpMethod, "POST")
                return try respond(#"{"ok":true}"#)
            case "/api/status":
                XCTAssertEqual(request.httpMethod, "GET")
                return try respond(Self.statusJSON)
            default:
                XCTFail("unexpected endpoint \(request.url?.path ?? "nil")")
                return try respond("{}", status: 404)
            }
        }

        await service.reconnectWorkBuddy()

        XCTAssertEqual(MockURLProtocol.requests.map { $0.url?.path }, [
            "/api/workbuddy/reconnect",
            "/api/status"
        ])
        guard case .ready = service.state else {
            return XCTFail("expected ready state, got \(service.state)")
        }
    }
}
