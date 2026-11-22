import XCTest
@testable import AICCCore

/// 真实导航状态机的消费语义（#115 复审 P1，本 PR 扩到三个入口）。
///
/// 总览点「去处理」= `openAccounts` + `requestRotationManager(.recovery)`；
/// 账号页、任务中心首次挂载与后续变化都调用 `consumeRotationManagerRequest()`。
/// 这里直接驱动同一个类型，覆盖"先请求后挂载"与"只消费一次"。
final class ConsoleNavigationTests: XCTestCase {
    @MainActor func testOverviewRecoveryRouteRequestsRotationAndConsumesOnce() {
        let nav = ConsoleNavigation(page: .overview)
        nav.openAccounts(filter: .workbuddy)
        nav.requestRotationManager(.recovery)

        XCTAssertEqual(nav.page, .accounts)
        XCTAssertEqual(nav.accountsFilter, .workbuddy)
        XCTAssertEqual(nav.rotationRequest?.focus, .recovery)

        // onAppear 或 onChange 任一先到，第一次消费成功。
        let request = nav.consumeRotationManagerRequest()
        XCTAssertEqual(request?.focus, .recovery)
        XCTAssertNil(nav.rotationRequest)

        // 同一次请求只开一次面板；第二次消费是空操作。
        XCTAssertNil(nav.consumeRotationManagerRequest())
    }

    @MainActor func testRequestRaisedBeforeFirstMountIsStillConsumedOnAppear() {
        let nav = ConsoleNavigation(page: .overview)
        // 模拟：请求先于账号页视图创建（首次挂载时值已经存在）。
        nav.requestRotationManager(.plan)
        XCTAssertNotNil(nav.rotationRequest)

        XCTAssertNotNil(nav.consumeRotationManagerRequest(), "首次挂载必须消费已经存在的请求")
        XCTAssertNil(nav.rotationRequest)
        XCTAssertNil(nav.consumeRotationManagerRequest())
    }

    @MainActor func testConsumeWithoutRequestDoesNothing() {
        let nav = ConsoleNavigation(page: .overview)
        XCTAssertNil(nav.consumeRotationManagerRequest())
        XCTAssertNil(nav.rotationRequest)
        XCTAssertEqual(nav.page, .overview)
    }

    /// 「去处理」只切页 + 发请求，不改变任何恢复/登录状态：导航类型没有写入口。
    @MainActor func testRecoveryRouteDoesNotTouchAccountSelectionWhenNoneProvided() {
        let nav = ConsoleNavigation(page: .overview)
        nav.accountSelection = .qoderProfile("profile-1")
        nav.openAccounts(filter: .workbuddy)
        nav.requestRotationManager(.recovery)

        XCTAssertNil(nav.accountSelection, "与筛选不匹配的选中项被清空，但仍不写业务状态")
        XCTAssertEqual(nav.page, .accounts)
    }

    /// 同一次请求的 token 必须变化，否则 `onChange` 无法识别第二次请求。
    @MainActor func testEachRequestCarriesANewToken() {
        let nav = ConsoleNavigation(page: .checkin)
        nav.requestRotationManager(.plan)
        let first = nav.rotationRequest?.token
        nav.requestRotationManager(.history)
        let second = nav.rotationRequest?.token

        XCTAssertNotNil(first)
        XCTAssertNotNil(second)
        XCTAssertNotEqual(first, second)
        XCTAssertEqual(nav.rotationRequest?.focus, .history)
    }

    // MARK: - 任务中心

    @MainActor func testTaskCenterModeSwitchIsNavigationOnly() {
        let nav = ConsoleNavigation(page: .overview)
        XCTAssertEqual(nav.taskCenterMode, .today)

        nav.openCheckin(platform: .workbuddy, workbuddy: .growth, mode: .automation)
        XCTAssertEqual(nav.page, .checkin)
        XCTAssertEqual(nav.taskCenterMode, .automation)
        XCTAssertEqual(nav.workbuddyTask, .growth)

        nav.taskCenterMode = .today
        XCTAssertEqual(nav.taskCenterMode, .today, "切模式只改导航状态")
        XCTAssertNil(nav.rotationRequest, "切模式不产生任何轮换请求")
    }

    @MainActor func testAutomationManageRoutesToExistingTaskDomains() {
        let nav = ConsoleNavigation(page: .checkin)

        nav.openCheckin(platform: .workbuddy, workbuddy: .checkin, mode: .today)
        XCTAssertEqual(nav.checkinPlatform, .workbuddy)
        XCTAssertEqual(nav.workbuddyTask, .checkin)
        XCTAssertEqual(nav.taskCenterMode, .today)

        nav.openCheckin(platform: .qoder, qoder: .credits, mode: .today)
        XCTAssertEqual(nav.checkinPlatform, .qoder)
        XCTAssertEqual(nav.qoderTask, .credits)
        XCTAssertEqual(nav.page, .checkin)

        nav.requestRotationManager(.plan)
        XCTAssertEqual(nav.rotationRequest?.focus, .plan)
        XCTAssertEqual(nav.page, .checkin, "请求轮换管理不切页，由当前页决定是否弹层")
    }

    // MARK: - 签到「去登录」的落点（PR #120 P1-1）

    /// 原来停在别的平台、选着别的账号，也要能定位到这个签到账号。
    @MainActor func testCheckinLoginRouteFindsTheTargetAccountFromAnotherPlatform() {
        let nav = ConsoleNavigation(page: .checkin)
        nav.accountsFilter = .qoder
        nav.accountSelection = .qoderService("qsa-cn-9")

        let accounts = [Self.account("acc-wb-1"), Self.account("acc-wb-2", nickname: "设计外包")]
        let decision = ConsoleCheckinLoginRoute.decision(
            accountId: "acc-wb-2",
            displayName: "设计外包",
            accounts: accounts,
            accountsError: nil
        )
        guard case .open(let filter, let selection) = decision else {
            return XCTFail("应当定位到 WorkBuddy 国内版账号：\(decision)")
        }
        XCTAssertEqual(filter, .workbuddy)
        nav.openAccounts(filter: filter, selection: selection)

        XCTAssertEqual(nav.page, .accounts)
        XCTAssertEqual(nav.accountsFilter, .workbuddy)
        XCTAssertEqual(nav.accountSelection, .workbuddy("acc-wb-2"))
    }

    /// 目标不在列表里：不跳转、不把选择换成列表第一项，并给出可读原因。
    @MainActor func testCheckinLoginRouteDoesNotFallBackToAnotherAccount() {
        let nav = ConsoleNavigation(page: .checkin)
        nav.accountsFilter = .workbuddy
        nav.accountSelection = .workbuddy("acc-wb-1")

        let decision = ConsoleCheckinLoginRoute.decision(
            accountId: "acc-missing",
            displayName: "外包账号",
            accounts: [Self.account("acc-wb-1")],
            accountsError: "账号服务连接失败"
        )
        guard case .blocked(let reason) = decision else {
            return XCTFail("找不到目标时不能给出 selection：\(decision)")
        }
        XCTAssertTrue(reason.contains("外包账号"), reason)
        XCTAssertTrue(reason.contains("账号服务连接失败"), reason)

        // 视图对 .blocked 分支什么都不做：页面与选中项都不能被改掉。
        if case .open(let filter, let selection) = decision {
            nav.openAccounts(filter: filter, selection: selection)
        }
        XCTAssertEqual(nav.page, .checkin)
        XCTAssertEqual(nav.accountSelection, .workbuddy("acc-wb-1"))
    }

    /// id 撞上国际版也不能跳过去：签到只属于国内版。
    func testCheckinLoginRouteRejectsInternationalAccount() {
        let decision = ConsoleCheckinLoginRoute.decision(
            accountId: "acc-ai",
            displayName: "国际版账号",
            accounts: [Self.account("acc-ai", provider: "workbuddy_ai")],
            accountsError: nil
        )
        guard case .blocked(let reason) = decision else {
            return XCTFail("国际版账号没有签到，不能作为登录目标：\(decision)")
        }
        XCTAssertTrue(reason.contains("没有在 AI 账号列表里找到"), reason)
    }

    private static func account(
        _ id: String,
        provider: String = "workbuddy",
        nickname: String = "账号"
    ) -> ConsoleAccount {
        ConsoleAccount(id: id, platform: "workbuddy", provider: provider, uid: "uid-\(id)", nickname: nickname)
    }
}
