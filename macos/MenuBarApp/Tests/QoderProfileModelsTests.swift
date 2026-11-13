import XCTest
@testable import AICCCore

/// Qoder Profile 模型：双版本、身份、快照状态、切换按钮文案。
///
/// 进程与数据库行为在 Rust 侧测试；这里只验"界面看到的字段是不是从真实数据来的"。
final class QoderProfileModelsTests: XCTestCase {

    private func row(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var base: [String: Any] = [
            "profileId": "qp_0123456789ab",
            "edition": "qoder_global",
            "displayName": "Global Account",
            "liveDir": "/Users/me/Library/Application Support/com.qoder.app.stable",
            "snapshotDir": "/Users/me/Library/Application Support/AICC-Dashboard/accounts/qoder/profiles/qp_0123456789ab",
            "authFile": "/snap/auth.v1.dat",
            "authStatus": "valid",
            "snapshotReady": true,
            "active": false,
            "switchEnabled": true,
            "managed": true,
            "runtime": ["state": "stopped"],
        ]
        for (key, value) in overrides { base[key] = value }
        return base
    }

    func testParsesGlobalProfile() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row()))
        XCTAssertEqual(profile.id, "qp_0123456789ab")
        XCTAssertEqual(profile.edition, "qoder_global")
        XCTAssertEqual(profile.editionName, "Qoder International")
        XCTAssertEqual(profile.authLabel, "已保存")
        XCTAssertEqual(profile.authTone, .ok)
        XCTAssertFalse(profile.runtime.isRunning)
        XCTAssertEqual(profile.runtimeLabel, "未运行")
        XCTAssertTrue(profile.snapshotReady)
    }

    func testParsesCNProfileSeparately() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "edition": "qoder_cn",
            "displayName": "工作账号",
            "liveDir": "/Users/me/Library/Application Support/com.qodercn.app",
            "authStatus": "missing",
            "snapshotReady": false,
        ])))
        XCTAssertEqual(profile.editionName, "Qoder CN")
        XCTAssertEqual(profile.authLabel, "未保存登录")
        XCTAssertEqual(profile.authTone, .neutral)
        XCTAssertEqual(profile.primaryActionTitle, "登录并保存")
    }

    func testIdentityPrefersUIDThenEmailAndNeverInvents() throws {
        let withUID = try XCTUnwrap(QoderProfile(json: row(["accountUid": "u-1", "email": "a@b.c"])))
        XCTAssertEqual(withUID.identityText, "u-1")
        XCTAssertEqual(withUID.identityLabel, "UID")

        let emailOnly = try XCTUnwrap(QoderProfile(json: row(["email": "a@b.c"])))
        XCTAssertEqual(emailOnly.identityText, "a@b.c")
        XCTAssertEqual(emailOnly.identityLabel, "邮箱")

        let none = try XCTUnwrap(QoderProfile(json: row()))
        XCTAssertNil(none.identityText, "没有可靠身份时必须留空，不能拿目录名顶替")
    }

    func testRuntimeStateAndActiveAccount() throws {
        let running = try XCTUnwrap(QoderProfile(json: row([
            "runtime": ["state": "running", "pid": 4242],
            "active": true,
        ])))
        XCTAssertTrue(running.runtime.isRunning)
        XCTAssertEqual(running.runtime.pid, 4242)
        XCTAssertEqual(running.runtimeLabel, "运行中（当前）")
        XCTAssertEqual(running.primaryActionTitle, "已在此账号")

        let idleActive = try XCTUnwrap(QoderProfile(json: row(["active": true])))
        XCTAssertEqual(idleActive.runtimeLabel, "当前账号")
        XCTAssertEqual(idleActive.primaryActionTitle, "打开")

        let idleOther = try XCTUnwrap(QoderProfile(json: row()))
        XCTAssertEqual(idleOther.primaryActionTitle, "切换并打开")
    }

    func testMissingSnapshotOffersLogin() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "snapshotReady": false,
            "authStatus": "missing",
        ])))
        XCTAssertEqual(profile.primaryActionTitle, "登录并保存")
        XCTAssertEqual(profile.authLabel, "未保存登录")
    }

    func testSwitchDisabledWinsOverEverything() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "switchEnabled": false,
            "switchBlockedReason": "认证快照不等于完整 Profile 隔离",
            "runtime": ["state": "running"],
            "active": true,
        ])))
        XCTAssertEqual(profile.primaryActionTitle, "切换未开放")
        XCTAssertEqual(profile.switchBlockedReason, "认证快照不等于完整 Profile 隔离")
    }

    func testUnknownRuntimeIsNotTreatedAsRunningOrStopped() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "runtime": ["state": "unknown", "pid": 4242],
        ])))
        XCTAssertFalse(profile.runtime.isRunning)
        XCTAssertTrue(profile.runtime.isUnknown)
        XCTAssertEqual(profile.runtimeLabel, "状态未知")
        XCTAssertEqual(profile.runtimeTone, .warning)
    }

    func testParallelLabelNeverClaimsForCN() throws {
        let cn = try XCTUnwrap(QoderEditionInfo(json: [
            "edition": "qoder_cn",
            "installed": false,
            "parallelInstances": "unknown",
            "switchEnabled": false,
        ]))
        XCTAssertEqual(cn.parallelLabel, "并行能力未复验")
        XCTAssertFalse(cn.switchEnabled)

        let global = try XCTUnwrap(QoderEditionInfo(json: [
            "edition": "qoder_global",
            "installed": true,
            "parallelInstances": "no",
            "switchEnabled": false,
            "switchBlockedReason": "未解决隔离",
        ]))
        XCTAssertEqual(global.parallelLabel, "实测不支持并行")
        XCTAssertEqual(global.switchBlockedReason, "未解决隔离")
    }

    func testReadOnlyStageNeverAdvertisesSwitch() throws {
        // 服务端现在恒发 switchEnabled=false；界面不能出现"可以切换"的文案。
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "switchEnabled": false,
            "switchBlockedReason": "账号切换未开放：只替换认证文件无法隔离 main.sqlite",
            "snapshotReady": true,
            "authStatus": "valid",
        ])))
        XCTAssertEqual(profile.primaryActionTitle, "切换未开放")
        XCTAssertFalse(profile.switchEnabled)
    }

    func testSnapshotIncompleteIsNotTreatedAsReady() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "snapshotReady": false,
            "snapshotIncomplete": true,
        ])))
        XCTAssertTrue(profile.snapshotIncomplete)
        XCTAssertFalse(profile.snapshotReady)
    }

    func testPendingSwitchIsParsedFromReadOnlyResponse() throws {
        let pending = try XCTUnwrap(QoderPendingSwitch(json: [
            "journalId": "sw_abc",
            "profileId": "qp_0123456789ab",
            "edition": "qoder_cn",
            "phase": "staged",
        ]))
        XCTAssertEqual(pending.id, "sw_abc")
        XCTAssertEqual(pending.editionName, "Qoder CN")
        XCTAssertEqual(pending.phase, "staged")
        XCTAssertNil(QoderPendingSwitch(json: ["profileId": "qp_1"]))
    }

    func testFingerprintNeverLeaksToken() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row(["authFingerprint": "9f1c2d3e"])))
        XCTAssertEqual(profile.fingerprintShort, "9f1c2d3e")
        let empty = try XCTUnwrap(QoderProfile(json: row()))
        XCTAssertEqual(empty.fingerprintShort, "—")
    }

    func testMissingRequiredFieldsDropTheRow() {
        XCTAssertNil(QoderProfile(json: ["edition": "qoder_global"]))
        XCTAssertNil(QoderProfile(json: ["profileId": "qp_1", "edition": "qoder_global"]))
    }

    func testEditionsAreEnumeratedWithStableNames() {
        XCTAssertEqual(QoderEdition.allCases.count, 2)
        XCTAssertEqual(QoderEdition.cn.rawValue, "qoder_cn")
        XCTAssertEqual(QoderEdition.global.rawValue, "qoder_global")
        XCTAssertEqual(QoderEdition.global.displayName, "Qoder International")
    }

    func testSectionMapping() {
        XCTAssertNil(ConsoleAccountSection.workbuddy.edition)
        XCTAssertEqual(ConsoleAccountSection.qoderCN.edition, .cn)
        XCTAssertEqual(ConsoleAccountSection.qoderGlobal.edition, .global)
        XCTAssertEqual(ConsoleAccountSection.allCases.count, 3)
    }

    func testFilteredSectionsRespectEdition() {
        XCTAssertTrue(ConsolePlatformFilter.all.showsWorkbuddy)
        XCTAssertTrue(ConsolePlatformFilter.all.showsQoder)
        XCTAssertFalse(ConsolePlatformFilter.workbuddy.showsQoder)
        XCTAssertNil(ConsolePlatformFilter.all.edition)
        XCTAssertEqual(ConsolePlatformFilter.qoder.edition, .cn)
        XCTAssertEqual(ConsolePlatformFilter.qoderGlobal.edition, .global)
    }

    func testEditionInfoSummaryAndSwitchMode() throws {
        let installed = try XCTUnwrap(QoderEditionInfo(json: [
            "edition": "qoder_global",
            "displayName": "Qoder International",
            "bundleId": "com.qoder.app",
            "installed": true,
            "clientVersion": "0.5.7",
            "dataDirs": ["/a"],
            "parallelInstances": "no",
            "switchMode": "snapshot",
            "verifiedOnThisMachine": true,
            "loggedIn": true,
            "loggedInEmail": "me@example.com",
        ]))
        XCTAssertEqual(installed.installSummary, "已安装 v0.5.7")
        XCTAssertEqual(installed.dataDirCount, 1)
        XCTAssertEqual(installed.switchMode, "snapshot")
        XCTAssertEqual(installed.currentAccountHint, "me@example.com")

        let missing = try XCTUnwrap(QoderEditionInfo(json: [
            "edition": "qoder_cn",
            "bundleId": "com.qodercn.app",
            "installed": false,
        ]))
        XCTAssertEqual(missing.installSummary, "未安装")
        XCTAssertNil(missing.currentAccountHint)
        XCTAssertFalse(missing.verifiedOnThisMachine)
    }

    func testSearchTextCoversNameAndIdentity() throws {
        let profile = try XCTUnwrap(QoderProfile(json: row([
            "displayName": "Global Account",
            "accountUid": "u-9",
        ])))
        XCTAssertTrue(profile.searchText.contains("global account"))
        XCTAssertTrue(profile.searchText.contains("qoder international"))
        XCTAssertTrue(profile.searchText.contains("u-9"))
    }
}
