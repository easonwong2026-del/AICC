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
        XCTAssertTrue(state.summary.needsAttention)

        XCTAssertEqual(state.items.count, 2)
        XCTAssertTrue(state.items[0].migratable)
        XCTAssertEqual(state.items[0].storageText, "钥匙串")
        XCTAssertNil(state.items[0].blockedText)
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
            total: 6
        )
        XCTAssertFalse(summary.needsAttention)
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
}
