import Foundation
import XCTest
@testable import AICCCore

/// 签到视图模型的解析与文案（PR #69）。
///
/// 这些用例只用 JSON 字面量，不联网、不启动助手 —— 它们钉住的是
/// "界面上那句话到底怎么来的"，而不是网络行为（那由 Rust Mock 覆盖）。
final class CheckinModelsTests: XCTestCase {

    // MARK: - 状态

    func testStateParsingCoversEveryWorkerState() {
        XCTAssertEqual(CheckinState(raw: "claimed"), .claimed)
        XCTAssertEqual(CheckinState(raw: "already"), .already)
        XCTAssertEqual(CheckinState(raw: "unchecked"), .unchecked)
        XCTAssertEqual(CheckinState(raw: "unknown"), .unknown)
        XCTAssertEqual(CheckinState(raw: "unauthorized"), .unauthorized)
        XCTAssertEqual(CheckinState(raw: "no_credential"), .noCredential)
        XCTAssertEqual(CheckinState(raw: "unsupported"), .unsupported)
        XCTAssertEqual(CheckinState(raw: "stopped"), .stopped)
        // 认不出来的状态必须是"未知"，不能默认成"未签到"或"已签到"。
        XCTAssertEqual(CheckinState(raw: "something_new"), .unknown)
        XCTAssertEqual(CheckinState(raw: nil), .unknown)
    }

    func testOnlyClaimedAndAlreadyCountAsChecked() {
        XCTAssertTrue(CheckinState.claimed.isChecked)
        XCTAssertTrue(CheckinState.already.isChecked)
        XCTAssertFalse(CheckinState.unchecked.isChecked)
        XCTAssertFalse(CheckinState.unknown.isChecked)
        XCTAssertFalse(CheckinState.failed.isChecked)
        XCTAssertFalse(CheckinState.disabled.isChecked)
    }

    func testProblemStatesNeedAttention() {
        XCTAssertTrue(CheckinState.failed.isProblem)
        XCTAssertTrue(CheckinState.unknown.isProblem)
        XCTAssertTrue(CheckinState.unauthorized.isProblem)
        XCTAssertTrue(CheckinState.noCredential.isProblem)
        XCTAssertFalse(CheckinState.claimed.isProblem)
        XCTAssertFalse(CheckinState.unchecked.isProblem)
    }

    // MARK: - 奖励

    func testUnknownRewardShowsADashInsteadOfZero() {
        let row = CheckinAccountRow(
            accountId: "a", nickname: "卡里", enabled: true, state: .already
        )
        XCTAssertEqual(row.creditsText, "—", "不知道奖励就不许写 0 或 100")
    }

    func testAConfirmedRewardIsPrefixedWithAPlus() {
        let row = CheckinAccountRow(
            accountId: "a", nickname: "卡里", enabled: true, state: .claimed, credits: 100
        )
        XCTAssertEqual(row.creditsText, "+100")
    }

    // MARK: - 三种"已签到"

    func testAnAiccClaimIsDescribedAsVerified() {
        let row = CheckinAccountRow(
            accountId: "a", nickname: "卡里", enabled: true, state: .claimed, credits: 100
        )
        XCTAssertTrue(row.detailText.contains("回查确认"))
    }

    func testAnEarlierAiccClaimIsNotConfusedWithAnotherClient() {
        let aicc = CheckinAccountRow(
            accountId: "a", nickname: "卡里", enabled: true, state: .already,
            claimedByAiccToday: true
        )
        XCTAssertTrue(aicc.detailText.contains("AICC"), aicc.detailText)
        XCTAssertFalse(aicc.detailText.contains("Magpie"), aicc.detailText)

        let other = CheckinAccountRow(
            accountId: "a", nickname: "卡里", enabled: true, state: .already
        )
        XCTAssertTrue(other.detailText.contains("Magpie"), other.detailText)
    }

    // MARK: - 汇总

    func testFleetSummaryComesFromTheServerCounts() {
        let json: [String: Any] = [
            "today": "2026-11-13",
            "counts": ["accounts": 4, "checked": 3, "unchecked": 1, "enabled": 3, "credit": 200],
            "accounts": [],
        ]
        let status = CheckinFleetStatus(json: json)
        XCTAssertEqual(status.headline, "今日 3/4 已签到")
        XCTAssertEqual(status.enabledNote, "自动开启 3 个账号")
        XCTAssertEqual(status.creditNote, "今日 AICC 签到 +200")
        XCTAssertEqual(status.today, "2026-11-13")
    }

    func testAClosedSwitchIsSaidPlainly() {
        let status = CheckinFleetStatus(json: ["counts": ["accounts": 2, "enabled": 0]])
        XCTAssertEqual(status.enabledNote, "未开启自动签到")
        XCTAssertNil(status.creditNote, "没有到账积分就不显示那一行")
    }

    func testRowsRequiringActionSortFirst() {
        let json: [String: Any] = [
            "accounts": [
                ["accountId": "1", "nickname": "B", "state": "claimed"],
                ["accountId": "2", "nickname": "C", "state": "unchecked", "enabled": true],
                ["accountId": "3", "nickname": "A", "state": "failed"],
            ]
        ]
        let status = CheckinFleetStatus(json: json)
        XCTAssertEqual(status.sortedRows.map(\.nickname), ["A", "C", "B"])
        XCTAssertEqual(status.pendingRows.map(\.accountId), ["2"])
    }

    func testAnAccountWithoutAnIdIsDropped() {
        let status = CheckinFleetStatus(json: ["accounts": [["nickname": "没有 id"]]])
        XCTAssertTrue(status.rows.isEmpty, "没有稳定 id 的行不能进列表")
    }

    // MARK: - 调度

    func testNextRunTextSaysTomorrowWhenTodayIsDone() {
        let now = CheckinClock.parse("2026-11-13T10:00:00+08:00")!
        let tomorrow = CheckinClock.parse("2026-11-14T09:05:00+08:00")!
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        let auto = CheckinAutoStatus(json: ["nextRunAt": formatter.string(from: tomorrow)])
        XCTAssertEqual(auto.nextRunText(now: now), "明天 09:05")
    }

    func testTheSchedulerOwnershipIsStatedHonestly() {
        let auto = CheckinAutoStatus(json: ["enabled": true, "enabledAccounts": 2])
        XCTAssertTrue(auto.ownershipNote.contains("退出即停止"))
        XCTAssertEqual(auto.nextRunText(), "等待下一次检查")
    }

    func testATickSkipIsTranslatedForHumans() {
        XCTAssertEqual(
            CheckinTickResult(json: ["skipped": "no_accounts_enabled"]).skippedText,
            "没有账号开启自动签到"
        )
        XCTAssertEqual(
            CheckinTickResult(json: ["skipped": "already_running"]).skippedText,
            "上一轮还在跑，稍后再试"
        )
        XCTAssertNil(CheckinTickResult(json: ["summary": "签到 1 个"]).skippedText)
    }

    // MARK: - 历史与时间

    func testHistoryRowsCarryTheirSourceAndStatus() {
        let row = CheckinHistoryRow(json: [
            "localDate": "2026-11-13",
            "status": "claimed",
            "credits": 100,
            "checkedAt": "2026-11-13T01:12:00Z",
            "executionSource": "auto",
        ])
        XCTAssertNotNil(row)
        XCTAssertEqual(row?.statusText, "已签到")
        XCTAssertEqual(row?.sourceText, "自动")
        XCTAssertEqual(row?.credits, 100)
        XCTAssertNil(CheckinHistoryRow(json: ["status": "claimed"]), "没有日期不能当历史记录")
    }

    func testTimestampsParseWithAndWithoutFractionalSeconds() {
        XCTAssertNotNil(CheckinClock.parse("2026-11-13T01:12:00Z"))
        XCTAssertNotNil(CheckinClock.parse("2026-11-13T01:12:00.123Z"))
        XCTAssertNil(CheckinClock.parse("今天早上"))
    }

    func testDatesAreShownInBeijingTimeRegardlessOfTheMachine() {
        // 01:12Z = 北京时间 09:12。
        let date = CheckinClock.parse("2026-11-13T01:12:00Z")!
        XCTAssertEqual(CheckinClock.time(date), "09:12")
        XCTAssertEqual(CheckinClock.beijingDay(date), "2026-11-13")
    }
}
