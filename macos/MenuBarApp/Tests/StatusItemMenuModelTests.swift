import XCTest
@testable import AICCCore

final class StatusItemMenuModelTests: XCTestCase {
    func testMenuContractHasExactlyFourCommandsInOrder() {
        XCTAssertEqual(
            StatusItemMenuCommand.allCases.map(\.rawValue),
            [
                "Open AICC Dashboard",
                "Refresh All Now",
                "Settings…",
                "Quit AICC"
            ]
        )
    }
}
