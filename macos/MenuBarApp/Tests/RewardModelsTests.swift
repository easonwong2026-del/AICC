import XCTest
@testable import AICCCore

/// 奖励状态视图模型的解析与派生（PR #98）。
///
/// 这些断言盯的都是"会让界面说谎"的地方：
/// - 账本读不到（`awaitingVerification = null`）被画成 0；
/// - 补签卡余额没查到被说成"没有卡"；
/// - 服务端返回没见过的档位状态被当成"可领"。
final class RewardModelsTests: XCTestCase {

    private func account(overrides: [String: Any] = [:]) -> [String: Any] {
        var base: [String: Any] = [
            "accountId": "acc-1",
            "nickname": "示例",
            "state": "ok",
            "streakDays": 16,
            "lotteryChances": 1,
            "tiers": [
                ["tier": "7d", "days": 7, "status": "claimed", "state": "claimed", "claimable": false],
                ["tier": "14d", "days": 14, "status": "available", "state": "claimable", "claimable": true],
                ["tier": "28d", "days": 28, "status": "locked", "state": "locked", "claimable": false],
            ],
            "makeup": [
                "balance": 2,
                "max": 4,
                "today": "2026-11-17",
                "candidates": ["2026-11-16"],
            ],
            "awaitingVerification": 1,
            "recentOperations": [],
            "errors": [],
            "scannedAt": "2026-11-17T01:00:00Z",
            "readOnly": true,
        ]
        for (key, value) in overrides {
            base[key] = value
        }
        return base
    }

    // MARK: - 档位

    func testTiersMapToReadableStates() {
        let state = RewardAccountState(json: account())
        XCTAssertEqual(state.tiers.map(\.tier), ["7d", "14d", "28d"])
        XCTAssertEqual(state.tiers[0].stateLabel, "已领取")
        XCTAssertEqual(state.tiers[1].stateLabel, "可领取")
        XCTAssertEqual(state.tiers[2].stateLabel, "未达成")
        XCTAssertEqual(state.claimableTiers.map(\.tier), ["14d"])
    }

    func testAnUnrecognisedTierStateIsUnknownAndNotClaimable() {
        // 服务端哪天加一个新状态，界面必须说"状态未知"，绝不能显示可领。
        let state = RewardAccountState(json: account(overrides: [
            "tiers": [
                ["tier": "7d", "days": 7, "state": "something_new", "claimable": false],
            ],
        ]))
        XCTAssertEqual(state.tiers[0].stateLabel, "状态未知")
        XCTAssertFalse(state.tiers[0].claimable)
        XCTAssertTrue(state.claimableTiers.isEmpty)
    }

    func testAMissingTierStatusFallsBackToUnknownWithoutCrashing() {
        let state = RewardAccountState(json: account(overrides: ["tiers": [["days": 7]]]))
        XCTAssertEqual(state.tiers.count, 1)
        XCTAssertEqual(state.tiers[0].state, "unknown")
        XCTAssertNil(state.tiers[0].status)
    }

    // MARK: - 账本（P1 回归）

    func testAnUnreadableLedgerIsNilNotZero() {
        let state = RewardAccountState(json: account(overrides: ["awaitingVerification": NSNull()]))
        XCTAssertNil(state.awaitingVerification)
        XCTAssertNil(state.awaitingText, "读不到时不能显示成'没有'")
        XCTAssertNotNil(state.ledgerUnreadableText, "必须明确提示账本读取失败")
    }

    func testZeroAwaitingIsQuiet() {
        let state = RewardAccountState(json: account(overrides: ["awaitingVerification": 0]))
        XCTAssertEqual(state.awaitingVerification, 0)
        XCTAssertNil(state.awaitingText)
        XCTAssertNil(state.ledgerUnreadableText)
    }

    func testAwaitingCountsAreSurfacedAsOneSentence() {
        let state = RewardAccountState(json: account(overrides: ["awaitingVerification": 2]))
        XCTAssertEqual(state.awaitingText, "有 2 条操作还没核验完，AICC 会先回查再动")
    }

    // MARK: - 补签（P1 回归）

    func testBalanceUnknownIsNotReportedAsNoCards() {
        let unknown = RewardAccountState(json: account(overrides: [
            "makeup": ["candidates": ["2026-11-16"], "note": "balance_unknown"],
        ]))
        XCTAssertNil(unknown.makeup.balance)
        XCTAssertEqual(unknown.makeup.balanceText, "—")
        XCTAssertEqual(unknown.makeup.noteText, "补签卡余额没查到，稍后重试")

        let none = RewardAccountState(json: account(overrides: [
            "makeup": ["balance": 0, "candidates": ["2026-11-16"], "note": "no_cards"],
        ]))
        XCTAssertEqual(none.makeup.balanceText, "0 张")
        XCTAssertEqual(none.makeup.noteText, "补签卡已用完")
    }

    func testCalendarProblemsAreTranslatedNotEchoedAsCodes() {
        let state = RewardAccountState(json: account(overrides: [
            "makeup": ["balance": 1, "note": "missing_today", "candidates": []],
        ]))
        XCTAssertEqual(state.makeup.noteText, "服务端日期不可用，暂时无法判断断签")
        XCTAssertFalse(state.makeup.hasWorkToDo)
    }

    func testCandidatesSurviveAndKeepTheirOrder() {
        let state = RewardAccountState(json: account(overrides: [
            "makeup": ["balance": 2, "candidates": ["2026-11-16", "2026-11-14", "2026-11-13"]],
        ]))
        XCTAssertEqual(state.makeup.candidates.first, "2026-11-16")
        XCTAssertEqual(state.makeup.candidates.count, 3)
        XCTAssertTrue(state.makeup.hasWorkToDo)
    }

    // MARK: - 操作历史

    func testOperationPhasesReadAsActionsNotFailures() {
        let rows: [[String: Any]] = [
            ["id": "1", "action": "lottery_draw", "phase": "unknown", "attempt": 1, "open": true],
            ["id": "2", "action": "redeem", "target": "7d", "phase": "confirmed", "open": false],
            ["id": "3", "action": "makeup_card", "target": "2026-11-10", "phase": "frozen", "open": true],
            ["id": "4", "action": "redeem", "target": "14d", "phase": "failed", "open": false],
        ]
        let state = RewardAccountState(json: account(overrides: ["recentOperations": rows]))
        XCTAssertEqual(state.recentOperations.map(\.actionLabel), ["抽奖", "连登兑换", "补签", "连登兑换"])
        XCTAssertEqual(state.recentOperations.map(\.phaseLabel), ["待核验", "已到账", "已冻结", "确认未执行"])
        // 只有 unknown 需要回查；frozen 等人工，不该出现回查按钮。
        XCTAssertEqual(state.recentOperations.filter(\.needsAttention).count, 1)
    }

    // MARK: - 汇总

    func testSummaryOmitsUnknownNumbersInsteadOfShowingZero() {
        let state = RewardAccountState(json: account(overrides: [
            "streakDays": NSNull(),
            "lotteryChances": 0,
            "makeup": ["balance": NSNull(), "candidates": []],
            // 三档也必须是"没有可领的"，否则摘要里会出现 "1 档可领"，
            // 这条测试要验的是"没有数值可写时只剩 —"。
            "tiers": [["tier": "7d", "days": 7, "state": "unknown", "claimable": false]],
        ]))
        XCTAssertEqual(state.summaryText, "—", "查不到的项不出现，也不写 0")
    }

    func testSummaryListsTheThingsWorthSeeing() {
        let state = RewardAccountState(json: account())
        XCTAssertEqual(state.summaryText, "连登 16 天 · 抽奖 1 次 · 补签卡 2 / 4 张 · 1 档可领")
    }

    func testStateLabelsAreHuman() {
        for (raw, label) in [
            ("ok", "已读取"),
            ("partial", "部分字段缺失"),
            ("unauthorized", "需要重新授权"),
            ("missing_credential", "没有可用凭据"),
            ("unsupported", "国际版不支持"),
            ("failed", "读取失败"),
        ] {
            XCTAssertEqual(RewardAccountState(json: account(overrides: ["state": raw])).stateLabel, label)
        }
    }

    // MARK: - 刷新失败（保留旧数据）

    func testAFailedRefreshOverCachedDataProducesANotice() {
        // 复审 P1 回归：Store 保留旧数据 + 记 failure，界面必须说清楚"这是旧数据"。
        let state = RewardAccountState(json: account())
        let notice = RewardRefreshNotice.make(state: state, failure: "请求超时（60s）")
        XCTAssertEqual(notice?.reason, "请求超时（60s）")
        XCTAssertEqual(notice?.readAt, "2026-11-17T01:00:00Z")
        XCTAssertTrue(notice?.hasReadAt == true)
        XCTAssertEqual(RewardRefreshNotice.title, "刷新失败，以下为上次成功读取的数据")
    }

    func testASuccessfulRefreshHasNoNotice() {
        let state = RewardAccountState(json: account())
        XCTAssertNil(RewardRefreshNotice.make(state: state, failure: nil))
        XCTAssertNil(RewardRefreshNotice.make(state: state, failure: ""))
    }

    func testAFailedRefreshWithoutCacheHasNoStaleNotice() {
        // 没有旧数据时不该说"以下是上次的数据"—— 因为压根没有。
        XCTAssertNil(RewardRefreshNotice.make(state: nil, failure: "connection refused"))
    }

    func testAnUnknownReadTimeIsReportedAsUnknown() {
        let state = RewardAccountState(json: account(overrides: ["scannedAt": ""]))
        let notice = RewardRefreshNotice.make(state: state, failure: "timeout")
        XCTAssertEqual(notice?.hasReadAt, false)
    }

    // MARK: - 写操作结果

    func testTheFourWriteStatesAreKeptApart() {
        XCTAssertEqual(RewardActionResult(json: ["sent": true, "state": "succeeded"]).state, "succeeded")
        XCTAssertEqual(RewardActionResult(json: ["sent": true, "state": "rejected"]).state, "rejected")
        XCTAssertEqual(RewardActionResult(json: ["sent": true, "state": "pending"]).state, "pending")
        XCTAssertEqual(RewardActionResult(json: ["sent": true, "state": "frozen"]).state, "frozen")
    }

    func testUnknownAndFrozenAreFlaggedForAttentionNotAsFailure() {
        XCTAssertTrue(RewardActionResult(json: ["state": "pending"]).needsAttention)
        XCTAssertTrue(RewardActionResult(json: ["state": "frozen"]).needsAttention)
        XCTAssertFalse(RewardActionResult(json: ["state": "rejected"]).needsAttention)
        XCTAssertFalse(RewardActionResult(json: ["state": "succeeded"]).needsAttention)
    }

    func testAPreviewIsNotReportedAsSent() {
        let preview = RewardActionResult(json: ["decision": "preview", "note": "这只是预览"])
        XCTAssertFalse(preview.sent)
        XCTAssertEqual(preview.state, "preview")
    }

    func testActionResultFallsBackToReasonThenNote() {
        XCTAssertEqual(RewardActionResult(json: ["decision": "not_ready", "reason": "抽奖次数为 0"]).message, "抽奖次数为 0")
        XCTAssertEqual(RewardActionResult(json: ["decision": "preview", "note": "预览"]).message, "预览")
    }

    // MARK: - 连登为 0 的解释

    func testASeroStreakExplainsThatCheckinIsADifferentSignal() {
        let state = RewardAccountState(json: account(overrides: ["streakDays": 0]))
        let note = state.streakNote
        XCTAssertNotNil(note)
        // 只陈述服务端事实 + 未确证，不再断言"签到不会增加连登"。
        XCTAssertTrue(note?.contains("连登 0 天") == true)
        XCTAssertTrue(note?.contains("尚无确证") == true)
        XCTAssertFalse(note?.contains("不会让它") == true, "不许断言因果")
    }

    func testANonZeroStreakHasNoExplanation() {
        let state = RewardAccountState(json: account())  // streakDays: 16
        XCTAssertNil(state.streakNote)
    }

    // MARK: - 按钮门禁（回归：曾出现"说改了但提交里没有"）

    func testMakeupNeedsBothABrokenDayAndACard() {
        func state(balance: Any?, candidates: [String]) -> RewardAccountState {
            RewardAccountState(json: account(overrides: [
                "makeup": ["balance": balance ?? NSNull(), "candidates": candidates],
            ]))
        }
        let day = ["2026-11-16"]
        XCTAssertTrue(RewardAction.makeup.isEnabled(for: state(balance: 2, candidates: day)))
        XCTAssertFalse(RewardAction.makeup.isEnabled(for: state(balance: 0, candidates: day)))
        XCTAssertFalse(RewardAction.makeup.isEnabled(for: state(balance: -1, candidates: day)))
        XCTAssertFalse(RewardAction.makeup.isEnabled(for: state(balance: nil, candidates: day)), "余额未知不许点")
        XCTAssertFalse(RewardAction.makeup.isEnabled(for: state(balance: 2, candidates: [])), "没断签日不许点")
    }

    func testDrawNeedsAKnownPositiveChanceCount() {
        func state(chances: Any?) -> RewardAccountState {
            RewardAccountState(json: account(overrides: ["lotteryChances": chances ?? NSNull()]))
        }
        XCTAssertTrue(RewardAction.draw.isEnabled(for: state(chances: 1)))
        XCTAssertFalse(RewardAction.draw.isEnabled(for: state(chances: 0)))
        XCTAssertFalse(RewardAction.draw.isEnabled(for: state(chances: -2)))
        XCTAssertFalse(RewardAction.draw.isEnabled(for: state(chances: nil)))
    }

    func testRedeemNeedsAClaimableTier() {
        XCTAssertTrue(RewardAction.redeem.isEnabled(for: RewardAccountState(json: account())))
        let none = RewardAccountState(json: account(overrides: [
            "tiers": [["tier": "7d", "days": 7, "state": "claimed", "claimable": false]],
        ]))
        XCTAssertFalse(RewardAction.redeem.isEnabled(for: none))
    }

    // MARK: - 动作映射（P0-A 回归）

    func testActionRawValuesAreTheProtocolStrings() {
        XCTAssertEqual(RewardAction.draw.rawValue, "lottery_draw")
        XCTAssertEqual(RewardAction.redeem.rawValue, "redeem")
        XCTAssertEqual(RewardAction.makeup.rawValue, "makeup_card")
        // 反向也必须解得回来，否则确认按钮点了没反应。
        for action in RewardAction.allCases {
            XCTAssertEqual(RewardAction(rawValue: action.rawValue), action)
        }
        XCTAssertNil(RewardAction(rawValue: "lottery"), "未知动作不许 fallback")
    }

    func testPreviewsDecodeToTheMatchingAction() {
        for (raw, expected) in [
            ("lottery_draw", RewardAction.draw),
            ("redeem", RewardAction.redeem),
            ("makeup_card", RewardAction.makeup),
        ] {
            let preview = RewardPreview(json: [
                "preview": ["accountId": "a", "action": raw, "balance": 1, "balancedReady": true, "cost": "消耗 1 次"],
            ])
            XCTAssertEqual(RewardAction(rawValue: preview?.action ?? ""), expected, raw)
        }
    }

    func testNeedsAttentionCoversOnlySentAndUnknown() {
        for (phase, expected) in [
            ("sent", true), ("unknown", true),
            ("pending", false), ("ready", false), ("frozen", false),
            ("confirmed", false), ("rejected", false),
        ] {
            let row = RewardOperationRow(json: ["id": "1", "action": "lottery_draw", "phase": phase, "open": true])
            XCTAssertEqual(row.needsAttention, expected, phase)
        }
    }

    // MARK: - 整包与 Mock

    func testResultLooksUpByAccountId() {
        let result = RewardStatusResult(json: [
            "readOnly": true,
            "accounts": [account(), account(overrides: ["accountId": "acc-2"])],
        ])
        XCTAssertEqual(result.accounts.count, 2)
        XCTAssertEqual(result.account("acc-2")?.accountId, "acc-2")
        XCTAssertNil(result.account("nope"))
        XCTAssertTrue(result.readOnly)
    }

    func testMockFixtureIsShapedLikeTheRealResponse() {
        // Mock 只用于预览与单测；这里锁住它至少能被自己的解析器读懂。
        let mock = RewardStatusResult.mock
        XCTAssertEqual(mock.accounts.count, 1)
        let state = mock.account("mock-account")
        XCTAssertNotNil(state)
        XCTAssertEqual(state?.claimableTiers.map(\.tier), ["14d"])
        XCTAssertEqual(state?.makeup.balance, 2)
        XCTAssertEqual(state?.awaitingVerification, 1)
        XCTAssertEqual(state?.recentOperations.first?.phaseLabel, "待核验")
    }

    func testAnEmptyPayloadDecodesToNothingRatherThanFakeRows() {
        let result = RewardStatusResult(json: [:])
        XCTAssertTrue(result.accounts.isEmpty)
        XCTAssertTrue(result.readOnly, "缺字段时默认只读是安全的一侧")
    }
}
