import XCTest
@testable import AICCCore

/// Qoder 平台服务账号模型。
///
/// Rust 侧验证"写进了哪张表、有没有复制令牌"；这里只验界面拿到的字段是不是
/// 从真实数据来、以及"服务账号 ≠ 本机快照"这条区分有没有被模型破坏。
final class QoderServiceAccountModelsTests: XCTestCase {

    private func row(_ overrides: [String: Any] = [:]) -> [String: Any] {
        var base: [String: Any] = [
            "id": "qa_0123456789ab",
            "edition": "qoder_global",
            "uid": "01a1afdc-c495-78ad-8a5c-9123e17e8708",
            "nickname": "海外号",
            "authorizations": [
                [
                    "source": "magpie",
                    "sourceKey": "qoder#a1b2c3",
                    "authStatus": "valid",
                    "hasSecret": false,
                ],
            ],
        ]
        for (key, value) in overrides { base[key] = value }
        return base
    }

    func testParsesGlobalAccount() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row()))
        XCTAssertEqual(account.edition, "qoder_global")
        XCTAssertEqual(account.editionName, "Qoder International")
        XCTAssertEqual(account.displayName, "海外号")
        XCTAssertEqual(account.authorizations.count, 1)
        // 默认 fixture 只有 Magpie 关联：不能因此显示"有效"。
        XCTAssertEqual(account.statusLabel, "已关联（未校验）")
    }

    func testCnAndGlobalShareNoLabels() throws {
        let cn = try XCTUnwrap(QoderServiceAccount(json: row(["edition": "qoder_cn"])))
        XCTAssertEqual(cn.editionName, "Qoder CN")
        XCTAssertNotEqual(cn.editionName, "Qoder International")
    }

    /// Magpie 只是"关联"，不是"认证有效"：AICC 手里没有可验证的凭据。
    func testMagpieLinkAloneIsNotClaimedAsValid() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row()))
        XCTAssertTrue(account.usableSources.isEmpty, "magpie 关联不能被当成可用凭据")
        XCTAssertEqual(account.linkedSources.count, 1)
        XCTAssertEqual(account.statusLabel, "已关联（未校验）")
        XCTAssertEqual(account.statusTone, .muted)
        XCTAssertEqual(account.sourceLabels, ["Magpie"])
    }

    /// 自有授权必须有凭据才算可用：界面不能把"登录过但没存下令牌"显示成有效。
    func testOwnBrowserSourceNeedsASecret() throws {
        let withoutSecret = try XCTUnwrap(QoderServiceAccount(json: row([
            "authorizations": [[
                "source": "own_browser",
                "authStatus": "valid",
                "hasSecret": false,
            ]],
        ])))
        XCTAssertTrue(withoutSecret.usableSources.isEmpty)
        XCTAssertEqual(withoutSecret.statusTone, .warn)

        let withSecret = try XCTUnwrap(QoderServiceAccount(json: row([
            "authorizations": [[
                "source": "own_browser",
                "authStatus": "valid",
                "hasSecret": true,
            ]],
        ])))
        XCTAssertEqual(withSecret.usableSources.count, 1)
        XCTAssertEqual(withSecret.sourceLabels, ["浏览器登录"])
    }

    /// 两个来源并存时，自有授权可用才算可用（Magpie 过期、自有授权仍有效）。
    func testOneUsableSourceIsEnough() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row([
            "authorizations": [
                ["source": "magpie", "sourceKey": "qoder", "authStatus": "expired", "hasSecret": false],
                ["source": "own_browser", "authStatus": "valid", "hasSecret": true],
            ],
        ])))
        XCTAssertEqual(account.usableSources.count, 1)
        XCTAssertEqual(account.statusLabel, "有效")
        XCTAssertEqual(account.sourceLabels, ["Magpie", "浏览器登录"])
    }

    /// 只有 Magpie 关联 + 自有授权过期：仍然不能说"有效"。
    func testMagpieLinkDoesNotMaskAnExpiredOwnAuthorization() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row([
            "authorizations": [
                ["source": "magpie", "sourceKey": "qoder", "authStatus": "valid", "hasSecret": false],
                ["source": "own_browser", "authStatus": "expired", "hasSecret": true],
            ],
        ])))
        XCTAssertTrue(account.usableSources.isEmpty)
        XCTAssertEqual(account.statusLabel, "已过期")
        XCTAssertEqual(account.statusTone, .warn)
    }

    func testExpiredOnlyIsWarnNotOk() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row([
            "authorizations": [
                ["source": "magpie", "sourceKey": "qoder", "authStatus": "expired", "hasSecret": false],
            ],
        ])))
        XCTAssertEqual(account.statusLabel, "已过期")
        XCTAssertEqual(account.statusTone, .warn)
    }

    func testMissingAuthorizationsIsNotMistakenForValid() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row(["authorizations": []])))
        XCTAssertEqual(account.statusLabel, "无授权")
        XCTAssertEqual(account.statusTone, .muted)
    }

    func testUidIsMasked() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row()))
        XCTAssertEqual(account.maskedUid, "01a1afdc…")
        XCTAssertFalse(account.maskedUid.contains("9123e17e8708"))
    }

    func testFallsBackToEmailThenUid() throws {
        let byEmail = try XCTUnwrap(QoderServiceAccount(json: row([
            "nickname": "",
            "email": "a@example.com",
        ])))
        XCTAssertEqual(byEmail.displayName, "a@example.com")

        let byUid = try XCTUnwrap(QoderServiceAccount(json: row([
            "nickname": "",
            "uid": "abcdef1234567890",
        ])))
        XCTAssertEqual(byUid.displayName, "abcdef12…")
    }

    func testRowWithoutUidIsRejected() {
        XCTAssertNil(QoderServiceAccount(json: row(["uid": ""])))
        XCTAssertNil(QoderServiceAccount(json: row(["id": NSNull()])))
    }

    /// 模型里不允许出现令牌字段——它是直接进界面的。
    func testUnknownFieldsCannotSmuggleAToken() throws {
        let account = try XCTUnwrap(QoderServiceAccount(json: row([
            "accessToken": "secret-value",
            "authorizations": [[
                "source": "magpie",
                "sourceKey": "qoder",
                "authStatus": "valid",
                "accessToken": "secret-value",
            ]],
        ])))
        let dump = String(describing: account)
        XCTAssertFalse(dump.contains("secret-value"))
    }
}
