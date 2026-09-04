import Foundation
import XCTest
@testable import AICCCore

private final class RecordingProcessRunner: ProcessRunning {
    struct Call {
        let executable: String
        let arguments: [String]
    }

    var calls: [Call] = []
    var results: [Result<ProcessRunnerResult, Error>] = []

    func run(
        executable: String,
        arguments: [String],
        environment: [String: String]?,
        currentDirectoryURL: URL?,
        timeout: TimeInterval
    ) async throws -> ProcessRunnerResult {
        calls.append(Call(executable: executable, arguments: arguments))
        guard !results.isEmpty else {
            throw ProcessRunnerError.launchFailed("missing test result")
        }
        return try results.removeFirst().get()
    }
}

@MainActor
final class OpenCodexQuotaTests: XCTestCase {
    private var previousCustomPath: String?

    override func setUp() {
        super.setUp()
        previousCustomPath = UserDefaults.standard.string(forKey: "ocxCustomPath")
        AppSettings.shared.ocxCustomPath = "/bin/sh"
    }

    override func tearDown() {
        if let previousCustomPath {
            AppSettings.shared.ocxCustomPath = previousCustomPath
        } else {
            AppSettings.shared.ocxCustomPath = ""
            UserDefaults.standard.removeObject(forKey: "ocxCustomPath")
        }
        super.tearDown()
    }

    func testParserConvertsUsedPercentAndIgnoresOrderAndUnknownFields() throws {
        let response = try OCXProviderQuotaResponse(jsonData: Data("""
        {
          "generatedAt": 1788000000,
          "futureField": true,
          "reports": [
            {"provider": "other", "quota": {"customWindows": []}},
            {
              "provider": "google-antigravity",
              "label": "Google Antigravity",
              "source": "fixture",
              "unknown": {"ignored": true},
              "quota": {
                "customWindows": [
                  {"label": "Cla", "percent": "51", "resetAt": 1788003600, "extra": 1},
                  {"label": "Gem", "percent": 22, "resetAt": "1788001800"}
                ],
                "updatedAt": 1788000000
              }
            }
          ]
        }
        """.utf8))

        let quota = try XCTUnwrap(OCXProviderQuotaParser.googleQuota(from: response))
        XCTAssertEqual(quota.gem?.remainingPercent, 78)
        XCTAssertEqual(quota.cla?.remainingPercent, 49)
        XCTAssertEqual(quota.gem?.resetAt?.timeIntervalSince1970, 1788001800)
        XCTAssertEqual(quota.cla?.resetAt?.timeIntervalSince1970, 1788003600)
        XCTAssertEqual(quota.updatedAt?.timeIntervalSince1970, 1788000000)
    }

    func testParserClampsPercentAndAllowsMissingReset() throws {
        let response = try OCXProviderQuotaResponse(jsonData: Data("""
        {"reports":[{"provider":"google-antigravity","quota":{"customWindows":[
          {"label":"Gem","percent":-10},
          {"label":"Cla","percent":150}
        ]}}]}
        """.utf8))

        let quota = try XCTUnwrap(OCXProviderQuotaParser.googleQuota(from: response))
        XCTAssertEqual(quota.gem?.remainingPercent, 100)
        XCTAssertEqual(quota.cla?.remainingPercent, 0)
        XCTAssertNil(quota.gem?.resetAt)
        XCTAssertNil(quota.cla?.resetAt)
    }

    func testParserHandlesMissingReportsWindowsAndLabels() throws {
        let noGoogle = try OCXProviderQuotaResponse(jsonData: Data("{}".utf8))
        XCTAssertNil(OCXProviderQuotaParser.googleQuota(from: noGoogle))

        let noWindows = try OCXProviderQuotaResponse(jsonData: Data("""
        {"reports":[{"provider":"google-antigravity","quota":{}}]}
        """.utf8))
        let partial = try XCTUnwrap(OCXProviderQuotaParser.googleQuota(from: noWindows))
        XCTAssertNil(partial.gem)
        XCTAssertNil(partial.cla)

        let onlyGem = try OCXProviderQuotaResponse(jsonData: Data("""
        {"reports":[{"provider":"google-antigravity","quota":{"customWindows":[
          {"label":"Gem","percent":0}
        ]}}]}
        """.utf8))
        let gemOnly = try XCTUnwrap(OCXProviderQuotaParser.googleQuota(from: onlyGem))
        XCTAssertNotNil(gemOnly.gem)
        XCTAssertNil(gemOnly.cla)
    }

    func testMalformedJSONFailsExplicitly() {
        XCTAssertThrowsError(try OCXProviderQuotaResponse(jsonData: Data("{".utf8)))
    }

    func testProviderQuotaCommandsUseExpectedArguments() {
        let normal = OCXCommandBuilder.providerQuota(ocxPath: "/bin/sh", force: false)
        XCTAssertEqual(normal.executable, "/bin/sh")
        XCTAssertEqual(normal.arguments, ["provider", "quota", "--json"])
        XCTAssertTrue(normal.environmentOverrides.isEmpty)

        let forced = OCXCommandBuilder.providerQuota(ocxPath: "/bin/sh", force: true)
        XCTAssertEqual(forced.arguments, ["provider", "quota", "--refresh", "--json"])
    }

    func testProviderQuotaTTLDoesNotRepeatWithinFiveMinutes() async {
        let runner = RecordingProcessRunner()
        runner.results = [success(quotaJSON)]
        var now = Date(timeIntervalSince1970: 1788000000)
        let controller = makeController(runner, now: { now })

        await controller.refreshProviderQuota()
        now.addTimeInterval(OCXOperationPolicy.providerQuotaTTL - 1)
        await controller.refreshProviderQuota()

        XCTAssertEqual(runner.calls.count, 1)
        XCTAssertEqual(controller.googleQuotaState, .live)
    }

    func testProviderQuotaRefreshesAfterTTLAndForceBypassesTTL() async {
        let runner = RecordingProcessRunner()
        runner.results = [success(quotaJSON), success(quotaJSON), success(quotaJSON)]
        var now = Date(timeIntervalSince1970: 1788000000)
        let controller = makeController(runner, now: { now })

        await controller.refreshProviderQuota()
        now.addTimeInterval(OCXOperationPolicy.providerQuotaTTL)
        await controller.refreshProviderQuota()
        await controller.refreshProviderQuota(force: true)

        XCTAssertEqual(runner.calls.count, 3)
        XCTAssertEqual(runner.calls[0].arguments, ["provider", "quota", "--json"])
        XCTAssertEqual(runner.calls[1].arguments, ["provider", "quota", "--json"])
        XCTAssertEqual(runner.calls[2].arguments, ["provider", "quota", "--refresh", "--json"])
    }

    func testTimeoutAndNonZeroExitAreOptionalQuotaFailures() async {
        let timeoutRunner = RecordingProcessRunner()
        timeoutRunner.results = [.failure(ProcessRunnerError.timedOut(8))]
        let timeoutController = makeController(timeoutRunner)
        await timeoutController.refreshProviderQuota()
        XCTAssertEqual(timeoutController.googleQuotaState, .unavailable)

        let failureRunner = RecordingProcessRunner()
        failureRunner.results = [.failure(ProcessRunnerError.nonZeroExit(
            status: 1,
            stdout: "",
            stderr: "proxy stopped"
        ))]
        let failureController = makeController(failureRunner)
        await failureController.refreshProviderQuota()
        XCTAssertEqual(failureController.googleQuotaState, .unavailable)
    }

    func testLastSuccessfulQuotaIsKeptAndMarkedStaleAfterFailure() async {
        let runner = RecordingProcessRunner()
        runner.results = [
            success(quotaJSON),
            .failure(ProcessRunnerError.nonZeroExit(status: 1, stdout: "", stderr: "failed"))
        ]
        var now = Date(timeIntervalSince1970: 1788000000)
        let controller = makeController(runner, now: { now })

        await controller.refreshProviderQuota()
        let lastSuccess = controller.googleQuotaLastRefresh
        let cached = controller.googleQuota
        now.addTimeInterval(OCXOperationPolicy.providerQuotaTTL)
        await controller.refreshProviderQuota()

        XCTAssertEqual(controller.googleQuotaState, .stale)
        XCTAssertEqual(controller.googleQuota, cached)
        XCTAssertEqual(controller.googleQuotaLastRefresh, lastSuccess)
    }

    func testQuotaFailureDoesNotChangeOpenCodexLifecycleOrCallLifecycleCommands() async {
        let runner = RecordingProcessRunner()
        runner.results = [
            success("opencodex 2.40.0"),
            success("""
            {"proxy":{"running":true,"health":{"ok":true}},"dashboard":{"url":"http://localhost:10100/"}}
            """),
            .failure(ProcessRunnerError.timedOut(8))
        ]
        let controller = makeController(runner)

        await controller.checkOnce()

        XCTAssertEqual(controller.status, .running)
        XCTAssertTrue(runner.calls.allSatisfy { call in
            call.arguments != ["ensure"] && call.arguments != ["stop"]
        })
        XCTAssertTrue(runner.calls.contains { $0.arguments == ["provider", "quota", "--json"] })
    }

    private func makeController(
        _ runner: RecordingProcessRunner,
        now: @escaping () -> Date = { Date() }
    ) -> OpenCodexController {
        OpenCodexController(processRunner: runner, dateProvider: now)
    }

    private func success(_ body: String) -> Result<ProcessRunnerResult, Error> {
        .success(ProcessRunnerResult(
            terminationStatus: 0,
            stdoutData: Data(body.utf8),
            stderrData: Data()
        ))
    }

    private var quotaJSON: String {
        """
        {"reports":[{"provider":"google-antigravity","quota":{"customWindows":[
          {"label":"Gem","percent":22,"resetAt":1788003600},
          {"label":"Cla","percent":51,"resetAt":1788003600}
        ]}}]}
        """
    }
}
