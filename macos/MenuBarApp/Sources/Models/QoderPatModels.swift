import Foundation

/// `qoder.auth.pat.save` 的返回值。
///
/// **两个结论必须分开**：`identity` 说的是"这张 PAT 换到了账号身份"，
/// `credits` 说的是"它能不能领 Credits"。前者通过不代表后者通过，
/// 所以界面文案永远不能把这里写成"登录已完成"。
struct QoderPatResult: Equatable {
    let accountId: String
    let edition: QoderEdition
    let uid: String
    let nickname: String
    let email: String?

    static func parse(_ json: [String: Any]) -> QoderPatResult? {
        guard json["saved"] as? Bool == true,
              let accountId = json["accountId"] as? String,
              let raw = json["edition"] as? String,
              let edition = QoderEdition(rawValue: raw),
              let uid = json["uid"] as? String
        else { return nil }
        return QoderPatResult(
            accountId: accountId,
            edition: edition,
            uid: uid,
            nickname: (json["nickname"] as? String) ?? "",
            email: (json["email"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        )
    }

    var displayName: String {
        if !nickname.isEmpty { return nickname }
        if let email { return email }
        return uid.count <= 8 ? uid : String(uid.prefix(8)) + "…"
    }
}

/// PAT 保存失败的类型。只有这几种，界面按类型给不同下一步。
///
/// 它是 `Error` 但不是"程序出错"：这几种都是服务端的业务结论，
/// 界面当成可读的结果展示，不做错误弹窗。
enum QoderPatFailure: Error, Equatable {
    case rejected(String)
    case forbidden(String)
    case profileMissing(String)
    case network(String)
    case malformed(String)
    case unknown(String)

    static func parse(_ json: [String: Any]) -> QoderPatFailure {
        let error = json["error"] as? [String: Any]
        let kind = error?["kind"] as? String ?? "unknown"
        let message = error?["message"] as? String ?? "PAT 保存失败"
        switch kind {
        case "rejected": return .rejected(message)
        case "forbidden": return .forbidden(message)
        case "profile_missing": return .profileMissing(message)
        case "network": return .network(message)
        case "malformed": return .malformed(message)
        default: return .unknown(message)
        }
    }

    var message: String {
        switch self {
        case .rejected(let text), .forbidden(let text), .profileMissing(let text),
             .network(let text), .malformed(let text), .unknown(let text):
            return text
        }
    }

    /// 下一步该做什么。空字符串表示没有额外建议。
    var hint: String {
        switch self {
        case .rejected:
            return "请确认 PAT 是在同一个站点生成的、且没有被撤销，然后重新复制一次。"
        case .forbidden:
            return "生成 PAT 时请勾选账号读取权限。"
        case .profileMissing:
            return "服务端没有返回账号身份，这张令牌不能用来登记账号。"
        case .network:
            return "检查网络后重试；令牌不需要重新生成。"
        case .malformed:
            return "注意不要带多余的空格或换行。"
        case .unknown:
            return "可以改用「浏览器授权」作为备用方式。"
        }
    }
}

extension QoderEdition {
    /// 用户生成 PAT 的官方页面。**CN 与 International 是两个不同的站点**。
    var patPageURL: URL? {
        switch self {
        case .cn: return URL(string: "https://qoder.cn/account/integrations")
        case .global: return URL(string: "https://qoder.com/account/integrations")
        }
    }
}
