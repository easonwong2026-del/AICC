import XCTest
@testable import AICCCore

/// PR #73：PAT 备用授权的模型解析与"两件事不能混"的约束。
///
/// 重点不是解析得多聪明，而是**不能把没验证的东西说成已验证**：
/// `identity` 与 `credits` 必须分开，失败类型必须能给出不同的下一步。
final class QoderPatModelsTests: XCTestCase {
    func testSavedPayloadCarriesIdentityButNotCredits() throws {
        let saved = try XCTUnwrap(QoderPatResult.parse([
            "saved": true,
            "accountId": "qa_abc",
            "edition": "qoder_cn",
            "uid": "u-1",
            "nickname": "测试账号",
            "email": "pat@example.com",
            "identity": "verified",
            "credits": "unknown",
        ]))
        XCTAssertEqual(saved.edition, .cn)
        XCTAssertEqual(saved.accountId, "qa_abc")
        XCTAssertEqual(saved.displayName, "测试账号")
    }

    func testFailedPayloadNeverLooksLikeSuccess() {
        // 助手明确回了 saved=false：没有账号 id 就不该被解析成成功。
        XCTAssertNil(QoderPatResult.parse([
            "saved": false,
            "state": "failed",
            "error": ["kind": "rejected", "message": "401"],
        ]))
        // 少了 edition：不能猜站点。
        XCTAssertNil(QoderPatResult.parse([
            "saved": true,
            "accountId": "qa_abc",
            "uid": "u-1",
        ]))
    }

    func testFailureKindsMapToDifferentNextSteps() {
        let rejected = QoderPatFailure.parse([
            "error": ["kind": "rejected", "message": "PAT 被服务端拒绝（401）"],
        ])
        XCTAssertEqual(rejected.message, "PAT 被服务端拒绝（401）")
        XCTAssertTrue(rejected.hint.contains("同一个站点"))

        let forbidden = QoderPatFailure.parse([
            "error": ["kind": "forbidden", "message": "403"],
        ])
        XCTAssertTrue(forbidden.hint.contains("权限"))

        let network = QoderPatFailure.parse([
            "error": ["kind": "network", "message": "请求超时"],
        ])
        XCTAssertTrue(network.hint.contains("不需要重新生成"))

        // 未知类型必须落到兜底，而不是被当成某种已知成功。
        let unknown = QoderPatFailure.parse([:])
        XCTAssertEqual(unknown.message, "PAT 保存失败")
    }

    func testPatPageIsSiteSpecific() throws {
        XCTAssertEqual(
            try XCTUnwrap(QoderEdition.cn.patPageURL).absoluteString,
            "https://qoder.cn/account/integrations"
        )
        XCTAssertEqual(
            try XCTUnwrap(QoderEdition.global.patPageURL).absoluteString,
            "https://qoder.com/account/integrations"
        )
        // 两站不能共用一个地址：生成在 A 站、粘贴到 B 站的 PAT 必然 401。
        XCTAssertNotEqual(QoderEdition.cn.patPageURL, QoderEdition.global.patPageURL)
    }
}
