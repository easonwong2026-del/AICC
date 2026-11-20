import Foundation

/// AICC 与内嵌 Rust 助手 `aicc-account-worker` 之间的协议。
///
/// 传输形式：stdin/stdout 上一行一条 JSON-RPC 2.0 消息（JSONL）。
/// 这个文件只放**可单测**的纯逻辑：没有进程、没有 AppKit，
/// 所以 `swift test` 能覆盖编解码和健康检查解析。
public enum AccountWorkerProtocol {
    /// 必须与 `account-worker/src/version.rs` 的 `PROTOCOL_VERSION` 一致。
    public static let version = 1

    public static let methodPing = "ping"
    public static let methodHealth = "health"
    public static let methodStorageInit = "storage.init"
    public static let methodSecureStatus = "secure.status"
    public static let methodShutdown = "worker.shutdown"
    /// 全局设置读写（PR #66）：目前只有首选授权来源。
    public static let methodSettingsGet = "settings.get"
    public static let methodSettingsSet = "settings.set"
    /// 单份授权删除（类型化接口，界面暂不提供按钮）。
    public static let methodDeleteAuthorization = "auth.authorization.delete"

    /// `app_settings` 里的键名，与 Rust 侧 `auth::broker::PREFERRED_SETTING` 一致。
    public static let settingPreferredAuthSource = "auth_preferred_source"

    /// 明文 JSON 的开关，与 Rust 侧 `auth::consent::JSON_CONSENT_SETTING` 一致。
    /// 只有用户在界面上明确同意之后才置 `"1"`；未置位时新登录继续走钥匙串。
    public static let settingCredentialsJSONConsent = "credentials_json_consent"

    /// 助手在 bundle 里的相对路径。
    public static let helperRelativePath = "Contents/Helpers/aicc-account-worker"
}

/// JSON-RPC 错误：`kind` 是助手给的分类，用来区分"没登录"和"钥匙串被拒"。
public struct AccountWorkerRPCError: Equatable, Error, LocalizedError {
    public let code: Int
    public let message: String
    public let kind: String?
    /// 失败时后端**已经做了什么**。目前只有账号切换会带：`true` = 已恢复到切换前的登录态，
    /// `false` = 恢复也失败了（必须让用户手动处理），`nil` = 这件事不涉及恢复。
    public let restored: Bool?

    public init(code: Int, message: String, kind: String? = nil, restored: Bool? = nil) {
        self.code = code
        self.message = message
        self.kind = kind
        self.restored = restored
    }

    public var errorDescription: String? { message }
}

public enum AccountWorkerFailure: Error, Equatable, LocalizedError {
    case malformedLine
    case notRunning
    case timeout
    case transport(String)
    case remote(AccountWorkerRPCError)

    public var errorDescription: String? {
        switch self {
        case .malformedLine: return "助手返回了无法解析的消息"
        case .notRunning: return "助手进程未运行"
        case .timeout: return "助手响应超时"
        case .transport(let detail): return "助手通信失败：\(detail)"
        case .remote(let err): return err.message
        }
    }
}

/// JSONL 编解码。助手只写一行一条，所以按 `\n` 切分，剩余部分留到下次。
public enum AccountWorkerJSON {
    public static func encodeRequest(id: Int, method: String, params: [String: Any] = [:]) throws -> Data {
        let object: [String: Any] = [
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        ]
        var data = try JSONSerialization.data(withJSONObject: object, options: [])
        data.append(0x0A)
        return data
    }

    /// 从缓冲区里取出一条完整消息，并把该行从缓冲区移除。
    public static func takeLine(from buffer: inout Data) -> Data? {
        guard let index = buffer.firstIndex(of: 0x0A) else { return nil }
        let line = buffer.subdata(in: buffer.startIndex..<index)
        buffer.removeSubrange(buffer.startIndex...index)
        return line
    }

    public static func decodeObject(_ data: Data) -> [String: Any]? {
        guard !data.isEmpty else { return nil }
        return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
    }

    /// 响应是否属于某次请求：通知（无 id）永远不匹配。
    public static func responseID(_ object: [String: Any]) -> Int? {
        if let number = object["id"] as? Int { return number }
        if let number = object["id"] as? NSNumber { return number.intValue }
        return nil
    }

    public static func rpcError(from object: [String: Any]) -> AccountWorkerRPCError? {
        guard let error = object["error"] as? [String: Any] else { return nil }
        let code = (error["code"] as? NSNumber)?.intValue ?? 0
        let message = error["message"] as? String ?? "未知错误"
        let data = error["data"] as? [String: Any]
        let kind = data?["kind"] as? String
        // `restored` 只对账号切换有意义；其他错误不带这个字段，保持 nil。
        let restored = data?["restored"] as? Bool
        return AccountWorkerRPCError(
            code: code,
            message: message,
            kind: kind,
            restored: restored
        )
    }
}

/// `health` 的结构化视图。
/// 凭据存储策略与现状。
///
/// 两个来源合成：`health.credentials`（是否已同意明文 JSON、风险说明）与
/// `health.authorizations.storage`（`localjson` / `magpie` / `keychain` 各几条）。
/// 助手没提供时返回 nil —— 老版本助手根本没有 JSON 存储，界面应当说"未知"而不是"未启用"。
public struct CredentialStoragePolicy: Equatable {
    /// 用户是否已同意把**新建**授权写成明文 JSON。
    public let jsonEnabled: Bool
    /// 尚未同意：界面需要先解释风险再让用户选择。
    public let consentRequired: Bool
    /// Rust 侧给出的风险说明（中文，可直接展示）。
    public let risk: String
    /// 引用种类 → 条数。只包含助手实际返回的项。
    public let counts: [String: Int]

    public static let kindJSON = "localjson"
    public static let kindMagpie = "magpie"
    public static let kindKeychain = "keychain"

    public init(jsonEnabled: Bool, consentRequired: Bool, risk: String, counts: [String: Int]) {
        self.jsonEnabled = jsonEnabled
        self.consentRequired = consentRequired
        self.risk = risk
        self.counts = counts
    }

    public var localJSONCount: Int { counts[Self.kindJSON] ?? 0 }
    public var magpieCount: Int { counts[Self.kindMagpie] ?? 0 }
    public var keychainCount: Int { counts[Self.kindKeychain] ?? 0 }

    /// 当前方式的一句话标题。
    public var headline: String {
        jsonEnabled ? "本地 JSON（明文）" : "macOS 钥匙串"
    }

    /// 各来源各几条，设置页直接显示。
    public var summary: String {
        var parts: [String] = []
        if localJSONCount > 0 { parts.append("本地 JSON \(localJSONCount)") }
        if keychainCount > 0 { parts.append("钥匙串 \(keychainCount)") }
        if magpieCount > 0 { parts.append("Magpie \(magpieCount)") }
        return parts.isEmpty ? "暂无授权" : parts.joined(separator: " · ")
    }

    /// 解析 `health` 里的两个字段。缺 `credentials` 说明是老助手，返回 nil。
    public static func parse(_ object: [String: Any]) -> CredentialStoragePolicy? {
        guard let credentials = object["credentials"] as? [String: Any],
              let enabled = credentials["jsonStorageEnabled"] as? Bool,
              let required = credentials["consentRequired"] as? Bool
        else { return nil }

        var counts: [String: Int] = [:]
        if let authorizations = object["authorizations"] as? [String: Any],
           let storage = authorizations["storage"] as? [[String: Any]] {
            for row in storage {
                guard let kind = row["kind"] as? String,
                      let count = (row["count"] as? NSNumber)?.intValue,
                      count > 0
                else { continue }
                counts[kind, default: 0] += count
            }
        }
        return CredentialStoragePolicy(
            jsonEnabled: enabled,
            consentRequired: required,
            risk: credentials["risk"] as? String ?? "",
            counts: counts
        )
    }
}

public struct AccountWorkerHealth: Equatable {
    public let status: String
    public let protocolVersion: Int
    public let workerVersion: String
    public let aiccVersion: String
    public let upstreamVersion: String
    public let upstreamCommit: String
    public let dataDirectory: String
    public let databaseOK: Bool
    public let schemaVersion: Int
    public let accountCount: Int
    public let workbuddyState: String
    public let qoderState: String
    public let wbSwitchHomeOwned: Bool
    /// 凭据存储策略；老助手不返回，此时为 nil。
    public let credentialStorage: CredentialStoragePolicy?

    /// 协议对不上的助手必须拒绝使用，否则会出现"半个新协议"。
    public var isProtocolCompatible: Bool {
        protocolVersion == AccountWorkerProtocol.version
    }

    public var isHealthy: Bool { status == "ok" && databaseOK }

    /// 一行摘要，给设置页直接显示。
    public var summary: String {
        guard isProtocolCompatible else {
            return "协议不匹配（助手 \(protocolVersion)，App \(AccountWorkerProtocol.version)）"
        }
        var parts = ["v\(workerVersion)", databaseOK ? "数据库正常" : "数据库异常"]
        parts.append("账号 \(accountCount)")
        parts.append("WorkBuddy \(Self.localized(state: workbuddyState))")
        parts.append("Qoder \(Self.localized(state: qoderState))")
        return parts.joined(separator: " · ")
    }

    /// 平台状态的中文说明。状态名与 Rust 侧一一对应，不做猜测。
    static func localized(state: String) -> String {
        switch state {
        case "missing": return "未登录"
        case "plaintext_token": return "已登录"
        case "encrypted_envelope": return "加密信封（待接入）"
        case "unrecognized": return "格式未识别"
        case "unreadable": return "不可读"
        case "auth_file_present": return "已安装"
        case "missing_auth_file": return "未登录"
        case "missing_auth_dir": return "未安装"
        case "ambiguous_auth_dir": return "多配置待指定"
        default: return state
        }
    }

    public static func parse(_ object: [String: Any]) -> AccountWorkerHealth? {
        guard let worker = object["worker"] as? [String: Any],
              let upstream = object["upstream"] as? [String: Any],
              let paths = object["paths"] as? [String: Any],
              let database = object["database"] as? [String: Any],
              let providers = object["providers"] as? [String: Any],
              let protocolVersion = (object["protocol"] as? NSNumber)?.intValue,
              let status = object["status"] as? String
        else { return nil }

        let counts = database["counts"] as? [String: Any]
        let accountCount = (counts?["accounts"] as? NSNumber)?.intValue ?? 0

        return AccountWorkerHealth(
            status: status,
            protocolVersion: protocolVersion,
            workerVersion: worker["version"] as? String ?? "?",
            aiccVersion: worker["aiccVersion"] as? String ?? "unknown",
            upstreamVersion: upstream["version"] as? String ?? "?",
            upstreamCommit: upstream["commit"] as? String ?? "?",
            dataDirectory: paths["dataDir"] as? String ?? "",
            databaseOK: database["ok"] as? Bool ?? false,
            schemaVersion: (database["schemaVersion"] as? NSNumber)?.intValue ?? 0,
            accountCount: accountCount,
            workbuddyState: (providers["workbuddy"] as? [String: Any])?["state"] as? String ?? "unknown",
            qoderState: (providers["qoder"] as? [String: Any])?["state"] as? String ?? "unknown",
            wbSwitchHomeOwned: paths["wbSwitchHomeOwned"] as? Bool ?? false,
            credentialStorage: CredentialStoragePolicy.parse(object)
        )
    }
}
