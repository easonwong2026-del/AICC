import XCTest
@testable import AICCCore

/// `growth.activity.status` 的模型层（PR #103，2026-11-18 修订）。
///
/// 守四句话：
/// 1. 签到成功 ≠ 今日活跃；
/// 2. 「本地没记录」≠「没签到」（可能是服务端或别的客户端签的）；
/// 3. 没查到 ≠ 未活跃；
/// 4. 下一档天数用服务端的，不做减法。
final class ActivityModelsTests: XCTestCase {

    private func checkinJSON(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var json: [String: Any] = [
            "state": "confirmed",
            "source": "local",
            "status": "already",
            "checkedAt": "2026-11-18T01:03:01Z",
            "localDate": "2026-11-18",
            "serverLastDate": "2026-11-18",
        ]
        for (key, value) in overrides { json[key] = value }
        return json
    }

    private func account(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var json: [String: Any] = [
            "accountId": "acc-1",
            "nickname": "主账号",
            "state": "ok",
            "serverToday": "2026-11-18",
            "todayActive": true,
            "todayScore": 4,
            "todayNote": "今日已活跃，继续点亮成长热力墙",
            "streakDays": 18,
            "nextTier": "28d",
            "nextTierRemaining": 10,
            "monthActiveDays": 18,
            "tiers": [
                ["tier": "7d", "days": 7, "status": "claimed", "state": "claimed", "claimable": false],
                ["tier": "14d", "days": 14, "status": "claimed", "state": "claimed", "claimable": false],
                ["tier": "28d", "days": 28, "status": "locked", "state": "locked", "claimable": false],
            ],
            "lastActiveDate": "2026-11-18",
            "activeDays7": 7,
            "serverUpdatedAt": "2026-11-18T02:00:00+08:00",
            "checkin": checkinJSON(),
            "diagnosis": [
                "code": "active", "label": "今日已活跃", "action": "", "needsAction": false,
            ],
            "errors": [],
            "fetchedAt": "2026-11-18T02:43:00Z",
        ]
        for (key, value) in overrides { json[key] = value }
        return json
    }

    private func fleet(_ accounts: [[String: Any]], summary: [String: Any]) -> ActivityFleetState {
        ActivityFleetState(json: [
            "accounts": accounts,
            "summary": summary,
            "fetchedAt": "2026-11-18T02:43:00Z",
            "readOnly": true,
        ])
    }

    // MARK: - 解码

    func testDecodesAFullPayload() {
        let state = ActivityAccountState(json: account())
        XCTAssertEqual(state.accountId, "acc-1")
        XCTAssertEqual(state.serverToday, "2026-11-18")
        XCTAssertEqual(state.todayActive, true)
        XCTAssertEqual(state.todayScore, 4)
        XCTAssertEqual(state.streakDays, 18)
        XCTAssertEqual(state.nextTierRemaining, 10)
        XCTAssertEqual(state.tiers.count, 3)
        XCTAssertEqual(state.checkin.state, "confirmed")
        XCTAssertEqual(state.checkin.serverLastDate, "2026-11-18")
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
        let state = ActivityAccountState(json: account(["todayActive": false, "todayScore": 0]))
        XCTAssertEqual(state.activeText, "未活跃", "0 分且未活跃时不能拼成已活跃 · 0 分")
    }

    func testActiveWithoutScoreStillSaysActive() {
        var json = account()
        json.removeValue(forKey: "todayScore")
        XCTAssertEqual(ActivityAccountState(json: json).activeText, "已活跃")
    }

    // MARK: - 签到：五种来源各说各话

    func testCheckinTextSeparatesLocalFromServer() {
        let local = ActivityAccountState(json: account(["checkin": checkinJSON()]))
        XCTAssertEqual(local.checkinText, "已签（本地）")

        let server = ActivityAccountState(json: account([
            "checkin": checkinJSON(["source": "server", "status": NSNull(), "localDate": NSNull()]),
        ]))
        XCTAssertEqual(server.checkinText, "已签（服务端）", "服务端确认但 AICC 没记录，必须说清来源")
        XCTAssertTrue(server.checkin.detail.contains("服务端最近签到"))
    }

    func testCheckinStatesHaveDistinctWording() {
        func text(_ state: String, source: String? = "server") -> String {
            let json = account(["checkin": checkinJSON(["state": state, "source": source ?? NSNull()])])
            return ActivityAccountState(json: json).checkinText
        }
        XCTAssertEqual(text("absent"), "未签到")
        XCTAssertEqual(text("failed"), "签到失败")
        XCTAssertEqual(text("unconfirmed"), "未确认")
        XCTAssertEqual(text("auth_unavailable", source: nil), "—")
    }

    func testAMissingCheckinObjectIsUnconfirmedNotMissing() {
        var json = account()
        json.removeValue(forKey: "checkin")
        let state = ActivityAccountState(json: json)
        XCTAssertEqual(state.checkin.state, "unconfirmed")
        XCTAssertEqual(state.checkinText, "未确认")
        XCTAssertFalse(state.checkin.isConfirmed)
    }

    /// 本地失败过但服务端说签了：界面上必须是"已签（服务端）"，不是"签到失败"。
    func testTheServerVerdictWinsOverALocalFailure() {
        let state = ActivityAccountState(json: account([
            "checkin": checkinJSON(["source": "server", "status": "failed", "localDate": NSNull()]),
        ]))
        XCTAssertTrue(state.checkin.isConfirmed)
        XCTAssertEqual(state.checkinText, "已签（服务端）")
    }

    // MARK: - 下一档：服务端数字优先

    func testNextTierUsesTheServerRemainingNotStreakArithmetic() {
        let state = ActivityAccountState(json: account([
            "streakDays": 0, "nextTier": "7d", "nextTierRemaining": 6,
            "todayActive": false, "todayScore": 0,
        ]))
        XCTAssertEqual(state.nextTierText, "7 天档 · 还差 6 天")
        XCTAssertNotEqual(state.nextTierText, "7 天档 · 还差 7 天")
    }

    func testNextTierWithoutRemainingDegradesCleanly() {
        var json = account()
        json.removeValue(forKey: "nextTierRemaining")
        XCTAssertEqual(ActivityAccountState(json: json).nextTierText, "28 天档")
    }

    func testMissingNextTierIsADash() {
        var json = account()
        json.removeValue(forKey: "nextTier")
        json.removeValue(forKey: "nextTierRemaining")
        XCTAssertEqual(ActivityAccountState(json: json).nextTierText, "—")
    }

    // MARK: - 档位与最近活跃

    func testTierLineListsEveryTierInAscendingOrder() {
        XCTAssertEqual(ActivityAccountState(json: account()).tierLine, "7 天 已领取 · 14 天 已领取 · 28 天 未达成")
    }

    func testTierLineIsADashWhenNothingWasRead() {
        XCTAssertEqual(ActivityAccountState(json: account(["tiers": []])).tierLine, "—")
    }

    func testRecentTextMarksToday() {
        XCTAssertEqual(ActivityAccountState(json: account()).recentText, "近 7 天活跃 7 天 · 上次 今天")
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

    // MARK: - 诊断文案

    func testOnlyTheStaleDiagnosisExplainsDelay() {
        let stale = ActivityAccountState(json: account([
            "todayActive": false,
            "todayScore": 30,
            "diagnosis": [
                "code": "stale_stats", "label": "服务端有分数但未标记活跃（可能延迟）",
                "action": "稍后点刷新回查，不用重复做事", "needsAction": false,
            ],
        ]))
        XCTAssertNotNil(stale.staleNote)
        XCTAssertFalse(stale.diagnosis.needsAction, "延迟不该催用户马上做事")

        let inactive = ActivityAccountState(json: account([
            "todayActive": false,
            "diagnosis": [
                "code": "checkin_only", "label": "签到已完成，今日未活跃",
                "action": "去客户端做一次真实任务", "needsAction": true,
            ],
        ]))
        XCTAssertNil(inactive.staleNote)
    }

    func testShortLabelsAreShortAndStable() {
        let cases = [
            ("active", "已活跃"), ("checkin_only", "未活跃"), ("checkin_unverified", "签到待查"),
            ("inactive", "今日未用"), ("stale_stats", "待落库"), ("needs_auth", "需授权"),
            ("unsupported", "不支持"), ("whatever", "未知"),
        ]
        for (code, expected) in cases {
            let diagnosis = ActivityDiagnosis(json: ["code": code, "label": "很长的一句话，不该进表格"])
            XCTAssertEqual(diagnosis.shortLabel, expected)
            XCTAssertLessThanOrEqual(diagnosis.shortLabel.count, 4)
            XCTAssertEqual(diagnosis.label, "很长的一句话，不该进表格", "完整句子必须保留给 tooltip")
        }
    }

    func testOnlyCheckinOnlyAndInactiveOfferTheClientButton() {
        for code in ["active", "checkin_unverified", "stale_stats", "unknown", "needs_auth"] {
            let diagnosis = ActivityDiagnosis(json: ["code": code, "action": "做什么"])
            XCTAssertFalse(diagnosis.needsClient, "\(code) 不该出现「打开客户端」")
        }
        for code in ["checkin_only", "inactive"] {
            XCTAssertTrue(ActivityDiagnosis(json: ["code": code]).needsClient)
        }
        // 右列只放短词，完整说法留在 action / tooltip。
        XCTAssertEqual(ActivityDiagnosis(json: ["code": "checkin_unverified"]).shortAction, "看签到")
        XCTAssertEqual(ActivityDiagnosis(json: ["code": "needs_auth"]).shortAction, "去授权")
        XCTAssertEqual(ActivityDiagnosis(json: ["code": "active"]).shortAction, "")
    }

    func testTooltipCombinesLabelAndAction() {
        let withAction = ActivityDiagnosis(json: [
            "code": "checkin_only", "label": "签到已完成，今日未活跃", "action": "去做任务",
        ])
        XCTAssertEqual(withAction.tooltip, "签到已完成，今日未活跃 · 去做任务")
        let withoutAction = ActivityDiagnosis(json: ["code": "active", "label": "今日已活跃", "action": ""])
        XCTAssertEqual(withoutAction.tooltip, "今日已活跃")
    }

    func testUnknownDiagnosisNeverIsActive() {
        XCTAssertFalse(ActivityDiagnosis(json: ["code": "unknown"]).isActive)
        XCTAssertTrue(ActivityDiagnosis(json: ["code": "unknown"]).isUnknown)
    }

    func testAMissingDiagnosisObjectFallsBackToUnknown() {
        var json = account()
        json.removeValue(forKey: "diagnosis")
        let state = ActivityAccountState(json: json)
        XCTAssertEqual(state.diagnosis.code, "unknown")
        XCTAssertFalse(state.diagnosis.needsAction)
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

    func testNeedsActionAndDelayedOnlyPickFlaggedAccounts() {
        let active = account(["accountId": "a"])
        let pending = account([
            "accountId": "b",
            "todayActive": false,
            "diagnosis": ["code": "checkin_only", "label": "…", "action": "…", "needsAction": true],
        ])
        let delayed = account([
            "accountId": "c",
            "todayActive": false,
            "todayScore": 20,
            "diagnosis": ["code": "stale_stats", "label": "…", "action": "…", "needsAction": false],
        ])
        let unknown = account([
            "accountId": "d",
            "diagnosis": ["code": "unknown", "label": "…", "action": "…", "needsAction": false],
        ])
        let state = fleet([active, pending, delayed, unknown], summary: [
            "total": 4, "active": 1, "pending": 2, "unknown": 1, "needsAuth": 0,
        ])
        XCTAssertEqual(state.needsAction.map(\.accountId), ["b"])
        XCTAssertEqual(state.delayed.map(\.accountId), ["c"])
        XCTAssertEqual(state.account("d")?.diagnosis.isUnknown, true)
        XCTAssertNil(state.account("zzz"))
    }

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
        XCTAssertEqual(mock.needsAction.count, 2, "checkin_only + checkin_unverified")
        XCTAssertEqual(mock.delayed.count, 1, "只有那条有分数没标记活跃的是延迟")
        XCTAssertEqual(mock.accounts.first?.activeText, "已活跃 · 4 分")
    }
}
