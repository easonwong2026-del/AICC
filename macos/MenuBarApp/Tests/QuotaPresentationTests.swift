import XCTest
@testable import AICCCore

/// 额度看板的纯展示契约：未知不为 0、来源不混算、每日奖励不冒充余额。
final class QuotaPresentationTests: XCTestCase {
    private func workerRow(_ overrides: [String: Any] = [:]) -> WorkerQuotaRow {
        var json: [String: Any] = [
            "accountId": "acc-1",
            "provider": "workbuddy",
            "nickname": "主账号",
            "source": "own",
            "status": "ok",
        ]
        for (key, value) in overrides { json[key] = value }
        return WorkerQuotaRow(json: json)!
    }

    private func card(_ overrides: [String: Any] = [:]) throws -> QuotaCard {
        var json: [String: Any] = [
            "id": "magpie-card-1",
            "provider": "codex",
            "provider_name": "Codex",
            "account_label": "main@example.com",
            "kind": "subscription",
            "items": [],
            "balance_parts": [],
            "last": false,
            "failed": false,
            "deferred": false,
            "stale": false,
        ]
        for (key, value) in overrides { json[key] = value }
        let data = try JSONSerialization.data(withJSONObject: json)
        return try JSONDecoder().decode(QuotaCard.self, from: data)
    }

    private func item(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var json: [String: Any] = [
            "name": "5 小时窗口",
            "used": 42.0,
            "remaining": 58.0,
            "remaining_text": "58%",
            "unlimited": false,
        ]
        for (key, value) in overrides { json[key] = value }
        return json
    }

    // MARK: 账号额度

    func testUnknownRemainingIsNotZero() {
        let display = AccountQuotaDisplay(row: workerRow())
        XCTAssertEqual(display.remainingValue, "未知")
        XCTAssertNil(display.unit)
        XCTAssertEqual(display.state, .live)
    }

    func testUnverifiedIsNotFailureAndShowsNoNumber() {
        let display = AccountQuotaDisplay(row: workerRow([
            "status": "unverified",
            "error": "该版本没有已证实的接口",
        ]))
        XCTAssertEqual(display.remainingValue, "—")
        XCTAssertEqual(display.state, .unverified)
        XCTAssertEqual(display.state.label, "暂不支持")
        XCTAssertEqual(display.state.tone, .neutral)
    }

    func testStaleTakesPrecedenceOverCachedAndKeepsWindowDays() {
        let display = AccountQuotaDisplay(row: workerRow([
            "remaining": 120,
            "unit": "credits",
            "cached": true,
            "stale": true,
            "usage": ["windowDays": 30, "requests": 42, "credits": 403.2, "truncated": false],
        ]))
        XCTAssertEqual(display.state, .stale)
        XCTAssertEqual(display.usagePrimary, "42 次请求")
        XCTAssertEqual(display.usageSecondary, "403.20 credits · 近 30 天")
    }

    func testUsageFailureDoesNotHideBalance() {
        let display = AccountQuotaDisplay(row: workerRow([
            "remaining": 100,
            "unit": "credits",
            "usageError": "usage timeout",
        ]))
        XCTAssertEqual(display.remainingValue, "100")
        XCTAssertEqual(display.usagePrimary, "读取失败")
        XCTAssertEqual(display.usageHelp, "usage timeout")
        XCTAssertEqual(display.state, .live)
    }

    /// Qoder 每日奖励只能出现在副行，不能进入剩余额度列。
    func testQoderCreditsNeverBecomeBalance() {
        let display = AccountQuotaDisplay(row: workerRow([
            "provider": "qoder_cn",
            "status": "ok",
            "credits": [
                "state": "claimable",
                "support": false,
                "supported": true,
                "today": ["credits": 100, "claimStatus": "claimable", "windowEnd": 1_763_000_000],
            ],
        ]))
        XCTAssertEqual(display.remainingValue, "未知")
        XCTAssertFalse(display.remainingValue.contains("100"))
        XCTAssertEqual(display.creditsBadge, "每日奖励 · 可领 100 Credits")
        XCTAssertTrue(display.creditsHelp?.contains("不是余额") == true)
        XCTAssertTrue(display.creditsHelp?.contains("活动窗口截止") == true)
        XCTAssertTrue(display.creditsNavigatesToCheckin)
    }

    func testLongNumberIsPreservedWithoutEllipsis() {
        let display = AccountQuotaDisplay(row: workerRow([
            "remaining": 123_456_789.5,
            "unit": "credits",
        ]))
        XCTAssertEqual(display.remainingValue, "123456789.50")
        XCTAssertFalse(display.remainingValue.contains("…"))
    }

    // MARK: Magpie 额度

    func testMagpiePercentageWindowShowsUsedAndCompactReset() throws {
        let card = try card([
            "items": [item(["reset": "2026-11-15 03:00"])],
        ])
        let rows = MagpieQuotaDisplayRow.rows(for: card, snapshotStale: false)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].remaining, "58%")
        XCTAssertNil(rows[0].unit)
        XCTAssertEqual(rows[0].usedText, "42%")
        XCTAssertEqual(rows[0].reset, "11-15 03:00")
        XCTAssertEqual(rows[0].state, .live)
        XCTAssertNil(rows[0].state.label)
    }

    func testMagpieCountWindowKeepsUnitAndLimit() throws {
        let card = try card([
            "items": [item([
                "name": "周窗口",
                "used": 6.0,
                "remaining": 94.0,
                "remaining_text": "4705",
                "unit": "credits",
                "limit": 5000,
            ])],
        ])
        let rows = MagpieQuotaDisplayRow.rows(for: card, snapshotStale: false)
        XCTAssertEqual(rows[0].remaining, "4705")
        XCTAssertEqual(rows[0].unit, "credits")
        XCTAssertEqual(rows[0].detail, "共 5000 credits")
    }

    func testMagpieBalanceCardKeepsParts() throws {
        let card = try card([
            "kind": "balance",
            "balance": "¥ 128.40",
            "balance_parts": [["label": "赠送", "text": "¥ 20.00", "percent": NSNull()]],
        ])
        let rows = MagpieQuotaDisplayRow.rows(for: card, snapshotStale: false)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].windowName, "余额")
        XCTAssertEqual(rows[0].remaining, "¥ 128.40")
        XCTAssertEqual(rows[0].detail, "赠送 ¥ 20.00")
        XCTAssertEqual(rows[0].state, .live)
    }

    func testMagpieStaleAndFailedStates() throws {
        let stale = try card(["items": [item()], "stale": true])
        XCTAssertEqual(
            MagpieQuotaDisplayRow.rows(for: stale, snapshotStale: false).first?.state,
            .stale
        )

        let failed = try card(["items": [], "failed": true])
        let rows = MagpieQuotaDisplayRow.rows(for: failed, snapshotStale: true)
        XCTAssertEqual(rows.count, 1)
        XCTAssertEqual(rows[0].windowName, "无额度窗口")
        XCTAssertEqual(rows[0].remaining, "—")
        XCTAssertEqual(rows[0].state, .failed)

        let fromStaleSnapshot = try card(["items": [item()]])
        XCTAssertEqual(
            MagpieQuotaDisplayRow.rows(for: fromStaleSnapshot, snapshotStale: true).first?.state,
            .stale
        )
    }

    func testMultipleWindowsProduceOneRowEach() throws {
        let card = try card([
            "items": [item(), item(["name": "周窗口", "used": 12.0, "remaining": 88.0, "remaining_text": "88%"])],
        ])
        let rows = MagpieQuotaDisplayRow.rows(for: card, snapshotStale: false)
        XCTAssertEqual(rows.count, 2)
        XCTAssertTrue(rows[0].isFirstRow)
        XCTAssertFalse(rows[1].isFirstRow)
        XCTAssertEqual(rows[1].windowName, "周窗口")
    }

    func testCreditsBadgeCoversUnsupportedAndAlready() {
        let unsupported = QoderCreditsInfo(json: [
            "state": "unsupported", "supported": false,
        ])!
        XCTAssertEqual(QuotaPresentation.creditsBadge(unsupported), "每日奖励 · 暂不支持")

        let already = QoderCreditsInfo(json: [
            "state": "already", "supported": true,
            "today": ["credits": 100.0],
        ])!
        XCTAssertEqual(QuotaPresentation.creditsBadge(already), "每日奖励 · 已领取")
    }
}
