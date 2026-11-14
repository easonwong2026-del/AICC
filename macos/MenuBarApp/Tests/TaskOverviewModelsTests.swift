import XCTest
@testable import AICCCore

/// PR #76：顶部汇总的语义。
///
/// 这组用例守两件事：**未查询不是 0**、**失败与未完成分开**。
final class TaskOverviewModelsTests: XCTestCase {
    func testOverviewSumsRealCounts() throws {
        let overview = try XCTUnwrap(TasksOverview(json: [
            "today": "2026-11-14",
            "accounts": ["total": 4, "workbuddy": 3, "qoder": 1],
            "checkin": ["done": 2, "pending": 1, "failed": 1],
            "growth": ["runs": 5, "failed": 1],
            "travel": ["claimed": 1, "pending": 2],
            "qoderCredits": ["claimed": 1, "failed": 0, "autoEnabled": 1, "claimable": NSNull()],
            "needsReauth": 2,
            "autoEnabled": 3,
            "failureCount": 2,
            "recentFailures": [
                ["platform": "qoder_cn", "accountId": "qa_1", "task": "qoder_credits",
                 "status": "rate_limited", "detail": "被限流", "at": "2026-11-14T02:00:00Z"],
            ],
        ]))
        XCTAssertEqual(overview.accountTotal, 4)
        XCTAssertEqual(overview.doneToday, 4)      // 2 签到 + 1 旅行 + 1 Credits
        XCTAssertEqual(overview.pendingToday, 3)   // 1 签到 + 2 旅行
        XCTAssertEqual(overview.failedToday, 2)    // 1 签到 + 1 成长 + 0 Credits
        XCTAssertEqual(overview.needsReauth, 2)
        XCTAssertEqual(overview.failureCount, 2)
        // null 必须是 nil：界面要能区分"未查询"与"0 个可领"。
        XCTAssertNil(overview.qoderClaimable)
        XCTAssertEqual(overview.recentFailures.first?.taskLabel, "Qoder Credits")
    }

    func testFailureTaskLabelsAreHumanReadable() throws {
        let growth = try XCTUnwrap(TaskFailure(json: [
            "task": "growth:expert_5", "status": "failed", "accountId": "acc_1",
        ]))
        XCTAssertEqual(growth.taskLabel, "成长 expert_5")
        // 没有昵称时用脱敏 id，不显示空字符串。
        XCTAssertFalse(growth.displayName.isEmpty)
        XCTAssertNil(growth.detail)
    }

    func testMissingSectionsDoNotCrashOrInventNumbers() throws {
        let overview = try XCTUnwrap(TasksOverview(json: ["today": "2026-11-14"]))
        XCTAssertEqual(overview.doneToday, 0)
        XCTAssertNil(overview.qoderClaimable)
        XCTAssertTrue(overview.recentFailures.isEmpty)
    }
}
