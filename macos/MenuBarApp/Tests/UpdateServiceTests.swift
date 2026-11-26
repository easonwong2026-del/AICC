import Foundation
import XCTest
@testable import AICCCore

private enum StubUpdateError: Error {
    case failed
}

private final class StubUpdateSession: UpdateURLSession, @unchecked Sendable {
    let result: Result<(Data, URLResponse), Error>
    private let lock = NSLock()
    private var recordedRequests: [URLRequest] = []

    var requests: [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return recordedRequests
    }

    init(result: Result<(Data, URLResponse), Error>) {
        self.result = result
    }

    func data(for request: URLRequest) async throws -> (Data, URLResponse) {
        lock.lock()
        recordedRequests.append(request)
        lock.unlock()
        return try result.get()
    }
}

private final class BlockingUpdateSession: UpdateURLSession, @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var continuation: CheckedContinuation<(Data, URLResponse), Error>?

    var requestCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func data(for _: URLRequest) async throws -> (Data, URLResponse) {
        try await withCheckedThrowingContinuation {
            (continuation: CheckedContinuation<(Data, URLResponse), Error>) in
            lock.lock()
            count += 1
            self.continuation = continuation
            lock.unlock()
        }
    }

    func finish(data: Data, response: URLResponse) {
        lock.lock()
        let active = continuation
        continuation = nil
        lock.unlock()
        active?.resume(returning: (data, response))
    }

    func fail(with error: Error) {
        lock.lock()
        let active = continuation
        continuation = nil
        lock.unlock()
        active?.resume(throwing: error)
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
        XCTAssertEqual(loaded.numericBuild, 4)
        XCTAssertEqual(loaded.displayText, "v2.5.0 (Build 4)")

        let missing = AppVersionInfo(infoDictionary: [:])
        XCTAssertEqual(missing.shortVersion, AppVersionInfo.fallbackValue)
        XCTAssertEqual(missing.build, AppVersionInfo.fallbackValue)
        XCTAssertNil(missing.numericBuild)
        XCTAssertEqual(missing.displayText, AppVersionInfo.fallbackValue)

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
        XCTAssertNotNil(SemanticVersion("1.0.0-0"))
        XCTAssertNil(SemanticVersion("2.5"))
        XCTAssertNil(SemanticVersion("not-a-version"))
        XCTAssertNil(SemanticVersion("2.05.0"))
        // SemVer 2.0.0 §9: numeric prerelease identifiers MUST NOT include leading zeroes.
        XCTAssertNil(SemanticVersion("1.0.0-01"))
        XCTAssertNil(SemanticVersion("1.0.0-alpha.01"))
    }

    func testSemanticVersionComparisonRejectsInvalidInput() {
        XCTAssertEqual(SemanticVersion("2.5.0"), SemanticVersion("v2.5.0"))
        XCTAssertGreaterThan(SemanticVersion("2.5.10")!, SemanticVersion("2.5.9")!)
        XCTAssertNil(SemanticVersion("invalid"))
        XCTAssertTrue(SemanticVersion("2.5.1")! > SemanticVersion("2.5.0")!)
        XCTAssertFalse(SemanticVersion("invalid").map { $0 > SemanticVersion("2.5.0")! } ?? false)
    }

    func testSystemVersionParsingAndComparison() {
        XCTAssertEqual(SystemVersion("14"), SystemVersion(major: 14, minor: 0, patch: 0))
        XCTAssertEqual(SystemVersion("14.0"), SystemVersion(major: 14, minor: 0, patch: 0))
        XCTAssertLessThan(SystemVersion("14.6")!, SystemVersion("15.0")!)
        XCTAssertLessThan(SystemVersion("14.6")!, SystemVersion("14.6.1")!)
        XCTAssertNil(SystemVersion(""))
        XCTAssertNil(SystemVersion("14.x"))
        XCTAssertNil(SystemVersion("14.0.1.2"))
    }

    func testUpdateCheckStateCasesRemainEquatable() {
        let info = UpdateInfo(version: "2.5.1", build: "5")
        XCTAssertEqual(UpdateCheckState.idle, .idle)
        XCTAssertEqual(UpdateCheckState.checking, .checking)
        XCTAssertEqual(UpdateCheckState.upToDate, .upToDate)
        XCTAssertEqual(UpdateCheckState.updateAvailable(info), .updateAvailable(info))
        XCTAssertEqual(
            UpdateCheckState.incompatibleSystem(info, requiredVersion: "15.0"),
            .incompatibleSystem(info, requiredVersion: "15.0")
        )
        XCTAssertEqual(UpdateCheckState.failed("network"), .failed("network"))
        XCTAssertEqual(UpdateCheckState.notConfigured, .notConfigured)
    }

    func testManifestConfigurationOnlyAcceptsHTTPSAndGatesEnvironmentOverride() {
        let https = UpdateManifestConfiguration.manifestURL(
            environment: [UpdateManifestConfiguration.environmentKey: " https://updates.example.test/aicc.json "],
            allowEnvironmentOverride: true
        )
        XCTAssertEqual(https?.scheme, "https")

        let blockedInRelease = UpdateManifestConfiguration.manifestURL(
            environment: [UpdateManifestConfiguration.environmentKey: "https://updates.example.test/aicc.json"],
            allowEnvironmentOverride: false
        )
        XCTAssertNil(blockedInRelease)

        let http = UpdateManifestConfiguration.manifestURL(
            environment: [UpdateManifestConfiguration.environmentKey: "http://updates.example.test/aicc.json"],
            allowEnvironmentOverride: true
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

        XCTAssertEqual(state, .failed("更新源地址无效"))
        XCTAssertTrue(session.requests.isEmpty)
    }

    func testInvalidCurrentVersionFailsBeforeNetworkRequest() async {
        let session = StubUpdateSession(result: .success((Data(), response(url: "https://updates.example.test/manifest.json"))))
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: nil, build: nil),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()

        XCTAssertEqual(state, .failed("当前应用版本不可用"))
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
            currentSystemVersion: SystemVersion(major: 14, minor: 5),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session,
            timeoutInterval: 2
        )

        let state = await service.checkForUpdates()

        guard case .updateAvailable(let info) = state else {
            return XCTFail("Expected an available update, got \(state)")
        }
        XCTAssertEqual(info.version, "2.5.1")
        XCTAssertEqual(info.formattedPublishedDate, "2026-08-03")
        XCTAssertEqual(session.requests.count, 1)
        XCTAssertEqual(session.requests.first?.httpMethod, "GET")
        XCTAssertEqual(session.requests.first?.value(forHTTPHeaderField: "Accept"), "application/json")
        XCTAssertNil(session.requests.first?.value(forHTTPHeaderField: "Authorization"))
        XCTAssertEqual(session.requests.first?.timeoutInterval, 2)
    }

    func testForwardCompatibleManifestIgnoresUnknownFieldsAndParsesOptionalMetadata() async throws {
        let data = try JSONSerialization.data(withJSONObject: [
            "version": "2.6.0",
            "build": "30",
            "minimumSystemVersion": "14.0",
            "downloadURL": "https://github.com/easonwong2026-del/AICC/releases/download/v2.6.0/AICC-2.6.0.dmg",
            "releaseNotesURL": "https://github.com/easonwong2026-del/AICC/releases/tag/v2.6.0",
            "sha256": String(repeating: "a", count: 64),
            "releaseSummary": "修复内存泄漏并新增应用内更新检查",
            "edSignature": "future-signature-field",
            "rolloutPercent": 100
        ])
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://raw.githubusercontent.com/easonwong2026-del/AICC/main/updates/aicc-update.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            currentSystemVersion: SystemVersion(major: 15, minor: 0),
            manifestURL: URL(string: "https://raw.githubusercontent.com/easonwong2026-del/AICC/main/updates/aicc-update.json"),
            session: session
        )

        let state = await service.checkForUpdates()

        guard case .updateAvailable(let info) = state else {
            return XCTFail("Expected forward-compatible update, got \(state)")
        }
        XCTAssertEqual(info.version, "2.6.0")
        XCTAssertEqual(info.sha256, String(repeating: "a", count: 64))
        XCTAssertEqual(info.releaseSummary, "修复内存泄漏并新增应用内更新检查")
    }

    func testSameVersionHigherBuildReportsUpdateAvailable() async throws {
        let data = Data(#"{"version":"2.14.1","build":"29"}"#.utf8)
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://updates.example.test/manifest.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.14.1", build: "28"),
            currentSystemVersion: SystemVersion(major: 15, minor: 0),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()
        guard case .updateAvailable(let info) = state else {
            return XCTFail("Expected build update to be available, got \(state)")
        }
        XCTAssertEqual(info.build, "29")
    }

    func testMinimumSystemVersionBlocksIncompatibleOS() async throws {
        let data = Data(#"{"version":"3.0.0","build":"40","minimumSystemVersion":"16.0"}"#.utf8)
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://updates.example.test/manifest.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.14.1", build: "28"),
            currentSystemVersion: SystemVersion(major: 15, minor: 2),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()
        guard case .incompatibleSystem(let info, let required) = state else {
            return XCTFail("Expected incompatibleSystem state, got \(state)")
        }
        XCTAssertEqual(info.version, "3.0.0")
        XCTAssertEqual(required, "16.0")
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

    func testHTTPStatusInvalidJSONAndOversizedPayloadBecomeFailedStates() async throws {
        let statusSession = StubUpdateSession(
            result: .success((Data(), response(url: "https://updates.example.test/manifest.json", status: 503)))
        )
        let statusService = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: statusSession
        )
        let statusState = await statusService.checkForUpdates()
        XCTAssertEqual(statusState, .failed("更新服务器返回 HTTP 503"))

        let invalidSession = StubUpdateSession(
            result: .success((Data(#"{"version":2.5}"#.utf8), response(url: "https://updates.example.test/manifest.json")))
        )
        let invalidService = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: invalidSession
        )
        let invalidState = await invalidService.checkForUpdates()
        XCTAssertEqual(invalidState, .failed("更新清单格式无效"))

        let oversizedData = Data(repeating: 0x20, count: UpdateService.maximumManifestBytes + 1)
        let oversizedSession = StubUpdateSession(
            result: .success((oversizedData, response(url: "https://updates.example.test/manifest.json")))
        )
        let oversizedService = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: oversizedSession
        )
        let oversizedState = await oversizedService.checkForUpdates()
        XCTAssertEqual(oversizedState, .failed("更新清单体积超出限制"))
    }

    func testUntrustedResponseHostIsRejected() async throws {
        let data = Data(#"{"version":"2.6.0"}"#.utf8)
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://evil.attacker.example/manifest.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()
        XCTAssertEqual(state, .failed("更新源地址无效"))
    }

    func testRedirectDelegateEnforcesHTTPSAndTrustedHosts() {
        let allowed = UpdateManifestConfiguration.trustedHosts(
            for: URL(string: "https://updates.example.test/manifest.json")
        )
        let delegate = HTTPSOnlyRedirectDelegate(allowedHosts: allowed)
        let dummySession = URLSession(configuration: .ephemeral)
        defer { dummySession.invalidateAndCancel() }
        let task = dummySession.dataTask(with: URL(string: "https://updates.example.test/manifest.json")!)
        let redirectResponse = response(url: "https://updates.example.test/manifest.json", status: 302)

        func evaluateRedirect(_ urlString: String) -> URLRequest? {
            var captured: URLRequest?
            let exp = expectation(description: "redirect \(urlString)")
            delegate.urlSession(
                dummySession,
                task: task,
                willPerformHTTPRedirection: redirectResponse,
                newRequest: URLRequest(url: URL(string: urlString)!)
            ) { result in
                captured = result
                exp.fulfill()
            }
            wait(for: [exp], timeout: 1)
            return captured
        }

        XCTAssertNotNil(evaluateRedirect("https://github.com/easonwong2026-del/AICC/releases"))
        XCTAssertNotNil(evaluateRedirect("https://updates.example.test/v2.json"))
        XCTAssertNil(evaluateRedirect("http://github.com/easonwong2026-del/AICC/releases"))
        XCTAssertNil(evaluateRedirect("https://untrusted.example.net/manifest.json"))
    }

    func testTimeoutIsReportedAndNoRetryIsStarted() async {
        let session = StubUpdateSession(result: .failure(URLError(.timedOut)))
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session
        )

        let state = await service.checkForUpdates()
        XCTAssertEqual(state, .failed("检查更新超时，请稍后重试"))
        XCTAssertEqual(session.requests.count, 1)
    }

    func testBackgroundCheckThrottlesAndRespectsIgnoreVersion() async throws {
        let suiteName = "com.aieink.dashboard.tests.update.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }

        var clock = Date(timeIntervalSince1970: 1_700_000_000)
        let data = Data(#"{"version":"2.6.0","build":"30"}"#.utf8)
        let session = StubUpdateSession(
            result: .success((data, response(url: "https://updates.example.test/manifest.json")))
        )
        let service = UpdateService(
            currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
            currentSystemVersion: SystemVersion(major: 15, minor: 0),
            manifestURL: URL(string: "https://updates.example.test/manifest.json"),
            session: session,
            userDefaults: defaults,
            autoCheckInterval: 3600,
            now: { clock }
        )

        // Disabled auto-check does not fire network call.
        _ = await service.checkForUpdatesIfDue(autoCheckEnabled: false)
        XCTAssertEqual(session.requests.count, 0)

        // First due check runs and finds 2.6.0.
        _ = await service.checkForUpdatesIfDue(autoCheckEnabled: true)
        XCTAssertEqual(session.requests.count, 1)
        XCTAssertEqual(service.visibleUpdate?.version, "2.6.0")

        // Second check within interval is throttled.
        clock = clock.addingTimeInterval(600)
        _ = await service.checkForUpdatesIfDue(autoCheckEnabled: true)
        XCTAssertEqual(session.requests.count, 1)

        // Ignoring hides visibleUpdate while keeping availableUpdate.
        let update = try XCTUnwrap(service.availableUpdate)
        service.ignoreUpdate(update)
        XCTAssertNil(service.visibleUpdate)
        XCTAssertEqual(service.availableUpdate?.version, "2.6.0")

        // Manual check clears ignored version so the user sees the update again.
        _ = await service.checkForUpdates(manual: true)
        XCTAssertEqual(session.requests.count, 2)
        XCTAssertEqual(service.visibleUpdate?.version, "2.6.0")
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

    func testOwnedSessionDeallocatesWithUpdateService() {
        weak var weakService: UpdateService?
        autoreleasepool {
            let service = UpdateService(
                currentVersion: AppVersionInfo(shortVersion: "2.5.0", build: "4"),
                manifestURL: URL(string: "https://updates.example.test/manifest.json")
            )
            weakService = service
            XCTAssertNotNil(weakService)
        }
        XCTAssertNil(weakService)
    }

    func testRepositoryUpdateManifestDecodesCleanly() throws {
        let testFile = URL(fileURLWithPath: #filePath)
        let repoRoot = testFile
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // MenuBarApp
            .deletingLastPathComponent() // macos
            .deletingLastPathComponent() // repo root
        let manifestURL = repoRoot.appendingPathComponent("updates/aicc-update.json")
        let data = try Data(contentsOf: manifestURL)
        let info = try JSONDecoder().decode(UpdateInfo.self, from: data)
        XCTAssertNotNil(SemanticVersion(info.version))
        if let minSys = info.minimumSystemVersion {
            XCTAssertNotNil(SystemVersion(minSys))
        }
        XCTAssertNotNil(info.downloadURL)
        XCTAssertNotNil(info.releaseNotesURL)
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
