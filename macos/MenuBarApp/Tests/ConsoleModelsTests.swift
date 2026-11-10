import XCTest
@testable import AICCCore

/// 控制台视图模型的解析与派生。这些是纯函数，`swift test` 在 CI 上就能守住。
final class ConsoleModelsTests: XCTestCase {

    private func row(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var base: [String: Any] = [
            "id": "acc-1",
            "platform": "workbuddy",
            "uid": "uid-1",
            "nickname": "工作号",
            "authStatus": "valid",
            "checkinEnabled": true,
            "updatedAt": "2026-11-09T10:00:00Z",
        ]
        for (key, value) in overrides { base[key] = value }
        return base
    }

    func testParsesHelperPayload() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row()))
        XCTAssertEqual(account.id, "acc-1")
        XCTAssertEqual(account.platformName, "WorkBuddy")
        XCTAssertEqual(account.nickname, "工作号")
        XCTAssertEqual(account.authLabel, "认证有效")
        XCTAssertEqual(account.authTone, .ok)
        XCTAssertTrue(account.checkinEnabled)
        XCTAssertEqual(account.updatedAt, "2026-11-09T10:00:00Z")
    }

    func testQoderPlatformIsDistinguished() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row(["platform": "qoder_cn"])))
        XCTAssertEqual(account.platformName, "Qoder CN")
        XCTAssertEqual(account.symbolName, "chevron.left.forwardslash.chevron.right")
    }

    func testBlankNicknameFallsBack() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row(["nickname": "   "])))
        XCTAssertEqual(account.nickname, "未命名账号")
    }

    func testMissingIdentityIsRejected() {
        XCTAssertNil(ConsoleAccount(json: ["platform": "workbuddy", "uid": "u"]))
        XCTAssertNil(ConsoleAccount(json: ["id": "x", "uid": "u"]))
    }

    func testAuthToneMapping() throws {
        let cases: [(String, ConsoleAccount.AuthTone)] = [
            ("valid", .ok),
            ("expired", .error),
            ("invalid", .error),
            ("denied", .error),
            ("unknown", .neutral),
            ("something-new", .warning),
        ]
        for (status, expected) in cases {
            let account = try XCTUnwrap(ConsoleAccount(json: row(["authStatus": status])))
            XCTAssertEqual(account.authTone, expected, "authStatus=\(status)")
        }
    }

    func testSearchTextCoversNicknameAndUID() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row(["nickname": "QA", "uid": "U-900"])))
        XCTAssertTrue(account.searchText.contains("qa"))
        XCTAssertTrue(account.searchText.contains("u-900"))
    }

    func testPlatformFilter() throws {
        let wb = try XCTUnwrap(ConsoleAccount(json: row()))
        let qd = try XCTUnwrap(ConsoleAccount(json: row(["platform": "qoder_cn"])))
        XCTAssertTrue(ConsolePlatformFilter.all.includes(wb))
        XCTAssertTrue(ConsolePlatformFilter.all.includes(qd))
        XCTAssertTrue(ConsolePlatformFilter.workbuddy.includes(wb))
        XCTAssertFalse(ConsolePlatformFilter.workbuddy.includes(qd))
        XCTAssertTrue(ConsolePlatformFilter.qoder.includes(qd))
        XCTAssertFalse(ConsolePlatformFilter.qoder.includes(wb))
    }

    func testStatsAreDerivedFromRealRows() throws {
        let accounts = [
            try XCTUnwrap(ConsoleAccount(json: row())),
            try XCTUnwrap(ConsoleAccount(json: row(["id": "2", "uid": "u2", "authStatus": "expired"]))),
            try XCTUnwrap(ConsoleAccount(json: row(["id": "3", "uid": "u3", "platform": "qoder_cn", "authStatus": "unknown", "checkinEnabled": false]))),
        ]
        let stats = ConsoleOverviewStats(accounts: accounts)
        XCTAssertEqual(stats.total, 3)
        XCTAssertEqual(stats.workbuddy, 2)
        XCTAssertEqual(stats.qoder, 1)
        XCTAssertEqual(stats.healthy, 1)
        XCTAssertEqual(stats.attention, 1)
        XCTAssertEqual(stats.checkinEnabled, 2)
        XCTAssertFalse(stats.isEmpty)
    }

    /// 空账号库必须得到全 0，而不是任何占位数字。
    func testEmptyStatsAreZero() {
        let stats = ConsoleOverviewStats(accounts: [])
        XCTAssertEqual(stats.total, 0)
        XCTAssertEqual(stats.healthy, 0)
        XCTAssertEqual(stats.attention, 0)
        XCTAssertTrue(stats.isEmpty)
    }

    func testQuotaSummaryWithoutSnapshotIsEmpty() {
        let summary = ConsoleQuotaSummary(snapshot: nil)
        XCTAssertTrue(summary.isEmpty)
        XCTAssertEqual(summary.cardCount, 0)
        XCTAssertFalse(summary.stale)
        XCTAssertNil(summary.updatedAt)
    }
}
