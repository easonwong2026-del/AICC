import XCTest
@testable import AICCCore

/// `workbuddy.client.*` 的模型层（WB-SWITCH-1）。
///
/// 守四句话：
/// 1. "当前登录"只认 `live.uid`，不认 AICC 的选中项；
/// 2. 不能切换必须带**具体原因**，不画灰按钮了事；
/// 3. 失败分"已回滚"与"恢复失败"，不能含混成一句"切换失败"；
/// 4. 活跃未知（`nil`）不等于未活跃。
final class WorkbuddySwitchModelsTests: XCTestCase {

    private func status(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var json: [String: Any] = [
            "client": [
                "installed": true,
                "running": true,
                "authFile": "/tmp/workbuddy-desktop.info",
                "authFileExists": true,
                "logoutMarker": false,
            ],
            "live": [
                "uid": "uid-a",
                "loggedIn": true,
                "tokenForm": "envelope",
                "accountsInFile": 1,
            ],
            "candidates": [
                [
                    "accountId": "acc-a",
                    "uid": "uid-a",
                    "nickname": "甲",
                    "switchable": false,
                    "reason": "live",
                ],
                [
                    "accountId": "acc-b",
                    "uid": "uid-b",
                    "nickname": "乙",
                    "switchable": true,
                    "reason": "ok",
                ],
                [
                    "accountId": "acc-c",
                    "uid": "uid-c",
                    "nickname": "丙",
                    "switchable": false,
                    "reason": "magpie_no_refresh",
                ],
            ],
        ]
        for (key, value) in overrides { json[key] = value }
        return json
    }

    func testLiveAccountIsMatchedByUIDNotBySelection() {
        let model = WorkbuddyClientStatus(json: status())
        XCTAssertEqual(model.liveAccountID, "acc-a")
        XCTAssertTrue(model.live.loggedIn)

        // 客户端登录了 AICC 列表里没有的账号：不能硬塞给第一个账号。
        var json = status()
        json["live"] = ["uid": "uid-unknown", "loggedIn": true]
        XCTAssertNil(WorkbuddyClientStatus(json: json).liveAccountID)
    }

    func testRefusalCarriesAnActionableReason() {
        let model = WorkbuddyClientStatus(json: status())

        let live = model.candidate(forAccount: "acc-a")
        XCTAssertEqual(live?.switchable, false)
        XCTAssertEqual(live?.reason, WorkbuddySwitchReason.live)
        XCTAssertTrue(live?.reason.explanation.contains("当前登录") ?? false)

        let ok = model.candidate(forAccount: "acc-b")
        XCTAssertEqual(ok?.switchable, true)
        XCTAssertEqual(ok?.reason, WorkbuddySwitchReason.ok)

        let magpie = model.candidate(forAccount: "acc-c")
        XCTAssertEqual(magpie?.reason, WorkbuddySwitchReason.magpieNoRefresh)
        XCTAssertTrue(
            magpie?.reason.explanation.contains("刷新令牌") ?? false,
            "禁用原因必须说明缺什么，不能只说「不支持」"
        )

        // 未知 reason 落成"没有可用授权"，不能变成"可切换"。
        var json = status()
        json["candidates"] = [["accountId": "acc-x", "uid": "uid-x", "switchable": false, "reason": "new_code"]]
        let unknown = WorkbuddyClientStatus(json: json).candidate(forAccount: "acc-x")
        XCTAssertEqual(unknown?.reason, WorkbuddySwitchReason.noCredential)
    }

    func testUnreadableLoginIsNotReportedAsLoggedOut() {
        var json = status()
        json["live"] = ["loggedIn": false, "error": "不是合法 JSON：EOF"]
        let model = WorkbuddyClientStatus(json: json)
        XCTAssertFalse(model.live.loggedIn)
        XCTAssertTrue(model.live.summary.contains("读不到"), model.live.summary)

        json["live"] = ["loggedIn": false]
        XCTAssertEqual(WorkbuddyClientStatus(json: json).live.summary, "客户端当前未登录")
    }

    func testJournalPhaseDecidesWhetherRestoreIsOffered() {
        let verified = WorkbuddySwitchJournal(json: [
            "id": "j-1",
            "targetAccountId": "acc-b",
            "targetUid": "uid-b",
            "previousUid": "uid-a",
            "phase": "verified",
            "createdAt": "2026-11-20T00:10:00Z",
        ])
        XCTAssertEqual(verified.phaseText, "已确认切换")
        XCTAssertTrue(verified.canRestore)
        XCTAssertFalse(
            verified.needsRestoreOffer,
            "切换成功时不主动提示恢复：回到上一个账号应当走「切换到此账号」"
        )
        XCTAssertFalse(verified.isPending)
        XCTAssertNotNil(verified.createdAt)

        let pending = WorkbuddySwitchJournal(json: [
            "id": "j-2",
            "targetUid": "uid-b",
            "phase": "written",
        ])
        XCTAssertTrue(pending.isPending)
        XCTAssertFalse(pending.canRestore, "还没确认的切换不该提供「恢复上一个」")

        let restored = WorkbuddySwitchJournal(json: [
            "id": "j-3",
            "targetUid": "uid-b",
            "previousUid": "uid-a",
            "phase": "restored",
        ])
        XCTAssertFalse(restored.canRestore, "已经恢复过的记录不该再提供恢复入口")

        let unconfirmed = WorkbuddySwitchJournal(json: [
            "id": "j-4",
            "targetUid": "uid-b",
            "previousUid": "uid-a",
            "phase": "unconfirmed",
        ])
        XCTAssertTrue(unconfirmed.needsRestoreOffer, "没确认到的切换要提示恢复")
    }

    func testOutcomeSeparatesWrittenFromConfirmed() {
        let confirmed = WorkbuddySwitchOutcome(json: [
            "changed": true,
            "verified": true,
            "phase": "verified",
        ])
        XCTAssertTrue(confirmed.summary.contains("已接受"))

        let unconfirmed = WorkbuddySwitchOutcome(json: [
            "changed": true,
            "verified": false,
            "phase": "unconfirmed",
        ])
        XCTAssertTrue(unconfirmed.summary.contains("没能确认"), unconfirmed.summary)

        let noop = WorkbuddySwitchOutcome(json: ["changed": false])
        XCTAssertTrue(noop.summary.contains("没有改动"), noop.summary)
    }

    func testActivityEvidenceKeepsUnknownDistinctFromInactive() {
        let unknown = WorkbuddyActivityEvidence(json: ["state": "failed", "error": "超时"])
        XCTAssertEqual(unknown.todayActive, nil)
        XCTAssertEqual(unknown.activeText, "活跃未知")
        XCTAssertTrue(unknown.summary.contains("查不到"), unknown.summary)

        let inactive = WorkbuddyActivityEvidence(json: [
            "todayActive": false,
            "streakDays": 0,
        ])
        XCTAssertEqual(inactive.activeText, "今日未活跃")
        XCTAssertEqual(inactive.streakText, "连登 0 天")
    }

    func testRestoreOutcomePrefersBackendDetail() {
        let ok = WorkbuddyRestoreOutcome(json: [
            "restored": true,
            "uid": "uid-a",
            "detail": "已恢复到切换前的登录态",
        ])
        XCTAssertTrue(ok.restored)
        XCTAssertEqual(ok.summary, "已恢复到切换前的登录态")

        let failed = WorkbuddyRestoreOutcome(json: ["restored": false])
        XCTAssertEqual(failed.summary, "恢复未完成")
    }
}
