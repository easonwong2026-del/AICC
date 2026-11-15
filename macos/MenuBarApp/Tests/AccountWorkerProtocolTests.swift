import XCTest
@testable import AICCCore

/// 覆盖 App 与 Rust 助手之间的编解码：这段逻辑出错会表现成"助手没反应"，
/// 所以必须能在没有助手进程的情况下单测。
final class AccountWorkerProtocolTests: XCTestCase {

    func testRequestIsSingleLineJSONL() throws {
        let data = try AccountWorkerJSON.encodeRequest(id: 7, method: "health")
        XCTAssertEqual(data.last, 0x0A, "必须以换行结尾，助手按行读")
        let text = String(decoding: data.dropLast(), as: UTF8.self)
        XCTAssertFalse(text.contains("\n"), "一行只能有一条消息")

        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(text.utf8)))
        XCTAssertEqual(object["jsonrpc"] as? String, "2.0")
        XCTAssertEqual(object["method"] as? String, "health")
        XCTAssertEqual(AccountWorkerJSON.responseID(object), 7)
    }

    func testTakeLineSplitsAndKeepsRemainder() {
        var buffer = Data("{\"id\":1}\n{\"id\":2}\n{\"id\":3".utf8)

        let first = AccountWorkerJSON.takeLine(from: &buffer)
        XCTAssertEqual(String(decoding: try! XCTUnwrap(first), as: UTF8.self), "{\"id\":1}")

        let second = AccountWorkerJSON.takeLine(from: &buffer)
        XCTAssertEqual(String(decoding: try! XCTUnwrap(second), as: UTF8.self), "{\"id\":2}")

        XCTAssertNil(AccountWorkerJSON.takeLine(from: &buffer), "半行要留到下次")
        XCTAssertEqual(String(decoding: buffer, as: UTF8.self), "{\"id\":3")
    }

    func testTakeLineHandlesEmptyBuffer() {
        var buffer = Data()
        XCTAssertNil(AccountWorkerJSON.takeLine(from: &buffer))
    }

    func testNotificationHasNoResponseID() {
        let object = AccountWorkerJSON.decodeObject(Data("{\"jsonrpc\":\"2.0\",\"method\":\"log\"}".utf8))
        XCTAssertNil(AccountWorkerJSON.responseID(try! XCTUnwrap(object)))
    }

    func testRPCErrorKeepsKind() throws {
        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(
            "{\"id\":3,\"error\":{\"code\":1002,\"message\":\"钥匙串授权被拒绝\",\"data\":{\"kind\":\"denied\"}}}".utf8
        )))
        let error = try XCTUnwrap(AccountWorkerJSON.rpcError(from: object))
        XCTAssertEqual(error.code, 1002)
        XCTAssertEqual(error.kind, "denied")
        XCTAssertEqual(error.message, "钥匙串授权被拒绝")
    }

    func testHealthParsesRealisticPayload() throws {
        let json = """
        {
          "status": "ok",
          "protocol": 1,
          "worker": {"version": "0.1.0", "aiccVersion": "2.12.2", "pid": 991, "uptimeMs": 12},
          "upstream": {"crate": "wb-switch-core", "version": "0.1.62", "commit": "22b318c1"},
          "paths": {"dataDir": "/tmp/x", "dbPath": "/tmp/x/accounts.sqlite", "wbSwitchHomeOwned": true},
          "database": {"ok": true, "schemaVersion": 1, "counts": {"accounts": 3}},
          "secureStore": {"backend": "keychain"},
          "providers": {
            "workbuddy": {"state": "encrypted_envelope"},
            "qoder": {"state": "auth_file_present"}
          }
        }
        """
        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(json.utf8)))
        let health = try XCTUnwrap(AccountWorkerHealth.parse(object))

        XCTAssertEqual(health.protocolVersion, 1)
        XCTAssertEqual(health.workerVersion, "0.1.0")
        XCTAssertEqual(health.upstreamVersion, "0.1.62")
        XCTAssertEqual(health.accountCount, 3)
        XCTAssertEqual(health.workbuddyState, "encrypted_envelope")
        XCTAssertEqual(health.qoderState, "auth_file_present")
        XCTAssertTrue(health.isProtocolCompatible)
        XCTAssertTrue(health.isHealthy)
        XCTAssertTrue(health.wbSwitchHomeOwned)
        XCTAssertTrue(health.summary.contains("账号 3"), "摘要要能直接放进设置页：\(health.summary)")
    }

    /// 凭据存储现状来自两个字段：`credentials`（开关与告知）和 `authorizations.storage`（条数）。
    func testHealthParsesCredentialStoragePolicy() throws {
        let json = """
        {
          "status": "ok",
          "protocol": 1,
          "worker": {"version": "0.1.0"},
          "upstream": {"version": "0.1.62", "commit": "x"},
          "paths": {"dataDir": "/tmp"},
          "database": {"ok": true, "counts": {"accounts": 2}},
          "providers": {"workbuddy": {"state": "missing"}, "qoder": {"state": "missing"}},
          "credentials": {
            "jsonStorageEnabled": false,
            "consentRequired": true,
            "settingKey": "credentials_json_consent",
            "risk": "本地 JSON 是明文存储"
          },
          "authorizations": {
            "storage": [
              {"kind": "magpie", "count": 4},
              {"kind": "keychain", "count": 2},
              {"kind": "localjson", "count": 0}
            ]
          }
        }
        """
        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(json.utf8)))
        let health = try XCTUnwrap(AccountWorkerHealth.parse(object))
        let policy = try XCTUnwrap(health.credentialStorage)

        XCTAssertFalse(policy.jsonEnabled)
        XCTAssertTrue(policy.consentRequired)
        XCTAssertEqual(policy.risk, "本地 JSON 是明文存储")
        XCTAssertEqual(policy.magpieCount, 4)
        XCTAssertEqual(policy.keychainCount, 2)
        XCTAssertEqual(policy.localJSONCount, 0, "count 为 0 的项不该出现在摘要里")
        XCTAssertEqual(policy.headline, "macOS 钥匙串")
        XCTAssertEqual(policy.summary, "钥匙串 2 · Magpie 4")
    }

    func testCredentialStorageHeadlineFollowsTheSwitch() {
        let enabled = CredentialStoragePolicy(
            jsonEnabled: true,
            consentRequired: false,
            risk: "",
            counts: ["localjson": 3, "magpie": 1]
        )
        XCTAssertEqual(enabled.headline, "本地 JSON（明文）")
        XCTAssertEqual(enabled.summary, "本地 JSON 3 · Magpie 1")

        let empty = CredentialStoragePolicy(jsonEnabled: false, consentRequired: true, risk: "", counts: [:])
        XCTAssertEqual(empty.summary, "暂无授权", "没有任何授权时不能显示成空白")
    }

    /// 老助手没有这两个字段：界面必须能区分"未提供"和"未启用"。
    func testHealthWithoutCredentialStorageReportsNil() throws {
        let json = """
        {"status":"ok","protocol":1,
         "worker":{"version":"0.1.0"},"upstream":{"version":"0.1.62","commit":"x"},
         "paths":{"dataDir":"/tmp"},"database":{"ok":true,"counts":{"accounts":0}},
         "providers":{"workbuddy":{"state":"missing"},"qoder":{"state":"missing"}}}
        """
        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(json.utf8)))
        let health = try XCTUnwrap(AccountWorkerHealth.parse(object))
        XCTAssertNil(health.credentialStorage)
    }

    func testHealthRejectsIncompatibleProtocol() throws {
        let json = """
        {"status":"ok","protocol":99,
         "worker":{"version":"9"},"upstream":{"version":"9","commit":"x"},
         "paths":{"dataDir":"/tmp"},"database":{"ok":true,"counts":{"accounts":0}},
         "providers":{"workbuddy":{"state":"missing"},"qoder":{"state":"missing_auth_dir"}}}
        """
        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(json.utf8)))
        let health = try XCTUnwrap(AccountWorkerHealth.parse(object))
        XCTAssertFalse(health.isProtocolCompatible)
        XCTAssertTrue(health.summary.contains("协议不匹配"))
    }

    func testHealthDegradedWhenDatabaseFails() throws {
        let json = """
        {"status":"degraded","protocol":1,
         "worker":{"version":"0.1.0"},"upstream":{"version":"0.1.62","commit":"x"},
         "paths":{"dataDir":"/tmp"},"database":{"ok":false,"error":"磁盘只读"},
         "providers":{"workbuddy":{"state":"missing"},"qoder":{"state":"missing"}}}
        """
        let object = try XCTUnwrap(AccountWorkerJSON.decodeObject(Data(json.utf8)))
        let health = try XCTUnwrap(AccountWorkerHealth.parse(object))
        XCTAssertFalse(health.isHealthy)
        XCTAssertTrue(health.summary.contains("数据库异常"), health.summary)
    }

    func testHealthRejectsMissingFields() {
        XCTAssertNil(AccountWorkerHealth.parse(["status": "ok"]))
    }

    func testLocalizedStatesAreStable() {
        // 这些字符串是 Rust 侧的状态名，改动会被这里挡住。
        XCTAssertEqual(AccountWorkerHealth.localized(state: "missing"), "未登录")
        XCTAssertEqual(AccountWorkerHealth.localized(state: "encrypted_envelope"), "加密信封（待接入）")
        XCTAssertEqual(AccountWorkerHealth.localized(state: "ambiguous_auth_dir"), "多配置待指定")
    }
}
