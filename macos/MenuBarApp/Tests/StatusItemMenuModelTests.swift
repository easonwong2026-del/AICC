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

    func testStatusItemClickRouterSeparatesLeftAndRightButtons() {
        XCTAssertEqual(StatusItemClickRouter.action(for: .left), .dashboard)
        XCTAssertEqual(StatusItemClickRouter.action(for: .right), .contextMenu)
    }
}
