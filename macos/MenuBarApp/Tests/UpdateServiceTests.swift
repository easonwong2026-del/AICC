import Foundation
import XCTest
@testable import AICCCore

private enum StubUpdateError: Error {
    case failed
}

private final class StubUpdateSession: UpdateURLSession {
    let result: Result<(Data, URLResponse), Error>
    private(set) var requests: [URLRequest] = []

    init(result: Result<(Data, URLResponse), Error>) {
        self.result = result
    }

    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        requests.append(request)
        return try result.get()
    }
}

private final class BlockingUpdateSession: UpdateURLSession {
    private(set) var requestCount = 0
    private var continuation: CheckedContinuation<(Data, URLResponse), Error>?

    func data(for _: URLRequest) async throws -> (Data, URLResponse) {
        requestCount += 1
        return try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<(Data, URLResponse), Error>) in
            self.continuation = continuation
        }
    }

    func finish(data: Data, response: URLResponse) {
        continuation?.resume(returning: (data, response))
        continuation = nil
    }
}

@MainActor
final class UpdateServiceTests: XCTestCase {
    func testAppVersionInfoReadsBundleKeysAndFallsBackSafely() {
        let loaded = AppVersionInfo(infoDictionary: [
            "CFBundleShortVersionString": "2.5.0",
            "CFBundleVersion": "4"
        ])
        XCTAssertEqual(loaded.shortVersion, "2.5.0")
        XCTAssertEqual(loaded.version, "2.5.0")
        XCTAssertEqual(loaded.build, "4")
        XCTAssertEqual(loaded.buildVersion, "4")

        let missing = AppVersionInfo(infoDictionary: [:])
        XCTAssertEqual(missing.shortVersion, AppVersionInfo.fallbackValue)
        XCTAssertEqual(missing.build, AppVersionInfo.fallbackValue)

        let blank = AppVersionInfo(shortVersion: "  ", build: "")
        XCTAssertEqual(blank, missing)
    }

    func testSemanticVersionComparisonHandlesNumericAndPrereleaseOrdering() {
        XCTAssertLessThan(SemanticVersion("2.5.0")!, SemanticVersion("2.5.1")!)
        XCTAssertLessThan(SemanticVersion("2.5.9")!, SemanticVersion("2.5.10")!)
        XCTAssertEqual(SemanticVersion("2.5.0")!, SemanticVersion("v2.5.0")!)
        XCTAssertEqual(SemanticVersion("V2.5.0")!, SemanticVersion("2.5.0")!)
        XCTAssertGreaterThan(SemanticVersion("2.6.0")!, SemanticVersion("2.5.10")!)
        XCTAssertLessThan(SemanticVersion("2.5.1-beta")!, SemanticVersion("2.5.1")!)
        XCTAssertLessThan(SemanticVersion("2.5.1-alpha.9")!, SemanticVersion("2.5.1-alpha.10")!)
        XCTAssertLessThan(SemanticVersion("2.5.1-alpha")!, SemanticVersion("2.5.1-beta")!)
        XCTAssertEqual(SemanticVersion("2.5.0+build-1")!, SemanticVersion("2.5.0+build-2")!)
        XCTAssertNil(SemanticVersion("2.5"))
        XCTAssertNil(SemanticVersion("not-a-version"))
        XCTAssertNil(SemanticVersion("2.05.0"))
    }

    func testSemanticVersionComparisonRejectsInvalidInput() {
        XCTAssertEqual(SemanticVersion("2.5.0"), SemanticVersion("v2.5.0"))
        XCTAssertGreaterThan(SemanticVersion("2.5.10")!, SemanticVersion("2.5.9")!)
        XCTAssertNil(SemanticVersion("invalid"))
        XCTAssertTrue(SemanticVersion("2.5.1")! > SemanticVersion("2.5.0")!)
        XCTAssertFalse(SemanticVersion("invalid").map { $0 > SemanticVersion("2.5.0")! } ?? false)
    }

    func testUpdateCheckStateCasesRemainEquatable() {
        let info = UpdateInfo(version: "2.5.1", build: "5")
        XCTAssertEqual(UpdateCheckState.idle, .idle)
        XCTAssertEqual(UpdateCheckState.checking, .checking)
        XCTAssertEqual(UpdateCheckState.upToDate, .upToDate)
        XCTAssertEqual(UpdateCheckState.updateAvailable(info), .updateAvailable(info))
        XCTAssertEqual(UpdateCheckState.failed("network"), .failed("network"))
        XCTAssertEqual(UpdateCheckState.notConfigured, .notConfigured)
    }

    func testManifestConfigurationOnlyAcceptsHTTPSAndSupportsEnvironmentOverride() {
        let https = UpdateManifestConfiguration.manifestURL(
            environment: [UpdateManifestConfiguration.environmentKey: " https://updates.example.test/aicc.json "]
        )
        XCTAssertEqual(https?.scheme, "https")

        let http = UpdateManifestConfiguration.manifestURL(
            environment: [UpdateManifestConfiguration.environmentKey: "http://updates.example.test/aicc.json"]
        )
        XCTAssertNil(http)
        XCTAssertNil(UpdateManifestConfiguration.httpsURL("https://user:secret@example.test/manifest.json"))
    }

    func testNotConfiguredDoesNotMakeARequest() async {
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: nil
        )

        XCTAssertEqual(service.state, .notConfigured)
        let state = await service.checkForUpdates()

        XCTAssertEqual(state, .notConfigured)
        XCTAssertEqual(service.state, .notConfigured)
    }

    func testNonHTTPSManifestIsRejectedBeforeNetworkRequest() async {
        let session = StubUpdateSession(result: .success((Data(), response(url: "https://updates.example.test/manifest.json"))))
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "http://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()

        XCTAssertEqual(state, .failed("Invalid update source."))
        XCTAssertTrue(session.requests.isEmpty)
    }

    func testSuccessfulManifestReportsUpdateAvailableWithoutAuthorization() async throws {
        let data = try JSONSerialization.data(withJSONObject: [
            "version": "2.5.1",
            "build": "5",
            "minimumSystemVersion": "14.0",
            "downloadURL": "https://updates.example.test/AICC-2.5.1.dmg",
            "releaseNotesURL": "https://updates.example.test/releases/2.5.1",
            "publishedAt": "2026-08-03T00:00:00Z"
        ])
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://updates.example.test/manifest.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session,
            timeoutInterval: 2
        )

        let state = await service.checkForUpdates()

        guard case .updateAvailable(let info) = state else {
            return XCTFail("Expected an available update, got \(state)")
        }
        XCTAssertEqual(info.version, "2.5.1")
        XCTAssertEqual(session.requests.count, 1)
        XCTAssertEqual(session.requests.first?.httpMethod, "GET")
        XCTAssertEqual(session.requests.first?.value(forHTTPHeaderField: "Accept"), "application/json")
        XCTAssertNil(session.requests.first?.value(forHTTPHeaderField: "Authorization"))
        XCTAssertEqual(session.requests.first?.timeoutInterval, 2)
    }

    func testCurrentVersionReportsUpToDateForStableAndPrereleaseManifest() async throws {
        let data = Data(#"{"version":"2.5.0-beta"}"#.utf8)
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://updates.example.test/manifest.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()

        XCTAssertEqual(state, .upToDate)
    }

    func testHTTPStatusAndInvalidJSONBecomeFailedStates() async throws {
        let statusSession = StubUpdateSession(
            result: .success((Data(), response(url: "https://updates.example.test/manifest.json", status: 503)))
        )
        let statusService = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: statusSession
        )
        let statusState = await statusService.checkForUpdates()
        XCTAssertEqual(statusState, .failed("Update server returned HTTP 503."))

        let invalidSession = StubUpdateSession(
            result: .success((Data(#"{"version":2.5}"#.utf8), response(url: "https://updates.example.test/manifest.json")))
        )
        let invalidService = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: invalidSession
        )
        let invalidState = await invalidService.checkForUpdates()
        XCTAssertEqual(invalidState, .failed("Invalid update manifest."))

        let unknownFieldSession = StubUpdateSession(
            result: .success((Data(#"{"version":"2.5.1","unexpected":true}"#.utf8), response(url: "https://updates.example.test/manifest.json")))
        )
        let unknownFieldService = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: unknownFieldSession
        )
        let unknownFieldState = await unknownFieldService.checkForUpdates()
        XCTAssertEqual(unknownFieldState, .failed("Invalid update manifest."))
    }

    func testTimeoutIsReportedAndNoRetryIsStarted() async {
        let session = StubUpdateSession(result: .failure(URLError(.timedOut)))
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()
        XCTAssertEqual(state, .failed("Update check timed out."))
        XCTAssertEqual(session.requests.count, 1)
    }

    func testCheckingDoesNotStartASecondConcurrentRequest() async {
        let session = BlockingUpdateSession()
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let firstCheck = Task { await service.checkForUpdates() }
        while session.requestCount == 0 {
            await Task.yield()
        }

        XCTAssertEqual(service.state, .checking)
        let duplicateState = await service.checkForUpdates()
        XCTAssertEqual(duplicateState, .checking)
        XCTAssertEqual(session.requestCount, 1)

        session.finish(
            data: Data(#"{"version":"2.5.0"}"#.utf8),
            response: response(url: "https://updates.example.test/manifest.json")
        )
        let firstState = await firstCheck.value
        XCTAssertEqual(firstState, .upToDate)
    }

    private func response(url: String, status: Int = 200) -> HTTPURLResponse {
        HTTPURLResponse(
            url: URL(string: url)!,
            statusCode: status,
            httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "application/json"]
        )!
    }
}
