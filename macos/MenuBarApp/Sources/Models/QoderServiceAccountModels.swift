import Foundation

/// Qoder 的**平台服务账号**（不是本机客户端快照）。
///
/// 两件事必须分开看：
///
/// | 概念 | 来源 | 需要客户端吗 |
/// | --- | --- | --- |
/// | `QoderProfile` | 本机客户端的认证快照 | 需要 |
/// | `QoderServiceAccount`（本文件） | 平台服务上的账号 + 授权来源 | **不需要** |
///
/// 所以界面上一个账号可能同时有"服务账号"与"本机快照"，但它们不是同一张卡片，
/// 也不能互相顶替：网页授权导入的账号在本机没有客户端也能存在。
///
/// 模型永远不含令牌：授权行只有来源、状态、是否有凭据（`hasSecret`）。
struct QoderServiceAccount: Identifiable, Equatable {
    let id: String
    let edition: String
    let uid: String
    let nickname: String
    let email: String?
    let authorizations: [QoderAuthorization]

    init?(json: [String: Any]) {
        guard let id = json["id"] as? String,
              let edition = json["edition"] as? String,
              let uid = json["uid"] as? String,
              !uid.isEmpty else { return nil }
        self.id = id
        self.edition = edition
        self.uid = uid
        self.nickname = (json["nickname"] as? String) ?? ""
        self.email = (json["email"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        self.authorizations = (json["authorizations"] as? [[String: Any]] ?? [])
            .compactMap(QoderAuthorization.init(json:))
    }

    var editionName: String {
        switch edition {
        case "qoder_cn": return "Qoder CN"
        case "qoder_global": return "Qoder International"
        default: return edition
        }
    }

    var displayName: String {
        if !nickname.isEmpty { return nickname }
        if let email { return email }
        return QoderServiceAccount.masked(uid)
    }

    /// UID 只显示前 8 位：够认人，又不用整串进截图。
    var maskedUid: String { QoderServiceAccount.masked(uid) }

    static func masked(_ uid: String) -> String {
        uid.count <= 8 ? uid : String(uid.prefix(8)) + "…"
    }

    /// 来源标签：Magpie / 浏览器登录。
    var sourceLabels: [String] {
        authorizations.map(\.sourceLabel)
    }

    /// 真正可用的来源：AICC 自己持有凭据且状态有效。
    var usableSources: [QoderAuthorization] {
        authorizations.filter { $0.isUsable }
    }

    /// 只有 Magpie 关联、还没用它向服务端验证过的来源。
    var linkedSources: [QoderAuthorization] {
        authorizations.filter { $0.isLinkedOnly }
    }

    /// 这个账号是否有一份 AICC 自己持有的 PAT。
    ///
    /// PAT 与浏览器授权是两条独立的来源：有 PAT 不代表浏览器授权也有效，
    /// 所以界面上不合并成"已登录"一个状态。
    var hasPAT: Bool {
        authorizations.contains { $0.source == "own_pat" }
    }

    var patAuth: QoderAuthorization? {
        authorizations.first { $0.source == "own_pat" }
    }

    /// 界面上的授权状态。
    ///
    /// 只有"自己拿着凭据"才算**有效**；仅仅在 Magpie 里有一条关联，
    /// 不能声称认证有效——那份令牌归 Magpie，AICC 还没真正用过它。
    var statusLabel: String {
        if !usableSources.isEmpty { return "有效" }
        if authorizations.contains(where: { $0.authStatus == "expired" }) { return "已过期" }
        if !linkedSources.isEmpty { return "已关联（未校验）" }
        if authorizations.isEmpty { return "无授权" }
        return "需重新登录"
    }

    var statusTone: QoderServiceStatusTone {
        if !usableSources.isEmpty { return .ok }
        if authorizations.contains(where: { $0.authStatus == "expired" }) { return .warn }
        if !linkedSources.isEmpty { return .muted }
        if authorizations.isEmpty { return .muted }
        return .warn
    }

    var searchText: String {
        [nickname, uid, email ?? "", editionName]
            .joined(separator: " ")
            .lowercased()
    }
}

struct QoderAuthorization: Equatable {
    let source: String
    let sourceKey: String?
    let authStatus: String
    let hasSecret: Bool
    let expiresAt: String?

    init?(json: [String: Any]) {
        guard let source = json["source"] as? String else { return nil }
        self.source = source
        self.sourceKey = json["sourceKey"] as? String
        self.authStatus = json["authStatus"] as? String ?? "unknown"
        self.hasSecret = json["hasSecret"] as? Bool ?? false
        self.expiresAt = json["tokenExpiresAt"] as? String
    }

    var sourceLabel: String {
        switch source {
        case "magpie": return "Magpie"
        case "own_browser": return "浏览器登录"
        case "own_pat": return "PAT"
        default: return source
        }
    }

    /// 只有自有授权（浏览器或 PAT）、且凭据真的存下来了，才算"可用"。
    ///
    /// Magpie 来源的令牌归 Magpie 刷新，AICC 手里没有任何凭据可验证，
    /// 因此不能把它当成"认证有效"，只能说"已关联"。
    var isUsable: Bool {
        (source == "own_browser" || source == "own_pat") && hasSecret && authStatus == "valid"
    }

    /// Magpie 关联：知道这个账号在 Magpie 里登录过，但还没实时验证。
    var isLinkedOnly: Bool {
        source == "magpie" && sourceKey != nil
    }

    var statusLabel: String {
        switch authStatus {
        case "valid": return "有效"
        case "expired": return "已过期"
        case "invalid": return "已失效"
        case "denied": return "授权被拒"
        case "missing": return "未登录"
        default: return "待检查"
        }
    }
}

enum QoderServiceStatusTone {
    case ok
    case warn
    case muted
}

/// `qoder.accounts.delete` 的返回值。
///
/// `secretsDeleted + secretsMissing` 是该账号可能存在的三条键：
/// 浏览器 access、refresh、PAT。重试时缺失的键按"已删"计，
/// 所以界面不该把 `secretsMissing > 0` 当成异常。
struct QoderAccountDeletion: Equatable {
    let accountId: String
    let edition: QoderEdition
    let removed: Bool
    let secretsDeleted: Int
    let secretsMissing: Int

    static func parse(_ json: [String: Any]) -> QoderAccountDeletion? {
        guard let accountId = json["accountId"] as? String,
              let raw = json["edition"] as? String,
              let edition = QoderEdition(rawValue: raw),
              let removed = json["removed"] as? Bool
        else { return nil }
        return QoderAccountDeletion(
            accountId: accountId,
            edition: edition,
            removed: removed,
            secretsDeleted: (json["secretsDeleted"] as? Int) ?? 0,
            secretsMissing: (json["secretsMissing"] as? Int) ?? 0
        )
    }
}
