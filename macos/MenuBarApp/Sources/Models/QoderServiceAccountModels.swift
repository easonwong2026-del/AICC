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

    /// 真正可用的来源：有凭据、或至少有 Magpie 关联的行。
    var usableSources: [QoderAuthorization] {
        authorizations.filter { $0.isUsable }
    }

    /// 界面上的授权状态：以"最好的那一份"为准，没有可用来源时退回最差状态。
    var statusLabel: String {
        if let best = authorizations.first(where: { $0.isUsable }) {
            return best.statusLabel
        }
        if authorizations.isEmpty { return "无授权" }
        if authorizations.contains(where: { $0.authStatus == "expired" }) { return "已过期" }
        return "需重新登录"
    }

    var statusTone: QoderServiceStatusTone {
        if authorizations.contains(where: { $0.isUsable }) { return .ok }
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
        default: return source
        }
    }

    /// Magpie 来源不持有 AICC 自己的凭据（令牌归 Magpie 刷新），
    /// 所以"有 sourceKey"就算可用；自有授权必须有凭据。
    var isUsable: Bool {
        switch authStatus {
        case "valid": return source == "magpie" ? sourceKey != nil : hasSecret
        default: return false
        }
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
