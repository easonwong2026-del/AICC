import XCTest
@testable import AICCCore

/// 成长中心只读视图模型的解析与派生（PR #67）。
///
/// 这些断言盯的都是"会让界面说谎"的地方：把没查到显示成 0、把未知任务
/// 显示成自动、把 401 显示成普通错误。CI 上 `swift test` 跑得到。
final class GrowthModelsTests: XCTestCase {

    private func task(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var base: [String: Any] = [
            "code": "create_canvas",
            "title": "创建第一个画布",
            "condition": "在画布中创建一张图",
            "current": 0,
            "target": 1,
            "rewardCredit": 20,
            "rewardEnergy": 0,
            // 真实 JSON 里 status 是枚举串、rawStatus 是原始值；模型只读
            // rawStatus 建枚举，两个都放进来才和线上一致。
            "status": "not_accepted",
            "rawStatus": "not_accepted",
            "class": "realAction",
            "classReason": "需要在 WorkBuddy 里真实操作",
            "locked": false,
        ]
        for (key, value) in overrides { base[key] = value }
        return base
    }

    private func scan(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var base: [String: Any] = [
            "accountId": "acc-1",
            "nickname": "工作号",
            "state": "ok",
            "tasks": [task()],
            "profile": [
                "level": 15,
                "completedTasks": 15,
                "totalTasks": 19,
                "energy": 8,
                "streakDays": 3,
            ],
            "counts": ["total": 1, "claimable": 0, "acceptable": 1],
            "authSource": "magpie",
            "pathPrefix": "/v2/activity/growth",
        ]
        for (key, value) in overrides { base[key] = value }
        return base
    }

    // MARK: - 任务

    func testParsesTaskAndKeepsClass() throws {
        let row = try XCTUnwrap(GrowthTaskRow(json: task()))
        XCTAssertEqual(row.code, "create_canvas")
        XCTAssertEqual(row.title, "创建第一个画布")
        XCTAssertEqual(row.taskClass, .realAction)
        XCTAssertEqual(row.status, .notAccepted)
        XCTAssertEqual(row.progressText, "0 / 1")
        XCTAssertEqual(row.rewardText, "+20 积分")
        XCTAssertFalse(row.isUnrecognised)
    }

    /// target 缺失时显示破折号，不是 "0 / 0" —— 后者会让人以为进度真的是零。
    func testMissingTargetShowsDashInsteadOfZero() throws {
        let row = try XCTUnwrap(GrowthTaskRow(json: task(["target": 0, "current": 0])))
        XCTAssertEqual(row.progressText, "—")
    }

    func testProgressNeverExceedsTarget() throws {
        let row = try XCTUnwrap(GrowthTaskRow(json: task(["current": 5, "target": 3])))
        XCTAssertEqual(row.progressText, "3 / 3", "服务端回超了也不能显示 5 / 3")
    }

    /// 未知分类必须显式暴露，界面据此标红并不自动执行。
    func testUnknownClassIsSurfaced() throws {
        let row = try XCTUnwrap(
            GrowthTaskRow(json: task(["class": "unknown", "status": "mystery", "rawStatus": "mystery"]))
        )
        XCTAssertEqual(row.taskClass, .unknown)
        XCTAssertTrue(row.isUnrecognised)
        XCTAssertEqual(row.status, .unknown, "没见过的状态落 unknown，不是 notAccepted")
    }

    func testMissingCodeDropsTheTask() {
        XCTAssertNil(GrowthTaskRow(json: task(["code": "  "])))
    }

    func testRewardTextOmitsZeroValues() throws {
        let row = try XCTUnwrap(GrowthTaskRow(json: task(["rewardCredit": 0, "rewardEnergy": 5])))
        XCTAssertEqual(row.rewardText, "+5 能量")
        let none = try XCTUnwrap(GrowthTaskRow(json: task(["rewardCredit": 0, "rewardEnergy": 0])))
        XCTAssertEqual(none.rewardText, "—")
    }

    // MARK: - 账号

    func testParsesAccountScanAndProfileSummary() throws {
        let account = try XCTUnwrap(GrowthAccountScan(json: scan()))
        XCTAssertEqual(account.accountId, "acc-1")
        XCTAssertEqual(account.state, "ok")
        XCTAssertEqual(account.stateText, "正常")
        XCTAssertEqual(account.authSourceText, "Magpie 授权")
        XCTAssertEqual(account.tasks.count, 1)
        XCTAssertEqual(account.profile.summaryText, "Lv.15 · 完成 15/19 · 8 能量 · 连签 3 天")
        XCTAssertEqual(account.pendingCount, 1)
        XCTAssertFalse(account.needsAuth)
    }

    /// 查不到的项不要出现在摘要里；一个都没有就是破折号。
    func testUnknownProfileFieldsDoNotBecomeZero() throws {
        let account = try XCTUnwrap(GrowthAccountScan(json: scan(["profile": [:]])))
        XCTAssertNil(account.profile.credit)
        XCTAssertNil(account.profile.level)
        XCTAssertEqual(account.profile.summaryText, "—")
    }

    /// 成长中心没有积分接口：摘要里不能出现"0 积分"这种会误导的措辞。
    func testSummaryNeverFabricatesCredit() throws {
        let account = try XCTUnwrap(GrowthAccountScan(json: scan([
            "profile": ["level": 3, "energy": 0],
        ])))
        XCTAssertNil(account.profile.credit)
        XCTAssertFalse(account.profile.summaryText.contains("积分"))
        XCTAssertTrue(account.profile.summaryText.contains("0 能量"), "能量确实是 0，要显示")
    }

    func testUnauthorizedAccountIsCalledOut() throws {
        let account = try XCTUnwrap(GrowthAccountScan(json: scan([
            "state": "unauthorized",
            "tasks": [],
            "errors": [["field": "tasks", "kind": "unauthorized", "message": "授权失效"]],
        ])))
        XCTAssertTrue(account.needsAuth)
        XCTAssertEqual(account.stateText, "授权失效")
        XCTAssertEqual(account.errors.first?.needsUserAction, true)
    }

    /// 缺授权和 429 都是错误，但只有前者需要人去处理。
    func testTransientErrorsDoNotAskForUserAction() {
        let kind = GrowthFieldError(json: ["field": "tasks", "kind": "rate_limited", "message": "限流"])
        XCTAssertFalse(kind.needsUserAction)
        let missing = GrowthFieldError(json: ["field": "tasks", "kind": "missing_credential", "message": "没有凭据"])
        XCTAssertTrue(missing.needsUserAction)
    }

    func testTravelStateText() throws {
        let account = try XCTUnwrap(GrowthAccountScan(json: scan([
            "travel": ["state": "arrived", "location": "咖啡馆"],
        ])))
        XCTAssertEqual(account.travel?.stateText, "已到达")
        XCTAssertEqual(account.travel?.location, "咖啡馆")

        let noTravel = try XCTUnwrap(GrowthAccountScan(json: scan(["travel": [:] as [String: Any]])))
        XCTAssertNil(noTravel.travel, "没有 state 就不该造一个旅行对象出来")
    }

    // MARK: - 总览

    func testOverviewHeadlineListsOnlyNonZero() {
        let overview = GrowthOverview(json: [
            "accounts": 4,
            "claimable": 2,
            "acceptable": 0,
            "manual": 1,
            "unknown": 0,
            "traveling": 1,
            "needsAuth": 0,
        ])
        XCTAssertEqual(overview.headline, "4 个账号 · 2 项待领奖 · 1 项需人工 · 1 个旅行中")
        XCTAssertFalse(overview.headline.contains("可接受"))
    }

    func testScanResultParsesScansAndOverview() {
        let result = GrowthScanResult(json: [
            "scans": [scan(), scan(["accountId": "acc-2", "nickname": "小号"])],
            "overview": ["accounts": 2, "claimable": 1],
        ])
        XCTAssertEqual(result.scans.count, 2)
        XCTAssertEqual(result.overview.accounts, 2)
        XCTAssertEqual(result.overview.claimable, 1)
    }

    func testEmptyResultIsReallyEmpty() {
        XCTAssertTrue(GrowthScanResult.empty.scans.isEmpty)
        XCTAssertEqual(GrowthScanResult.empty.overview.accounts, 0)
    }

    // MARK: - 自动管理（PR #68）

    func testAutoStatusDefaultsToOff() {
        let status = GrowthAutoStatus(json: nil)
        XCTAssertFalse(status.enabled, "默认必须是关的")
        XCTAssertFalse(status.continuesAfterQuit, "不许暗示 App 退出后还在跑")
        XCTAssertEqual(status.intervalText, "每 3 分钟")
        XCTAssertEqual(status.nextTickText, "未开启")
    }

    func testAutoStatusFormatsIntervals() {
        XCTAssertEqual(
            GrowthAutoStatus(json: ["enabled": true, "intervalSeconds": 180]).intervalText,
            "每 3 分钟"
        )
        XCTAssertEqual(
            GrowthAutoStatus(json: ["enabled": true, "intervalSeconds": 60]).intervalText,
            "每分钟"
        )
        XCTAssertEqual(
            GrowthAutoStatus(json: ["enabled": true, "intervalSeconds": 3600]).intervalText,
            "每 1 小时"
        )
    }

    func testAutoStatusReadsNextTick() {
        let future = ISO8601DateFormatter().string(from: Date().addingTimeInterval(120))
        let status = GrowthAutoStatus(json: ["enabled": true, "nextTickAt": future])
        XCTAssertTrue(status.nextTickText.hasPrefix("下次"), status.nextTickText)
    }

    func testTickResultExplainsWhyItSkipped() {
        XCTAssertEqual(
            GrowthTickResult(json: ["skipped": "disabled"]).skippedText,
            "自动管理未开启"
        )
        XCTAssertEqual(
            GrowthTickResult(json: ["skipped": "not_due"]).skippedText,
            "还没到检查时间"
        )
        XCTAssertNil(GrowthTickResult(json: ["summary": "领奖 1"]).skippedText)
    }

    func testRunRecordSummaryHidesZeroRewards() throws {
        let ok = try XCTUnwrap(GrowthRunRecord(json: [
            "id": 1,
            "accountId": "acc-1",
            "taskCode": "t1",
            "action": "claim",
            "outcome": "ok",
            "credit": 100,
            "energy": 5,
        ]))
        XCTAssertEqual(ok.summary, "上次 领奖 · 成功 +100 积分 +5 能量")
        XCTAssertFalse(ok.outcome.isProblem)
    }

    func testRunRecordShowsTheFailureReason() throws {
        let rejected = try XCTUnwrap(GrowthRunRecord(json: [
            "taskCode": "t1",
            "action": "accept",
            "outcome": "rejected",
            "errorDetail": "prerequisite not met: first_buddy",
        ]))
        XCTAssertTrue(rejected.outcome.isProblem)
        XCTAssertTrue(rejected.summary.contains("被拒绝"))
        XCTAssertTrue(rejected.summary.contains("first_buddy"))
    }

    /// `already` 是幂等回执，不能标红当错误。
    func testAlreadyClaimedIsNotAProblem() throws {
        let already = try XCTUnwrap(GrowthRunRecord(json: [
            "taskCode": "t1",
            "action": "claim",
            "outcome": "already",
            "credit": 0,
        ]))
        XCTAssertFalse(already.outcome.isProblem)
        XCTAssertFalse(already.summary.contains("+0"))
    }

    func testRunsIndexByTaskAndKeepTotals() {
        let runs = GrowthRuns(json: [
            "latestByTask": [
                ["id": 2, "taskCode": "a", "action": "claim", "outcome": "ok"],
                ["id": 3, "taskCode": "b", "action": "accept", "outcome": "ok"],
            ],
            "grantedCredit": 250,
            "grantedEnergy": 12,
        ])
        XCTAssertEqual(runs.latestByTask.count, 2)
        XCTAssertEqual(runs.latestByTask["a"]?.outcome, .ok)
        XCTAssertEqual(runs.grantedCredit, 250)
        XCTAssertEqual(runs.grantedEnergy, 12)
    }
}
