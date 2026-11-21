import XCTest
@testable import AICCCore

/// CRED-4：迁移面板的解析契约。
///
/// 界面只说后端给出的结论，所以这里的重点是**不能编造**：
/// 缺字段就返回 nil（界面显示"数据不完整"），而不是默认 0 假装正常。
final class CredentialMigrationModelsTests: XCTestCase {
    func testInventorySummaryAndItemsParse() throws {
        let state = try XCTUnwrap(CredentialMigrationState.parse([
            "summary": [
                "migratable": 2,
                "alreadyJson": 3,
                "magpie": 1,
                "blocked": 1,
                "rollbackable": 2,
                "total": 7,
            ],
            "items": [
                [
                    "domain": "workbuddy",
                    "platform": "workbuddy",
                    "accountId": "acc-1",
                    "nickname": "旧账号",
                    "source": "own",
                    "storage": "keychain",
                    "reference": "keychain:workbuddy/acc-1/own/access",
                    "migratable": true,
                ],
                [
                    "domain": "qoder",
                    "platform": "qoder_cn",
                    "accountId": "qa-1",
                    "nickname": "",
                    "source": "own_pat",
                    "storage": "localjson",
                    "migratable": false,
                    "blocked": "already_json",
                ],
            ],
        ]))

        XCTAssertEqual(state.summary.migratable, 2)
        XCTAssertEqual(state.summary.alreadyJSON, 3)
        XCTAssertEqual(state.summary.rollbackable, 2)
        XCTAssertTrue(state.summary.needsAttention)

        XCTAssertEqual(state.items.count, 2)
        XCTAssertTrue(state.items[0].migratable)
        XCTAssertEqual(state.items[0].storageText, "钥匙串")
        XCTAssertNil(state.items[0].blockedText)
        XCTAssertEqual(state.items[0].rollbackable, false)
        XCTAssertEqual(state.items[1].blockedText, "已经是本地 JSON")
        XCTAssertEqual(state.items[1].storageText, "本地 JSON")
        // 没有昵称时退回账号 id，不显示空白行。
        XCTAssertEqual(state.items[1].nickname, "")
    }

    func testInventoryWithoutSummaryIsRejectedInsteadOfGuessed() {
        XCTAssertNil(CredentialMigrationState.parse(["items": []]))
    }

    /// 一条都不需要迁移时，面板不该催用户操作。
    func testSummaryNeedsNoAttentionWhenEverythingIsAlreadyJson() {
        let summary = CredentialMigrationSummary(
            migratable: 0,
            alreadyJSON: 4,
            magpie: 2,
            blocked: 0,
            rollbackable: 0,
            total: 6
        )
        XCTAssertFalse(summary.needsAttention)
    }

    /// 原生 JSON（没有迁移记录）不算可回滚：面板的按钮必须跟着这个数走。
    func testRollbackRequiresARecord() {
        let summary = CredentialMigrationSummary(
            migratable: 0,
            alreadyJSON: 3,
            magpie: 0,
            blocked: 0,
            rollbackable: 0,
            total: 3
        )
        XCTAssertEqual(summary.rollbackable, 0, "没有迁移记录就不能回滚")
    }

    func testVerifyReportSurfacesProblems() throws {
        let clean = try XCTUnwrap(CredentialVerifyReport.parse([
            "ok": 3,
            "broken": 0,
            "pendingKeychain": 1,
            "orphans": [],
            "brokenFiles": 0,
        ]))
        XCTAssertFalse(clean.hasProblems)
        XCTAssertEqual(clean.pendingKeychain, 1)

        let dirty = try XCTUnwrap(CredentialVerifyReport.parse([
            "ok": 2,
            "broken": 1,
            "pendingKeychain": 0,
            "orphans": [["secretId": "abc"]],
            "brokenFiles": 1,
        ]))
        XCTAssertTrue(dirty.hasProblems)
        XCTAssertEqual(dirty.orphanCount, 1)
    }

    func testVerifyWithoutOkFieldIsRejected() {
        XCTAssertNil(CredentialVerifyReport.parse(["broken": 0]))
    }

    /// CRED-5：借用授权还指着钥匙串时，后端给 `needs_relink` + `action: relink`。
    ///
    /// 面板必须把它当成"可修复"，既不能显示成"由 Magpie 管理"（会让用户以为已经好了），
    /// 也不能算进"可迁移"（那会把 Magpie 的令牌复制成明文文件）。
    func testNeedsRelinkIsItsOwnStateWithAnAction() throws {
        let state = try XCTUnwrap(CredentialMigrationState.parse([
            "summary": [
                "migratable": 0,
                "alreadyJson": 0,
                "magpie": 0,
                "needsRelink": 2,
                "blocked": 0,
                "rollbackable": 0,
                "total": 2,
            ],
            "migratedRecords": 3,
            "items": [
                [
                    "domain": "workbuddy",
                    "accountId": "acc-1",
                    "source": "magpie",
                    "storage": "keychain",
                    "migratable": false,
                    "blocked": "needs_relink",
                    "action": "relink",
                ]
            ],
        ]))

        XCTAssertEqual(state.summary.needsRelink, 2)
        XCTAssertEqual(state.migratedRecords, 3)
        XCTAssertTrue(state.summary.needsAttention, "需要重新关联时面板必须提示")
        XCTAssertEqual(state.items[0].action, "relink")
        XCTAssertEqual(state.items[0].blockedText, "借用授权仍指向钥匙串，需重新关联")
        XCTAssertFalse(state.items[0].migratable)
    }

    /// 已迁移条数与"本地 JSON 条数"不是一个数：前者只认迁移记录。
    func testMissingNeedsRelinkDefaultsToZeroForOlderWorkers() throws {
        let state = try XCTUnwrap(CredentialMigrationState.parse([
            "summary": [
                "migratable": 1,
                "alreadyJson": 0,
                "magpie": 0,
                "blocked": 0,
                "rollbackable": 0,
                "total": 1,
            ],
            "items": [],
        ]))
        XCTAssertEqual(state.summary.needsRelink, 0, "老助手不返回这个字段")
        XCTAssertEqual(state.migratedRecords, 0)
        XCTAssertEqual(state.items.count, 0)
    }

    /// 回滚后副本没删掉：要能被看见，但不算"有故障"（记录还在，下次回滚会重试）。
    func testVerifyReportsPendingCleanupWithoutCallingItBroken() throws {
        let report = try XCTUnwrap(CredentialVerifyReport.parse([
            "ok": 2,
            "broken": 0,
            "pendingKeychain": 0,
            "orphans": [],
            "brokenFiles": 0,
            "pendingCleanup": [["domain": "workbuddy", "accountId": "a"]],
        ]))
        XCTAssertEqual(report.pendingCleanup, 1)
        XCTAssertFalse(report.hasProblems, "可重试的清理不该让面板标红")
    }

    /// CRED-5：核验也把"待重新关联"和"待迁移"分开计数。
    ///
    /// 混在一起会让用户看到"还有 7 条在钥匙串"，其实只有一部分需要迁移，
    /// 另一部分是借用授权（只该重新关联）。
    func testVerifySeparatesNeedsRelinkFromPendingMigration() throws {
        let report = try XCTUnwrap(CredentialVerifyReport.parse([
            "ok": 3,
            "broken": 0,
            "pendingKeychain": 1,
            "needsRelink": 4,
            "orphans": [],
            "brokenFiles": 0,
        ]))
        XCTAssertEqual(report.needsRelink, 4)
        XCTAssertEqual(report.pendingKeychain, 1)
        XCTAssertFalse(report.hasProblems, "待关联不是故障")
    }

    /// CRED-5 复核 P0-1：旧键搬运失败要单独出来，并带一个显式重试的入口。
    ///
    /// 这个数字为 0 时面板不显示任何按钮 —— 自动流程永远不碰 `failed`。
    func testKeyMigrationFailureIsParsedSeparately() throws {
        let state = try XCTUnwrap(CredentialMigrationState.parse([
            "summary": [
                "migratable": 0,
                "alreadyJson": 0,
                "magpie": 0,
                "needsRelink": 0,
                "blocked": 0,
                "rollbackable": 0,
                "total": 0,
            ],
            "keyMigration": ["failed": 2, "pending": 1],
            "items": [],
        ]))
        XCTAssertEqual(state.keyMigration.failed, 2)
        XCTAssertEqual(state.keyMigration.pending, 1)
        XCTAssertTrue(state.keyMigration.needsAttention)

        // 老助手不返回这个块：按 0 处理，不显示按钮。
        let legacy = try XCTUnwrap(CredentialMigrationState.parse([
            "summary": [
                "migratable": 0,
                "alreadyJson": 0,
                "magpie": 0,
                "blocked": 0,
                "rollbackable": 0,
                "total": 0,
            ],
            "items": [],
        ]))
        XCTAssertEqual(legacy.keyMigration.failed, 0)
        XCTAssertFalse(legacy.keyMigration.needsAttention)
    }

    /// 批量重新关联的结果要列出**全部**失败原因，而不只是第一条。
    func testReasonSummaryCountsEveryFailedRow() {
        let items: [[String: Any]] = [
            ["accountId": "a", "ok": true],
            ["accountId": "b", "ok": false, "reason": "source_missing"],
            ["accountId": "c", "ok": false, "reason": "write_failed"],
            ["accountId": "d", "ok": false, "reason": "source_missing"],
        ]
        let text = CredentialRelinkSummary.reasonSummary(items)
        XCTAssertTrue(text.contains("source_missing ×2"), text)
        XCTAssertTrue(text.contains("write_failed ×1"), text)
        XCTAssertTrue(CredentialRelinkSummary.reasonSummary([]).isEmpty, "全成功时不显示原因")
    }
}
