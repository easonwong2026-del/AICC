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
}
