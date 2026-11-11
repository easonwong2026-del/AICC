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

    // MARK: WorkBuddy 扫码登录

    func testLoginStartParsesVerificationUri() throws {
        let start = try XCTUnwrap(WorkbuddyLoginStart.parse([
            "loginId": "wb1b_abc",
            "verificationUri": "https://www.codebuddy.cn/login?state=1",
            "expiresIn": 600,
        ]))
        XCTAssertEqual(start.loginId, "wb1b_abc")
        XCTAssertEqual(start.expiresIn, 600)
        XCTAssertTrue(start.verificationUri.contains("codebuddy.cn"))
    }

    func testLoginStartRejectsIncompletePayload() {
        XCTAssertNil(WorkbuddyLoginStart.parse(["loginId": "x"]))
        XCTAssertNil(WorkbuddyLoginStart.parse(["verificationUri": "https://x"]))
        XCTAssertNil(WorkbuddyLoginStart.parse(["loginId": "", "verificationUri": "https://x"]))
    }

    func testLoginPhaseMapping() {
        XCTAssertEqual(WorkbuddyLoginPhase.parse(["state": "pending"]), .pending)
        XCTAssertEqual(WorkbuddyLoginPhase.parse(["state": "success", "reused": true]), .success(reused: true))
        XCTAssertEqual(WorkbuddyLoginPhase.parse(["state": "success"]), .success(reused: false))
        XCTAssertEqual(WorkbuddyLoginPhase.parse(["state": "cancelled"]), .cancelled)
        XCTAssertEqual(WorkbuddyLoginPhase.parse(["state": "expired"]), .expired)
        XCTAssertEqual(WorkbuddyLoginPhase.parse(["state": "finished"]), .finished)
    }

    func testLoginFailureKeepsKindAndMessage() {
        let phase = WorkbuddyLoginPhase.parse([
            "state": "failed",
            "error": ["kind": "domain_mismatch", "message": "档位不符"],
        ])
        XCTAssertEqual(phase, .failed(kind: "domain_mismatch", message: "档位不符"))
    }

    func testTerminalFlag() {
        XCTAssertFalse(WorkbuddyLoginPhase.pending.isTerminal)
        XCTAssertTrue(WorkbuddyLoginPhase.cancelled.isTerminal)
        XCTAssertTrue(WorkbuddyLoginPhase.success(reused: false).isTerminal)
    }

    func testExpiredTokenOverridesValidStatus() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row([
            "authStatus": "valid",
            "tokenExpiresAt": "2020-01-01T00:00:00Z",
        ])))
        XCTAssertTrue(account.isTokenExpired())
        XCTAssertEqual(account.displayAuthLabel, "已过期")
        XCTAssertEqual(account.displayAuthTone, .error)
    }

    func testFutureTokenKeepsValidStatus() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row([
            "authStatus": "valid",
            "tokenExpiresAt": "2099-01-01T00:00:00Z",
        ])))
        XCTAssertFalse(account.isTokenExpired())
        XCTAssertEqual(account.displayAuthLabel, "认证有效")
    }

    func testMissingExpiryDoesNotFakeExpiry() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row(["authStatus": "valid"])))
        XCTAssertFalse(account.isTokenExpired())
        XCTAssertEqual(account.displayAuthLabel, "认证有效")
    }

    // MARK: Magpie 导入

    func testBorrowedAccountIsMarkedAndLabelled() throws {
        let account = try XCTUnwrap(ConsoleAccount(json: row([
            "authStatus": "valid",
            "authSource": "magpie",
        ])))
        XCTAssertTrue(account.isBorrowed)
        XCTAssertEqual(account.sourceLabel, "Magpie")

        let own = try XCTUnwrap(ConsoleAccount(json: row(["authStatus": "valid"])))
        XCTAssertFalse(own.isBorrowed)
        XCTAssertNil(own.sourceLabel, "独立授权的账号不该有来源标签")
    }

    func testMagpieCandidateParsing() throws {
        let candidate = try XCTUnwrap(MagpieAccountCandidate.parse([
            "key": "workbuddy#2ntprc",
            "uid": "0a9b8c7d-aaaa-bbbb-cccc-ddddeeeeffff",
            "nickname": "一锅端",
            "expiresInHours": 385.6,
            "expired": false,
            "imported": false,
        ]))
        XCTAssertEqual(candidate.nickname, "一锅端")
        XCTAssertTrue(candidate.isAvailable)
        XCTAssertEqual(candidate.statusText, "约 16 天后过期")
    }

    func testMagpieCandidateStates() throws {
        let imported = try XCTUnwrap(MagpieAccountCandidate.parse([
            "key": "workbuddy", "uid": "u", "imported": true, "importedSource": "own",
        ]))
        XCTAssertFalse(imported.isAvailable)
        XCTAssertEqual(imported.statusText, "已独立授权")

        let expired = try XCTUnwrap(MagpieAccountCandidate.parse([
            "key": "workbuddy", "uid": "u", "expired": true, "expiresInHours": -3.0,
        ]))
        XCTAssertFalse(expired.isAvailable)
        XCTAssertEqual(expired.statusText, "令牌已过期")

        let soon = try XCTUnwrap(MagpieAccountCandidate.parse([
            "key": "workbuddy", "uid": "u", "expiresInHours": 5.4,
        ]))
        XCTAssertEqual(soon.statusText, "约 5 小时后过期")
    }

    func testMagpieCandidateRejectsIncompleteRows() {
        XCTAssertNil(MagpieAccountCandidate.parse(["uid": "u"]))
        XCTAssertNil(MagpieAccountCandidate.parse(["key": "workbuddy"]))
        XCTAssertNil(MagpieAccountCandidate.parse(["key": "", "uid": "u"]))
    }

    func testMagpiePreviewFiltersImportable() {
        let preview = MagpiePreview.parse([
            "available": true,
            "accounts": [
                ["key": "workbuddy", "uid": "u1", "nickname": "甲"],
                ["key": "workbuddy#a", "uid": "u2", "nickname": "乙", "imported": true],
                ["key": "workbuddy#b", "uid": "u3", "nickname": "丙", "expired": true],
            ],
        ])
        XCTAssertTrue(preview.available)
        XCTAssertEqual(preview.candidates.count, 3)
        XCTAssertEqual(preview.importable.map(\.nickname), ["甲"])
    }

    func testMagpiePreviewWithoutMagpie() {
        let preview = MagpiePreview.parse([
            "available": false,
            "message": "没有找到 Magpie 的登录数据",
            "accounts": [],
        ])
        XCTAssertFalse(preview.available)
        XCTAssertTrue(preview.importable.isEmpty)
        XCTAssertEqual(preview.message, "没有找到 Magpie 的登录数据")
    }

    func testImportResultParsing() {
        let result = MagpieImportResult.parse([
            "imported": 3,
            "skipped": [["key": "workbuddy", "reason": "already_authorized"]],
        ])
        XCTAssertEqual(result.imported, 3)
        XCTAssertEqual(result.skipped, 1)
    }

    func testQuotaSummaryWithoutSnapshotIsEmpty() {
        let summary = ConsoleQuotaSummary(snapshot: nil)
        XCTAssertTrue(summary.isEmpty)
        XCTAssertEqual(summary.cardCount, 0)
        XCTAssertFalse(summary.stale)
        XCTAssertNil(summary.updatedAt)
    }
}

// MARK: - Buddy 旅行

final class TravelModelsTests: XCTestCase {

    private func summaryJSON() -> [String: Any] {
        [
            "today": "2026-11-10",
            "auto": [
                "owner": "AICC.app 的账号助手进程（aicc-account-worker）",
                "runsOnlyWhileAppRunning": true,
                "departIntervalSecs": 1800,
                "claimIntervalSecs": 900,
                "enabledAccounts": 1,
                "busy": false,
                "lastTick": ["at": "2026-11-10T02:00:00Z"],
            ] as [String: Any],
            "accounts": [
                [
                    "accountId": "acc-1",
                    "uid": "uid-1",
                    "nickname": "工作号",
                    "authSource": "magpie",
                    "travelEnabled": true,
                    "travel": [
                        "label": "traveling",
                        "locationName": "咖啡馆",
                        "arriveAt": 1_794_000_000,
                        "claimed": false,
                        "dailyLimitReached": false,
                    ] as [String: Any],
                ],
                [
                    "accountId": "acc-2",
                    "uid": "uid-2",
                    "nickname": "备用号",
                    "authSource": "own",
                    "travelEnabled": false,
                    "travel": [
                        "label": "done_today",
                        "locationName": "健身房",
                        "claimed": true,
                        "rewardCredits": 5,
                        "claimedAt": "2026-11-10T00:12:00Z",
                        "dailyLimitReached": true,
                    ] as [String: Any],
                ],
                [
                    "accountId": "acc-3",
                    "uid": "uid-3",
                    "nickname": "空号",
                    "authSource": "own",
                    "travelEnabled": false,
                    "travel": NSNull(),
                ],
            ],
        ]
    }

    func testParsesSummary() throws {
        let summary = TravelSummary.parse(summaryJSON())
        XCTAssertEqual(summary.rows.count, 3)
        XCTAssertTrue(summary.automationRunning, "有账号开着自动旅行")
        XCTAssertEqual(summary.rows[0].nickname, "工作号")
        XCTAssertTrue(summary.rows[0].isBorrowed)

        let status = try XCTUnwrap(summary.rows[0].status)
        XCTAssertEqual(status.stage, .traveling)
        XCTAssertEqual(status.stageText, "旅行中")
        XCTAssertTrue(status.destinationText.contains("咖啡馆"))
        XCTAssertTrue(status.destinationText.contains("到达"))
        XCTAssertFalse(status.canDepart)
        XCTAssertFalse(status.canClaim)
    }

    func testClaimResultIsRendered() throws {
        let summary = TravelSummary.parse(summaryJSON())
        let status = try XCTUnwrap(summary.rows[1].status)
        XCTAssertEqual(status.stage, .doneToday)
        XCTAssertEqual(status.stageText, "今日已完成")
        XCTAssertEqual(status.claimText?.contains("+5 积分"), true)
        XCTAssertNil(status.warningText)
    }

    func testMissingSnapshotIsNilNotFabricated() throws {
        let summary = TravelSummary.parse(summaryJSON())
        XCTAssertNil(summary.rows[2].status, "没查过就没有状态，不许编")
        XCTAssertEqual(summary.rows[2].nickname, "空号")
    }

    func testAutoStatusExplainsHostDependency() throws {
        let summary = TravelSummary.parse(summaryJSON())
        let auto = try XCTUnwrap(summary.auto)
        XCTAssertTrue(auto.runsOnlyWhileAppRunning, "必须如实说明 App 退出即停")
        XCTAssertEqual(auto.intervalText, "派出 30 分钟 · 领取 15 分钟")
        XCTAssertEqual(auto.enabledAccounts, 1)
        XCTAssertEqual(auto.lastTickText.hasPrefix("上次检查"), true)
        XCTAssertTrue(auto.owner.contains("aicc-account-worker"))
    }

    func testStageAndAvailability() throws {
        let dispatchable = try XCTUnwrap(TravelStatus(json: ["label": "dispatchable"]))
        XCTAssertEqual(dispatchable.stage, .dispatchable)
        XCTAssertTrue(dispatchable.canDepart)

        XCTAssertEqual(TravelStatus(json: ["label": "no_buddy"])?.stageText ?? "", "无 Buddy")
        XCTAssertEqual(TravelStatus(json: ["label": "unsupported"])?.stageText ?? "", "不支持")
        XCTAssertEqual(TravelStatus(json: ["label": "weird"])?.stage ?? .doneToday, .unknown)

        let arrived = try XCTUnwrap(TravelStatus(json: ["label": "arrived", "claimed": false]))
        XCTAssertTrue(arrived.canClaim)
        let claimed = try XCTUnwrap(TravelStatus(json: ["label": "arrived", "claimed": true]))
        XCTAssertFalse(claimed.canClaim, "已领过的不再给按钮")

        let failed = try XCTUnwrap(TravelStatus(json: ["label": "failed", "lastError": "令牌过期"]))
        XCTAssertEqual(failed.destinationText, "令牌过期")
        XCTAssertEqual(failed.warningText, "令牌过期")
    }

    func testArrivedDoesNotPretendAFutureEta() throws {
        let arrived = try XCTUnwrap(TravelStatus(json: [
            "label": "arrived",
            "locationName": "咖啡馆",
            "arriveAt": 1_793_785_332,
            "rewardCredits": 5,
            "lastCheckedAt": "2026-11-10T13:20:00Z",
        ]))
        XCTAssertEqual(arrived.destinationText, "咖啡馆 · 已到达", "到达后不该再显示过去的预计时间")
        XCTAssertEqual(arrived.secondaryText, "可领取 +5 积分")

        let idle = try XCTUnwrap(TravelStatus(json: ["label": "dispatchable"]))
        XCTAssertEqual(idle.destinationText, "等待派出")
        XCTAssertEqual(idle.secondaryText, "还没检查")
    }

    func testConflictWinsOverPlainError() throws {
        let status = try XCTUnwrap(TravelStatus(json: [
            "label": "done_today",
            "lastError": "领取失败",
            "conflict": "奖励已在其它客户端领取",
        ]))
        XCTAssertEqual(status.warningText, "奖励已在其它客户端领取")
    }

    func testSyncResultSurfacesFirstEventMessage() {
        let result = TravelSyncResult.parse([
            "travel": ["label": "traveling"] as [String: Any],
            "events": [
                ["kind": "depart", "status": "success", "message": "已派往咖啡馆"],
            ] as [[String: Any]],
        ])
        XCTAssertEqual(result.summaryText, "已派往咖啡馆")
        XCTAssertEqual(result.status?.stage ?? .unknown, .traveling)
    }
}
