import XCTest
@testable import AICCCore

final class DashboardTypographyTests: XCTestCase {
    func testPrimaryFontNeverShrinksBelowReadableFloor() {
        XCTAssertEqual(DashboardTypography.primaryFontSize(number: "92"), DashboardTypography.primaryMetricLarge)
        XCTAssertEqual(DashboardTypography.primaryFontSize(number: "5,343.37"), 34)
        XCTAssertEqual(DashboardTypography.primaryFontSize(number: "999,999.99"), 32)
        XCTAssertGreaterThanOrEqual(DashboardTypography.primaryFontSize(number: "9,876,543,210"), 30)
        XCTAssertGreaterThanOrEqual(DashboardTypography.primaryFontSize(number: "9,876,543,210", compact: true), 24)
    }
}
