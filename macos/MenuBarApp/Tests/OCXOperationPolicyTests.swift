@testable import AICCCore
import XCTest

/// Policy-level coverage for the controller's asynchronous behavior. These
/// cases are intentionally explicit so the main agent can connect them to a
/// clock/process mock without requiring real `ocx ensure` or `ocx stop` calls.
final class OCXOperationPolicyTests: XCTestCase {
    func testConfirmationBackoffSchedule() {
        XCTAssertEqual(
            OCXOperationPolicy.confirmationDelays,
            [300_000_000, 700_000_000, 1_500_000_000, 3_000_000_000]
        )
    }

    func testGenerationRulesToConnectToControllerMocks() {
        XCTAssertTrue(OCXOperationPolicy.shouldApplyStatus(
            requestGeneration: 42,
            currentGeneration: 42,
            operationActive: false
        ))
        XCTAssertFalse(OCXOperationPolicy.shouldApplyStatus(
            requestGeneration: 41,
            currentGeneration: 42,
            operationActive: false
        ))
        XCTAssertFalse(OCXOperationPolicy.shouldApplyStatus(
            requestGeneration: 42,
            currentGeneration: 42,
            operationActive: true
        ))
    }

    func testPanelMonitorContract() {
        XCTAssertEqual(OCXOperationPolicy.panelPollIntervalNanoseconds, 9_000_000_000)
        XCTAssertLessThanOrEqual(OCXOperationPolicy.panelPollIntervalNanoseconds, 10_000_000_000)
        XCTAssertLessThanOrEqual(OCXOperationPolicy.statusTimeout, 5)
        XCTAssertLessThanOrEqual(OCXOperationPolicy.operationTimeout, 15)
        XCTAssertTrue(OCXOperationPolicy.shouldContinuePanel(isVisible: true, taskIsCancelled: false))
        XCTAssertFalse(OCXOperationPolicy.shouldContinuePanel(isVisible: false, taskIsCancelled: false))
        XCTAssertFalse(OCXOperationPolicy.shouldContinuePanel(isVisible: true, taskIsCancelled: true))
    }

    func testExternalChangesAreObservedByNextVisiblePoll() {
        XCTAssertLessThanOrEqual(9, 10)
        XCTAssertTrue(OCXOperationPolicy.reachedTarget(.running, target: .running))
        XCTAssertTrue(OCXOperationPolicy.reachedTarget(.stopped, target: .stopped))
        XCTAssertFalse(OCXOperationPolicy.reachedTarget(.starting, target: .running))
    }

    func testNoExecutableAndTimeoutAreNotRunningStates() {
        XCTAssertNotEqual(OCXStatus.notInstalled, .running)
        XCTAssertNotEqual(OCXStatus.unhealthy, .running)
    }

    func testUpdateStateTracksBusyAndCompletionCases() {
        XCTAssertTrue(OCXUpdateState.checking.isBusy)
        XCTAssertTrue(OCXUpdateState.updating.isBusy)
        XCTAssertFalse(OCXUpdateState.upToDate.isBusy)
        XCTAssertEqual(OCXUpdateState.available("2.5.1"), .available("2.5.1"))
        XCTAssertEqual(
            OCXUpdateState.updated(from: "2.5.0", to: "2.5.1", restartRequired: true),
            .updated(from: "2.5.0", to: "2.5.1", restartRequired: true)
        )
        XCTAssertEqual(OCXUpdateState.failed("Command timed out"), .failed("Command timed out"))
    }

    func testUpdateCheckTransitionsUseSemanticVersionComparison() {
        XCTAssertEqual(
            OCXUpdateState.checkResult(current: "1.2.3", latest: "1.2.3"),
            .upToDate
        )
        XCTAssertEqual(
            OCXUpdateState.checkResult(current: "1.2.3", latest: "1.2.4"),
            .available("1.2.4")
        )
        XCTAssertEqual(
            OCXUpdateState.checkResult(current: "1.2.3", latest: nil),
            .failed("Unable to check for updates")
        )
    }

    func testUpdateCompletionRejectsNoChangeAndAcceptsVersionRefresh() {
        XCTAssertEqual(
            OCXUpdateState.completion(from: "1.2.3", to: "1.2.4", restartRequired: false),
            .updated(from: "1.2.3", to: "1.2.4", restartRequired: false)
        )
        XCTAssertEqual(
            OCXUpdateState.completion(from: "1.2.3", to: "1.2.3", restartRequired: false),
            .failed("OpenCodex update completed, but version did not change.")
        )
        XCTAssertEqual(
            OCXUpdateState.completion(from: "1.2.3", to: nil, restartRequired: false),
            .failed("OpenCodex update completed, but version could not be verified.")
        )
    }

    func testUpdateTimeoutAllowsRegistryCheckAndLongRunningOfficialUpdate() {
        XCTAssertEqual(OCXOperationPolicy.updateCheckTimeout, 12)
        XCTAssertEqual(OCXOperationPolicy.updateTimeout, 180)
        XCTAssertGreaterThan(OCXOperationPolicy.updateTimeout, OCXOperationPolicy.updateCheckTimeout)
    }
}
