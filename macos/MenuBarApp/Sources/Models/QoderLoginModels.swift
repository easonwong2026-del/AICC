import Foundation

/// `qoder.auth.start` 的返回值：Qoder **设备授权**。
///
/// 和 WorkBuddy 的扫码登录是两套独立流程：Qoder 没有回调地址，登录页在
/// `qoder.cn` / `qoder.com` 上，凭据是 device token（`qoder_auth.rs`）。
/// 这里只解析，不含任何令牌。
struct QoderLoginStart: Equatable {
    let edition: QoderEdition
    let loginId: String
    let verificationUri: String
    let expiresIn: Int

    static func parse(_ json: [String: Any]) -> QoderLoginStart? {
        guard let loginId = json["loginId"] as? String, !loginId.isEmpty else { return nil }
        let uri = (json["verificationUri"] as? String) ?? (json["qrPayload"] as? String) ?? ""
        guard !uri.isEmpty else { return nil }
        // edition 必须显式给：不能把一次国际版授权当成本地 CN 账号。
        guard let raw = json["edition"] as? String,
              let edition = QoderEdition(rawValue: raw) else { return nil }
        let expires = (json["expiresIn"] as? NSNumber)?.intValue ?? 900
        return QoderLoginStart(
            edition: edition,
            loginId: loginId,
            verificationUri: uri,
            expiresIn: expires
        )
    }

    /// 可以交给系统浏览器 / 编码成二维码的地址。
    var trustedURL: URL? { QoderLoginStart.trusted(verificationUri, for: edition) }

    /// 只接受**本版本**站点下的 https 登录页。
    ///
    /// 与 WorkBuddy 同样的纪律：反斜杠在浏览器里等价于 `/`，userinfo 是常见伪装，
    /// 两者都直接拒绝；主机再用 URLComponents 归一后按标签边界比对。
    static func trusted(_ raw: String, for edition: QoderEdition) -> URL? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.contains("\\") else { return nil }
        guard !trimmed.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7F })
        else { return nil }
        guard let components = URLComponents(string: trimmed),
              components.scheme?.lowercased() == "https",
              components.user == nil,
              components.password == nil,
              let host = components.host?.lowercased(),
              !host.isEmpty
        else { return nil }
        let root = edition.loginDomain
        guard host == root || host.hasSuffix("." + root) else { return nil }
        return components.url
    }
}

extension QoderEdition {
    /// 登录页所在域。CN 与 International 是**不同域名**，不能互相打开。
    var loginDomain: String {
        switch self {
        case .cn: return "qoder.cn"
        case .global: return "qoder.com"
        }
    }
}

/// 轮询结果。失败要区分类型，界面提示不一样。
enum QoderLoginPhase: Equatable {
    case pending
    case success(name: String)
    case cancelled
    case expired
    case finished
    case failed(kind: String, message: String)

    var isTerminal: Bool {
        if case .pending = self { return false }
        return true
    }

    static func parse(_ json: [String: Any]) -> QoderLoginPhase {
        let state = json["state"] as? String ?? "failed"
        switch state {
        case "pending":
            return .pending
        case "success":
            let nickname = (json["nickname"] as? String)?.trimmingCharacters(in: .whitespaces)
            let email = (json["email"] as? String)?.trimmingCharacters(in: .whitespaces)
            return .success(name: (nickname?.isEmpty == false ? nickname : email) ?? "已授权")
        case "cancelled":
            return .cancelled
        case "expired":
            return .expired
        case "finished":
            return .finished
        default:
            let error = json["error"] as? [String: Any]
            return .failed(
                kind: error?["kind"] as? String ?? "failed",
                message: error?["message"] as? String ?? "授权失败"
            )
        }
    }
}
