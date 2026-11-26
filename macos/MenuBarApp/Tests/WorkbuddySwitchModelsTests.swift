import XCTest
@testable import AICCCore

/// `workbuddy.client.*` 的模型层（WB-SWITCH-1 / #112）。
///
/// 守五句话：
/// 1. "当前登录"只认 `live.uid`，不认 AICC 的选中项；
/// 2. 自有授权与 Magpie 临时授权必须能一眼分开（按钮、标记、到期时间）；
/// 3. 临时登录即便成功，也要一直给"恢复原账号"入口；
/// 4. 不能切换必须带**具体原因**，不画灰按钮了事；
/// 5. 失败分"已回滚"与"恢复失败"，不能含混成一句"切换失败"。
final class WorkbuddySwitchModelsTests: XCTestCase {

    private func iso(_ offset: TimeInterval) -> String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.string(from: Date().addingTimeInterval(offset))
    }

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
                    "mode": "own_refresh",
                ],
                [
                    "accountId": "acc-c",
                    "uid": "uid-c",
                    "nickname": "丙",
                    "switchable": true,
                    "reason": "ok",
                    "mode": "magpie_access_temporary",
                    "temporary": true,
                    "accessExpiresAt": iso(3_600),
                    "remainingSeconds": 3_600,
                ],
                [
                    "accountId": "acc-d",
                    "uid": "uid-d",
                    "nickname": "丁",
                    "switchable": false,
                    "reason": "magpie_too_short",
                    "reasonDetail": "Magpie 访问令牌只剩 5 分钟，不足以完成一次临时切换",
                ],
            ],
            "restoreAvailable": false,
        ]
        for (key, value) in overrides { json[key] = value }
        return json
    }

    func testLiveAccountIsMatchedByUIDNotBySelection() {
        let model = WorkbuddyClientStatus(json: status())
        XCTAssertEqual(model.liveAccountID, "acc-a")
        XCTAssertTrue(model.live.loggedIn)
        XCTAssertFalse(model.live.temporary)

        var json = status()
        json["live"] = ["uid": "uid-unknown", "loggedIn": true]
        XCTAssertNil(WorkbuddyClientStatus(json: json).liveAccountID)
    }

    func testTheTwoModesUseDifferentWords() {
        let model = WorkbuddyClientStatus(json: status())

        let own = model.candidate(forAccount: "acc-b")
        XCTAssertEqual(own?.mode, WorkbuddySwitchMode.ownRefresh)
        XCTAssertEqual(own?.temporary, false)
        XCTAssertEqual(own?.buttonTitleForTest, "切换到此账号")
        XCTAssertNil(own?.temporaryNote, "自有授权不该出现有效期提示")

        let magpie = model.candidate(forAccount: "acc-c")
        XCTAssertEqual(magpie?.mode, WorkbuddySwitchMode.magpieTemporary)
        XCTAssertEqual(magpie?.temporary, true)
        XCTAssertEqual(magpie?.buttonTitleForTest, "临时切换到此账号")
        XCTAssertEqual(magpie?.temporaryNote?.contains("剩余 1 小时"), true)
        XCTAssertNotNil(magpie?.accessExpiresAt)
    }

    func testRefusalCarriesAnActionableReason() {
        let model = WorkbuddyClientStatus(json: status())

        let live = model.candidate(forAccount: "acc-a")
        XCTAssertEqual(live?.switchable, false)
        XCTAssertEqual(live?.reason, WorkbuddySwitchReason.live)
        XCTAssertTrue(live?.reason.explanation.contains("当前登录") ?? false)

        let tooShort = model.candidate(forAccount: "acc-d")
        XCTAssertEqual(tooShort?.reason, WorkbuddySwitchReason.magpieTooShort)
        // Rust 给的补充说明优先于固定文案。
        XCTAssertEqual(tooShort?.reasonDetail?.contains("5 分钟"), true)
        XCTAssertTrue(tooShort?.reason.isActionableElsewhere == false)

        // 未知 reason 落成"没有可用授权"，不能变成"可切换"。
        var json = status()
        json["candidates"] = [[
            "accountId": "acc-x", "uid": "uid-x", "switchable": false, "reason": "new_code",
        ]]
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

    /// 临时登录：左栏要同时显示到期与"恢复原账号"，不能只留在详情里。
    func testTemporaryLiveLoginShowsExpiryAndKeepsTheRestoreOffer() {
        var json = status()
        json["live"] = [
            "uid": "uid-c",
            "loggedIn": true,
            "temporary": true,
            "temporaryAccountId": "acc-c",
            "accessExpiresAt": iso(2_400),
            "remainingSeconds": 2_400,
            "safeRestoreUntil": iso(600),
        ]
        json["temporaryRestore"] = [
            "id": "j-1",
            "targetAccountId": "acc-c",
            "targetUid": "uid-c",
            "previousUid": "uid-a",
            "phase": "verified",
            "temporary": true,
            "authSource": "magpie",
            "accessExpiresAt": iso(2_400),
            "restoreState": "pending",
        ]
        json["restoreAvailable"] = true

        let model = WorkbuddyClientStatus(json: json)
        XCTAssertTrue(model.live.temporary)
        XCTAssertEqual(model.live.temporarySummary?.contains("临时登录"), true)
        XCTAssertEqual(model.live.temporarySummary?.contains("建议立即恢复"), false)
        XCTAssertEqual(model.restoreAvailable, true)
        XCTAssertEqual(model.restoreOffer?.id, "j-1", "临时登录成功后也必须一直能恢复")

        let journal = model.temporaryRestore
        XCTAssertEqual(journal?.temporary, true)
        XCTAssertEqual(journal?.phaseText, "已临时切换")
        XCTAssertEqual(journal?.needsRestoreOffer, true)
        XCTAssertEqual(journal?.temporaryNote?.contains("临时登录"), true)
    }

    /// 若客户端当前已登录别的账号，历史未结算的临时切换/最近切换不得贴到当前账号上。
    func testStaleTemporaryRestoreForDifferentLiveAccountIsIgnored() {
        var json = status()
        json["live"] = [
            "uid": "uid-a",
            "loggedIn": true,
            "temporary": false,
        ]
        json["temporaryRestore"] = [
            "id": "j-1",
            "targetAccountId": "acc-c",
            "targetUid": "uid-c",
            "previousUid": "uid-b",
            "phase": "verified",
            "temporary": true,
            "authSource": "magpie",
            "accessExpiresAt": iso(2_400),
            "restoreState": "pending",
        ]
        json["lastSwitch"] = [
            "id": "j-1",
            "targetAccountId": "acc-c",
            "targetUid": "uid-c",
            "previousUid": "uid-b",
            "phase": "verified",
            "temporary": true,
            "authSource": "magpie",
            "accessExpiresAt": iso(2_400),
            "restoreState": "pending",
        ]
        json["restoreAvailable"] = false

        let model = WorkbuddyClientStatus(json: json)
        XCTAssertNil(
            model.restoreOffer,
            "客户端已回到原账号 uid-a，不应把 uid-c 的临时登录恢复挂到 uid-a 头上"
        )
    }

    /// 临时令牌过期导致客户端掉线（loggedIn == false）时，必须保留恢复入口。
    func testTemporaryRestoreStaysAvailableWhenClientLoggedOut() {
        var json = status()
        json["live"] = [
            "uid": "uid-c",
            "loggedIn": false,
            "temporary": false,
        ]
        json["temporaryRestore"] = [
            "id": "j-1",
            "targetAccountId": "acc-c",
            "targetUid": "uid-c",
            "previousUid": "uid-a",
            "phase": "verified",
            "temporary": true,
            "authSource": "magpie",
            "accessExpiresAt": iso(-60),
            "restoreState": "pending",
        ]
        json["restoreAvailable"] = true

        let model = WorkbuddyClientStatus(json: json)
        XCTAssertEqual(model.restoreOffer?.id, "j-1")
    }

    /// 自有授权的成功切换不主动提示恢复；恢复过（done）或被取代（superseded）也不再提示。
    func testRestoreOfferOnlyWhenThereIsSomethingToGoBackTo() {
        let verifiedOwn = WorkbuddySwitchJournal(json: [
            "id": "j-own",
            "targetUid": "uid-b",
            "previousUid": "uid-a",
            "phase": "verified",
            "temporary": false,
            "restoreState": "pending",
        ])
        XCTAssertTrue(verifiedOwn.canRestore)
        XCTAssertFalse(verifiedOwn.needsRestoreOffer)

        let verifiedTemporary = WorkbuddySwitchJournal(json: [
            "id": "j-magpie",
            "targetUid": "uid-c",
            "previousUid": "uid-a",
            "phase": "verified",
            "temporary": true,
            "restoreState": "pending",
        ])
        XCTAssertTrue(verifiedTemporary.needsRestoreOffer)

        let restored = WorkbuddySwitchJournal(json: [
            "id": "j-done",
            "targetUid": "uid-c",
            "previousUid": "uid-a",
            "phase": "verified",
            "temporary": true,
            "restoreState": "done",
        ])
        XCTAssertFalse(restored.needsRestoreOffer, "已经恢复过就不该再提示")

        let pending = WorkbuddySwitchJournal(json: [
            "id": "j-2",
            "targetUid": "uid-b",
            "phase": "written",
        ])
        XCTAssertTrue(pending.isPending)
        XCTAssertFalse(pending.canRestore, "还没确认的切换不该提供「恢复上一个」")
    }

    func testOutcomeSeparatesWrittenFromConfirmedAndMentionsTheDeadline() {
        let confirmed = WorkbuddySwitchOutcome(json: [
            "changed": true,
            "verified": true,
            "phase": "verified",
            "mode": "own_refresh",
            "temporary": false,
        ])
        XCTAssertTrue(confirmed.summary.contains("已接受"))
        XCTAssertFalse(confirmed.summary.contains("恢复原账号"))

        let temporary = WorkbuddySwitchOutcome(json: [
            "changed": true,
            "verified": true,
            "phase": "verified",
            "mode": "magpie_access_temporary",
            "temporary": true,
            "accessExpiresAt": iso(1_800),
            "remainingSeconds": 1_800,
            "authAccepted": "accepted",
        ])
        XCTAssertTrue(temporary.summary.contains("恢复原账号"), temporary.summary)
        XCTAssertTrue(temporary.summary.contains("30 分钟"), temporary.summary)

        let rejected = WorkbuddySwitchOutcome(json: [
            "changed": true,
            "verified": true,
            "mode": "magpie_access_temporary",
            "temporary": true,
            "authAccepted": "rejected",
            "authAcceptedDetail": "服务端拒绝了这份令牌（HTTP 401）",
        ])
        XCTAssertTrue(rejected.summary.contains("401"), rejected.summary)
        // 服务端明确拒绝时，文案不能还说"客户端已接受"——那是把客户端回读
        // 当成服务端接受了这份令牌，用户会以为切换成功、实际每次请求都 401。
        XCTAssertFalse(rejected.summary.contains("已接受"), rejected.summary)
        XCTAssertTrue(rejected.summary.contains("服务端拒绝"), rejected.summary)

        let unconfirmed = WorkbuddySwitchOutcome(json: [
            "changed": true,
            "verified": false,
            "phase": "unconfirmed",
        ])
        XCTAssertTrue(unconfirmed.summary.contains("没能确认"), unconfirmed.summary)

        let noop = WorkbuddySwitchOutcome(json: ["changed": false])
        XCTAssertTrue(noop.summary.contains("没有改动"), noop.summary)
    }

    func testServerAcceptanceIsATriState() {
        XCTAssertEqual(WorkbuddyAuthAccepted(raw: "accepted"), .accepted)
        XCTAssertEqual(WorkbuddyAuthAccepted(raw: "rejected"), .rejected)
        XCTAssertEqual(WorkbuddyAuthAccepted(raw: "unknown"), .unknown)
        XCTAssertEqual(WorkbuddyAuthAccepted(raw: nil), .unknown, "没证据就是未知，不是接受")
        XCTAssertTrue(WorkbuddyAuthAccepted.rejected.isBad)
        XCTAssertFalse(WorkbuddyAuthAccepted.unknown.isBad)
    }

    func testRemainingTimeNeverTurnsUnknownIntoZero() {
        XCTAssertEqual(WorkbuddyTime.remaining(nil), "有效期未知")
        XCTAssertEqual(WorkbuddyTime.remaining(0), "已过期")
        XCTAssertEqual(WorkbuddyTime.remaining(59), "剩余 1 分钟")
        XCTAssertEqual(WorkbuddyTime.remaining(1_800), "剩余 30 分钟")
        XCTAssertEqual(WorkbuddyTime.remaining(7_200), "剩余 2 小时")
        XCTAssertEqual(WorkbuddyTime.remaining(5_400), "剩余 1 小时 30 分")
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

private extension WorkbuddySwitchCandidate {
    /// `buttonTitle` 在 `mode` 上；没有 mode 时按自有授权兜底。
    var buttonTitleForTest: String {
        (mode ?? .ownRefresh).buttonTitle
    }
}
