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
