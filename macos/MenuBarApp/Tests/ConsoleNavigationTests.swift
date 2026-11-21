import XCTest
@testable import AICCCore

/// 真实导航状态机的消费语义（#115 复审 P1）。
///
/// 总览点「去处理」= `openAccounts` + `requestRotationSheet`；
/// 账号页首次挂载与后续变化都调用 `consumeRotationSheetRequest()`。
/// 这里直接驱动同一个类型，覆盖"先请求后挂载"与"只消费一次"。
final class ConsoleNavigationTests: XCTestCase {
    @MainActor func testOverviewRecoveryRouteRequestsRotationAndConsumesOnce() {
        let nav = ConsoleNavigation(page: .overview)
        nav.openAccounts(filter: .workbuddy)
        nav.requestRotationSheet()

        XCTAssertEqual(nav.page, .accounts)
        XCTAssertEqual(nav.accountsFilter, .workbuddy)
        XCTAssertTrue(nav.openRotationSheet)

        // onAppear 或 onChange 任一先到，第一次消费成功。
        XCTAssertTrue(nav.consumeRotationSheetRequest())
        XCTAssertFalse(nav.openRotationSheet)

        // 同一次请求只开一次面板；第二次消费是空操作。
        XCTAssertFalse(nav.consumeRotationSheetRequest())
    }

    @MainActor func testRequestRaisedBeforeFirstMountIsStillConsumedOnAppear() {
        let nav = ConsoleNavigation(page: .overview)
        // 模拟：请求先于账号页视图创建（首次挂载时值已经是 true）。
        nav.requestRotationSheet()
        XCTAssertTrue(nav.openRotationSheet)

        XCTAssertTrue(nav.consumeRotationSheetRequest(), "首次挂载必须消费已经存在的请求")
        XCTAssertFalse(nav.openRotationSheet)
        XCTAssertFalse(nav.consumeRotationSheetRequest())
    }

    @MainActor func testConsumeWithoutRequestDoesNothing() {
        let nav = ConsoleNavigation(page: .overview)
        XCTAssertFalse(nav.consumeRotationSheetRequest())
        XCTAssertFalse(nav.openRotationSheet)
        XCTAssertEqual(nav.page, .overview)
    }

    /// 「去处理」只切页 + 发请求，不改变任何恢复/登录状态：导航类型没有写入口。
    @MainActor func testRecoveryRouteDoesNotTouchAccountSelectionWhenNoneProvided() {
        let nav = ConsoleNavigation(page: .overview)
        nav.accountSelection = .qoderProfile("profile-1")
        nav.openAccounts(filter: .workbuddy)
        nav.requestRotationSheet()

        XCTAssertNil(nav.accountSelection, "与筛选不匹配的选中项被清空，但仍不写业务状态")
        XCTAssertEqual(nav.page, .accounts)
    }
}
