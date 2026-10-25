import Foundation
import XCTest
@testable import AICCCore

/// The footer's page turns travel through the one domain the sandboxed widget
/// may read.  These pin the two properties the extension relies on: an empty
/// domain means "no tap", and a tap carries the stamp it was made at so the
/// provider can drop a repeat.
final class BoardCommandTests: XCTestCase {
    private var defaults: UserDefaults!
    private var suite: String!

    override func setUp() {
        super.setUp()
        suite = "aicc.board.command.tests.\(UUID().uuidString)"
        defaults = UserDefaults(suiteName: suite)
    }

    override func tearDown() {
        defaults.removePersistentDomain(forName: suite)
        defaults = nil
        suite = nil
        super.tearDown()
    }

    func testNoCommandUntilSomeoneTurnsAPage() {
        XCTAssertNil(WidgetPreferences.boardCommand(from: defaults))
    }

    func testRequestKeepsItsDirectionAndStamp() {
        let at = Date(timeIntervalSince1970: 1_792_934_520.86)
        WidgetPreferences.requestBoardStep(1, at: at, to: defaults)

        let command = WidgetPreferences.boardCommand(from: defaults)
        XCTAssertEqual(command?.step, 1)
        XCTAssertEqual(command?.at ?? 0, at.timeIntervalSince1970, accuracy: 0.000_001)
    }

    func testTheLatestRequestWins() {
        WidgetPreferences.requestBoardStep(1, at: Date(timeIntervalSince1970: 1_000), to: defaults)
        let second = Date(timeIntervalSince1970: 1_060)
        WidgetPreferences.requestBoardStep(-1, at: second, to: defaults)

        let command = WidgetPreferences.boardCommand(from: defaults)
        XCTAssertEqual(command?.step, -1)
        XCTAssertEqual(command?.at ?? 0, second.timeIntervalSince1970, accuracy: 0.000_001)
    }

    /// The command shares the display domain with the widget's settings, and
    /// `save` rewrites one key: writing settings must not swallow a pending tap.
    func testSavingDisplaySettingsLeavesTheRequestAlone() {
        WidgetPreferences.requestBoardStep(-1, at: Date(timeIntervalSince1970: 2_000), to: defaults)
        WidgetPreferences.save(port: 4100, cards: .automatic, to: defaults)

        XCTAssertEqual(WidgetPreferences.load(from: defaults).port, 4100)
        XCTAssertEqual(WidgetPreferences.boardCommand(from: defaults)?.step, -1)
    }

    /// A command only counts once; the provider compares the stamp it already
    /// honoured against the incoming one, so an older stamp must not look fresh.
    func testAStaleStampCannotOutrankTheHonouredOne() {
        WidgetPreferences.requestBoardStep(1, at: Date(timeIntervalSince1970: 3_000), to: defaults)
        let honoured = WidgetPreferences.boardCommand(from: defaults)?.at ?? 0

        WidgetPreferences.requestBoardStep(1, at: Date(timeIntervalSince1970: 2_900), to: defaults)
        XCTAssertLessThan(WidgetPreferences.boardCommand(from: defaults)?.at ?? 0, honoured)
    }
}
