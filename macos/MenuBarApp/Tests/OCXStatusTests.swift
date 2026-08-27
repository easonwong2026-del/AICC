@testable import AICCCore
import Foundation
import XCTest

/// Test vectors are kept outside Sources because the production build uses a
/// recursive Swift source collector. The future XCTest target can compile this
/// file together with the app sources and call the pure parser directly.
final class OCXStatusTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures", isDirectory: true)
            .appendingPathComponent(name)
        return try Data(contentsOf: url)
    }

    func testRunningStatusPreservesDynamicDetails() throws {
        let snapshot = try OCXSnapshot(jsonData: fixture("ocx-running.json"))
        XCTAssertEqual(snapshot.resolvedStatus, .running)
        XCTAssertEqual(snapshot.pid, 4321)
        XCTAssertEqual(snapshot.port, 12001)
        XCTAssertEqual(snapshot.dashboardURL?.host, "localhost")
        XCTAssertEqual(snapshot.healthURL?.port, 12001)
    }

    func testStoppedAndUnhealthyMappings() throws {
        XCTAssertEqual(try OCXSnapshot(jsonData: fixture("ocx-stopped.json")).resolvedStatus, .stopped)
        XCTAssertEqual(try OCXSnapshot(jsonData: fixture("ocx-unhealthy.json")).resolvedStatus, .unhealthy)
    }

    func testMissingHealthMappingDependsOnRunningState() throws {
        XCTAssertEqual(
            try OCXSnapshot(jsonData: fixture("ocx-running-missing-health.json")).resolvedStatus,
            .unhealthy
        )
        XCTAssertEqual(
            try OCXSnapshot(jsonData: fixture("ocx-stopped-missing-health.json")).resolvedStatus,
            .stopped
        )
    }

    func testDynamicPortAndLossyNumbers() throws {
        let snapshot = try OCXSnapshot(jsonData: fixture("ocx-dynamic-port.json"))
        XCTAssertEqual(snapshot.resolvedStatus, .running)
        XCTAssertEqual(snapshot.pid, 24680)
        XCTAssertEqual(snapshot.port, 18888)
        XCTAssertEqual(snapshot.dashboardURL?.scheme, "https")
    }

    func testMissingDashboardDisablesDashboardAction() throws {
        let snapshot = try OCXSnapshot(jsonData: fixture("ocx-missing-dashboard.json"))
        XCTAssertEqual(snapshot.resolvedStatus, .running)
        XCTAssertFalse(snapshot.hasValidDashboardURL)
    }

    func testInvalidHTTPURLsAreNotExposedAsDashboardOrHealthURLs() throws {
        let snapshot = try OCXSnapshot(jsonData: fixture("ocx-invalid-dashboard.json"))
        XCTAssertEqual(snapshot.resolvedStatus, .running)
        XCTAssertNil(snapshot.dashboardURL)
        XCTAssertNil(snapshot.healthURL)
    }

    func testUnknownFieldsAreIgnored() throws {
        XCTAssertEqual(try OCXSnapshot(jsonData: fixture("ocx-unknown-fields.json")).resolvedStatus, .stopped)
    }

    func testRuntimeVersionAndCLIVersionAreSeparate() throws {
        let snapshot = try OCXSnapshot(jsonData: fixture("ocx-running.json"))
        XCTAssertEqual(snapshot.version, "0.146.0")
        XCTAssertEqual(OCXVersionParser.parse("opencodex 2.7.42\n"), "opencodex 2.7.42")
        XCTAssertEqual(
            OCXVersionParser.semanticVersion(from: "opencodex 2.7.42\n")?.description,
            "2.7.42"
        )
        XCTAssertEqual(OCXVersionParser.semanticVersion(from: "2.7.42\n")?.description, "2.7.42")
        XCTAssertNil(OCXVersionParser.parse(" \n\r\n"))
    }

    func testMissingRequiredAndBadJSONAreSafeFailures() throws {
        XCTAssertThrowsError(try OCXSnapshot(jsonData: fixture("ocx-missing-required.json")))
        XCTAssertThrowsError(try OCXSnapshot(jsonData: fixture("ocx-bad-json.json")))
    }

    func testStrictStateSetAndTogglePolicy() {
        XCTAssertTrue(OCXStatus.running.isRunning)
        XCTAssertTrue(OCXStatus.unhealthy.isToggleOn)
        XCTAssertFalse(OCXStatus.checking.isToggleOn)
        XCTAssertTrue(OCXStatus.starting.isBusy)
        XCTAssertTrue(OCXStatus.stopping.isBusy)
    }
}
