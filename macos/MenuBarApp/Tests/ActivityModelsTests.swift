import XCTest
@testable import AICCCore

/// `growth.activity.status` 的模型层（PR #103）。
///
/// 这组测试守的是三句界面不能说错的话：
/// 1. 签到成功 ≠ 今日活跃；
/// 2. 没查到 ≠ 未活跃；
/// 3. 下一档天数用服务端的，不做减法。
final class ActivityModelsTests: XCTestCase {

    private func account(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var json: [String: Any] = [
            "accountId": "acc-1",
            "nickname": "主账号",
            "state": "ok",
            "serverToday": "2026-11-17",
            "todayActive": true,
            "todayScore": 105,
            "todayNote": "今日已活跃，继续点亮成长热力墙",
            "streakDays": 17,
            "nextTier": "28d",
            "nextTierRemaining": 11,
            "monthActiveDays": 17,
            "tiers": [
                ["tier": "7d", "days": 7, "status": "claimed", "state": "claimed", "claimable": false],
                ["tier": "14d", "days": 14, "status": "claimed", "state": "claimed", "claimable": false],
                ["tier": "28d", "days": 28, "status": "locked", "state": "locked", "claimable": false],
            ],
            "lastActiveDate": "2026-11-17",
            "activeDays7": 7,
            "updatedAt": "2026-11-17T22:38:04+08:00",
            "checkin": [
                "status": "already",
                "checkedAt": "2026-11-17T01:03:01Z",
                "localDate": "2026-11-17",
                "source": "auto",
            ],
            "diagnosis": ["code": "active", "label": "今日已活跃", "needsAction": false],
            "errors": [],
            "fetchedAt": "2026-11-17T14:40:00Z",
        ]
        for (key, value) in overrides { json[key] = value }
        return json
    }

    private func fleet(_ accounts: [[String: Any]], summary: [String: Any]) -> ActivityFleetState {
        ActivityFleetState(json: [
            "accounts": accounts,
            "summary": summary,
            "fetchedAt": "2026-11-17T14:40:00Z",
            "readOnly": true,
        ])
    }

    // MARK: - 解码

    func testDecodesAFullPayload() {
        let state = ActivityAccountState(json: account())
        XCTAssertEqual(state.accountId, "acc-1")
        XCTAssertEqual(state.serverToday, "2026-11-17")
        XCTAssertEqual(state.todayActive, true)
        XCTAssertEqual(state.todayScore, 105)
        XCTAssertEqual(state.streakDays, 17)
        XCTAssertEqual(state.nextTierRemaining, 11)
        XCTAssertEqual(state.tiers.count, 3)
        XCTAssertEqual(state.checkin?.status, "already")
        XCTAssertEqual(state.diagnosis.code, "active")
        XCTAssertEqual(state.state, "ok")
    }

    func testAMissingActiveFlagStaysUnknown() {
        var json = account()
        json.removeValue(forKey: "todayActive")
        let state = ActivityAccountState(json: json)
        XCTAssertNil(state.todayActive)
        XCTAssertEqual(state.activeText, "—", "没查到必须显示破折号，不能显示未活跃")
    }

    func testFalseActiveReadsAsInactiveWithZeroScore() {
        let state = ActivityAccountState(json: account([
            "todayActive": false,
            "todayScore": 0,
            "diagnosis": ["code": "checkin_only", "label": "签到已完成，今日未活跃", "needsAction": true],
        ]))
        XCTAssertEqual(state.activeText, "未活跃", "0 分且未活跃时不能拼成已活跃 · 0 分")
        XCTAssertTrue(state.diagnosis.needsAction)
    }

    func testActiveWithoutScoreStillSaysActive() {
        var json = account()
        json.removeValue(forKey: "todayScore")
        let state = ActivityAccountState(json: json)
        XCTAssertEqual(state.activeText, "已活跃")
    }

    // MARK: - 签到与活跃分开

    func testCheckinTextNeverClaimsActivity() {
        let state = ActivityAccountState(json: account())
        XCTAssertEqual(state.checkinText, "今日已签（自动）")
        XCTAssertEqual(state.activeText, "已活跃 · 105 分")
    }

    func testNoCheckinRecordIsStatedAsSuch() {
        var json = account()
        json.removeValue(forKey: "checkin")
        let state = ActivityAccountState(json: json)
        XCTAssertNil(state.checkin)
        XCTAssertEqual(state.checkinText, "今天没有签到记录")
    }

    func testFailedCheckinKeepsItsOwnWording() {
        let state = ActivityAccountState(json: account([
            "checkin": ["status": "failed", "checkedAt": "2026-11-17T01:00:00Z",
                        "localDate": "2026-11-17", "source": "manual"],
        ]))
        XCTAssertEqual(state.checkinText, "签到失败（手动）")
    }

    // MARK: - 下一档：服务端数字优先

    func testNextTierUsesTheServerRemainingNotStreakArithmetic() {
        // 实测样本：连登 0 天，但 7 天档还差 6 天。本地减法会算出 7。
        let state = ActivityAccountState(json: account([
            "streakDays": 0,
            "nextTier": "7d",
            "nextTierRemaining": 6,
            "todayActive": false,
            "todayScore": 0,
        ]))
        XCTAssertEqual(state.nextTierText, "7 天档 · 还差 6 天")
        XCTAssertNotEqual(state.nextTierText, "7 天档 · 还差 7 天")
    }

    func testNextTierWithoutRemainingDegradesCleanly() {
        var json = account()
        json.removeValue(forKey: "nextTierRemaining")
        let state = ActivityAccountState(json: json)
        XCTAssertEqual(state.nextTierText, "28 天档")
    }

    func testMissingNextTierIsADash() {
        var json = account()
        json.removeValue(forKey: "nextTier")
        json.removeValue(forKey: "nextTierRemaining")
        XCTAssertEqual(ActivityAccountState(json: json).nextTierText, "—")
    }

    // MARK: - 档位行

    func testTierLineListsEveryTierInAscendingOrder() {
        let state = ActivityAccountState(json: account())
        XCTAssertEqual(state.tierLine, "7 天 已领取 · 14 天 已领取 · 28 天 未达成")
    }

    func testTierLineIsADashWhenNothingWasRead() {
        let state = ActivityAccountState(json: account(["tiers": []]))
        XCTAssertEqual(state.tierLine, "—")
    }

    // MARK: - 最近活跃

    func testRecentTextMarksToday() {
        let state = ActivityAccountState(json: account())
        XCTAssertEqual(state.recentText, "近 7 天活跃 7 天 · 上次 今天")
    }

    func testRecentTextOmittsTheTodaySuffixOnOlderDates() {
        let state = ActivityAccountState(json: account(["lastActiveDate": "2026-11-13", "activeDays7": 1]))
        XCTAssertEqual(state.recentText, "近 7 天活跃 1 天 · 上次 11-13")
    }

    func testRecentTextIsNilWithoutEvidence() {
        var json = account()
        json.removeValue(forKey: "lastActiveDate")
        json.removeValue(forKey: "activeDays7")
        XCTAssertNil(ActivityAccountState(json: json).recentText)
    }

    // MARK: - 延迟解释

    func testOnlyTheStaleDiagnosisExplainsDelay() {
        let stale = ActivityAccountState(json: account([
            "todayActive": false,
            "diagnosis": ["code": "stale_stats", "label": "今天有行为，活跃尚未落库（可能延迟）", "needsAction": false],
        ]))
        XCTAssertNotNil(stale.staleNote)
        XCTAssertFalse(stale.diagnosis.needsAction, "延迟不该催用户马上做事")

        let inactive = ActivityAccountState(json: account([
            "todayActive": false,
            "diagnosis": ["code": "checkin_only", "label": "签到已完成，今日未活跃", "needsAction": true],
        ]))
        XCTAssertNil(inactive.staleNote)
    }

    // MARK: - 顶栏汇总

    func testSummaryTextOnlyListsNonZeroBuckets() {
        let state = fleet([account()], summary: [
            "total": 4, "active": 1, "pending": 2, "unknown": 0, "needsAuth": 0,
        ])
        XCTAssertEqual(state.summaryText, "4 个账号 · 已活跃 1 · 待处理 2")
        XCTAssertFalse(state.summaryText.contains("未知 0"))
    }

    func testSummaryTextDoesNotInventAccounts() {
        XCTAssertEqual(ActivityFleetState.empty.summaryText, "还没有可检查的国内版账号")
    }

    func testNeedsActionOnlyPicksFlaggedAccounts() {
        let active = account(["accountId": "a"])
        let pending = account([
            "accountId": "b",
            "todayActive": false,
            "diagnosis": ["code": "checkin_only", "label": "签到已完成，今日未活跃", "needsAction": true],
        ])
        let unknown = account([
            "accountId": "c",
            "diagnosis": ["code": "unknown", "label": "活跃状态未知", "needsAction": false],
        ])
        let state = fleet([active, pending, unknown], summary: [
            "total": 3, "active": 1, "pending": 1, "unknown": 1, "needsAuth": 0,
        ])
        XCTAssertEqual(state.needsAction.map(\.accountId), ["b"])
        XCTAssertEqual(state.account("c")?.diagnosis.isUnknown, true)
        XCTAssertNil(state.account("zzz"))
    }

    func testUnknownDiagnosisNeverIsActive() {
        XCTAssertFalse(ActivityDiagnosis(json: ["code": "unknown"]).isActive)
        XCTAssertTrue(ActivityDiagnosis(json: ["code": "unknown"]).isUnknown)
    }

    /// 表格列宽只有 84pt，长句子必须压成短标签；完整说法留在 tooltip（`label`）。
    func testShortLabelsAreShortAndStable() {
        let cases = [
            ("active", "已活跃"), ("checkin_only", "未活跃"), ("checkin_missing", "签到未跑"),
            ("stale_stats", "待落库"), ("needs_auth", "需授权"), ("unsupported", "不支持"),
            ("whatever", "未知"),
        ]
        for (code, expected) in cases {
            let diagnosis = ActivityDiagnosis(json: ["code": code, "label": "很长的一句话，不该进表格"])
            XCTAssertEqual(diagnosis.shortLabel, expected)
            XCTAssertLessThanOrEqual(diagnosis.shortLabel.count, 4)
            XCTAssertEqual(diagnosis.label, "很长的一句话，不该进表格", "完整句子必须保留给 tooltip")
        }
    }

    // MARK: - 状态文案

    func testStateLabelsCoverEveryBackendValue() {
        func label(_ state: String) -> String {
            ActivityAccountState(json: account(["state": state])).statusLabel
        }
        XCTAssertEqual(label("ok"), "已读取")
        XCTAssertEqual(label("partial"), "部分字段缺失")
        XCTAssertEqual(label("unauthorized"), "需要重新授权")
        XCTAssertEqual(label("missing_credential"), "没有可用凭据")
        XCTAssertEqual(label("unsupported"), "国际版不支持")
        XCTAssertEqual(label("weird"), "读取失败")
    }

    func testReadOnlyDefaultsToTrue() {
        let state = ActivityFleetState(json: ["accounts": [], "summary": [String: Any]()])
        XCTAssertTrue(state.readOnly)
        XCTAssertEqual(state.summary.total, 0)
    }

    func testMockIsInternallyConsistent() {
        let mock = ActivityFleetState.mock()
        XCTAssertEqual(mock.accounts.count, 4)
        XCTAssertEqual(mock.summary.total, 4)
        XCTAssertEqual(mock.needsAction.count, 1, "只有 checkin_only 那条需要用户去客户端做事")
        XCTAssertEqual(mock.accounts.first?.activeText, "已活跃 · 105 分")
    }
}
