import XCTest
@testable import AICCCore

/// PR #73：Qoder 设备授权的模型解析与登录地址校验。
///
/// 这些用例只覆盖**本地**判定：真实换票要真机，不能拿模拟响应当"已验证"。
final class QoderLoginModelsTests: XCTestCase {
    func testStartParsesEditionAndExpiry() throws {
        let start = try XCTUnwrap(QoderLoginStart.parse([
            "edition": "qoder_cn",
            "loginId": "qd1_abc",
            "verificationUri": "https://qoder.cn/device/selectAccounts?challenge=c&challenge_method=S256",
            "expiresIn": 900,
        ]))
        XCTAssertEqual(start.edition, .cn)
        XCTAssertEqual(start.loginId, "qd1_abc")
        XCTAssertEqual(start.expiresIn, 900)
        XCTAssertNotNil(start.trustedURL)
    }

    func testStartRejectsIncompleteOrAmbiguousPayload() {
        // 没有 edition：不能猜成 CN 或 International。
        XCTAssertNil(QoderLoginStart.parse([
            "loginId": "qd1_abc",
            "verificationUri": "https://qoder.cn/device/selectAccounts",
        ]))
        // 含糊的 `qoder` 不是合法版本。
        XCTAssertNil(QoderLoginStart.parse([
            "edition": "qoder",
            "loginId": "qd1_abc",
            "verificationUri": "https://qoder.cn/device/selectAccounts",
        ]))
        XCTAssertNil(QoderLoginStart.parse([
            "edition": "qoder_global",
            "loginId": "",
            "verificationUri": "https://qoder.com/device/selectAccounts",
        ]))
        XCTAssertNil(QoderLoginStart.parse([
            "edition": "qoder_global",
            "loginId": "qd1_abc",
            "verificationUri": "",
        ]))
    }

    func testPhaseMapping() {
        XCTAssertEqual(QoderLoginPhase.parse(["state": "pending"]), .pending)
        XCTAssertEqual(
            QoderLoginPhase.parse(["state": "success", "nickname": "阿翊"]),
            .success(name: "阿翊")
        )
        // 没有昵称就用邮箱，都没有也给出可读文案。
        XCTAssertEqual(
            QoderLoginPhase.parse(["state": "success", "email": "a@b.c"]),
            .success(name: "a@b.c")
        )
        XCTAssertEqual(QoderLoginPhase.parse(["state": "success"]), .success(name: "已授权"))
        XCTAssertEqual(QoderLoginPhase.parse(["state": "cancelled"]), .cancelled)
        XCTAssertEqual(QoderLoginPhase.parse(["state": "expired"]), .expired)
        XCTAssertEqual(QoderLoginPhase.parse(["state": "finished"]), .finished)
        XCTAssertEqual(
            QoderLoginPhase.parse(["state": "failed", "error": ["kind": "network", "message": "断网"]]),
            .failed(kind: "network", message: "断网")
        )
        XCTAssertFalse(QoderLoginPhase.pending.isTerminal)
        XCTAssertTrue(QoderLoginPhase.expired.isTerminal)
    }

    /// 登录页必须是**本版本**站点下的 https 页面。
    func testTrustedLoginURLIsEditionScoped() {
        // 各自站点放行（含子域与端口）。
        XCTAssertNotNil(QoderLoginStart.trusted(
            "https://qoder.cn/device/selectAccounts?challenge=c",
            for: .cn
        ))
        XCTAssertNotNil(QoderLoginStart.trusted(
            "https://qoder.com/device/selectAccounts?challenge=c",
            for: .global
        ))
        XCTAssertNotNil(QoderLoginStart.trusted(
            "https://www.qoder.cn:8443/device/selectAccounts",
            for: .cn
        ))

        // 跨版本一律拒绝：CN 的会话不能打开国际版登录页，反之亦然。
        XCTAssertNil(QoderLoginStart.trusted("https://qoder.com/device/selectAccounts", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("https://qoder.cn/device/selectAccounts", for: .global))

        // 非 https 与危险 scheme。
        XCTAssertNil(QoderLoginStart.trusted("http://qoder.cn/device/selectAccounts", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("javascript:alert(1)", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("file:///etc/passwd", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("qoder.cn/device/selectAccounts", for: .cn))

        // 域名仿冒、userinfo 伪装、反斜杠（浏览器会当 `/`）。
        XCTAssertNil(QoderLoginStart.trusted("https://qoder.cn.evil.com/device", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("https://evil.com/?qoder.cn", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("https://qoder.cn@evil.com/device", for: .cn))
        XCTAssertNil(QoderLoginStart.trusted("https://evil.com\\@qoder.com/device", for: .global))
        XCTAssertNil(QoderLoginStart.trusted("https://qoder.com\\@evil.com/device", for: .global))
    }

    /// 列表里 CN 与 International 的登录域名必须不同（同一份代码两个站）。
    func testEditionsHaveDistinctLoginDomains() {
        XCTAssertEqual(QoderEdition.cn.loginDomain, "qoder.cn")
        XCTAssertEqual(QoderEdition.global.loginDomain, "qoder.com")
        XCTAssertNotEqual(QoderEdition.cn.loginDomain, QoderEdition.global.loginDomain)
    }
}
