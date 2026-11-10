import Foundation

/// 主控制台用的账号视图模型。
///
/// 只做**解析与派生**，不碰网络和进程，所以能进 `swift test`。
/// 数据来源只有 PR1a 的 `accounts.list`；没有账号就是空数组，绝不填充演示数据。
struct ConsoleAccount: Identifiable, Equatable {
    enum AuthTone: Equatable {
        case ok
        case warning
        case error
        case neutral

        var label: String {
            switch self {
            case .ok: return "正常"
            case .warning: return "待验证"
            case .error: return "失效"
            case .neutral: return "未知"
            }
        }
    }

    let id: String
    let platform: String
    let uid: String
    let nickname: String
    let authStatus: String
    let checkinEnabled: Bool
    let updatedAt: String?

    init(
        id: String,
        platform: String,
        uid: String,
        nickname: String,
        authStatus: String = "unknown",
        checkinEnabled: Bool = false,
        updatedAt: String? = nil
    ) {
        self.id = id
        self.platform = platform
        self.uid = uid
        let trimmed = nickname.trimmingCharacters(in: .whitespacesAndNewlines)
        self.nickname = trimmed.isEmpty ? "未命名账号" : trimmed
        self.authStatus = authStatus
        self.checkinEnabled = checkinEnabled
        self.updatedAt = updatedAt
    }

    /// 助手返回的是 camelCase（见 `AccountRecord` 的 serde 配置）。
    init?(json: [String: Any]) {
        guard let id = json["id"] as? String,
              let platform = json["platform"] as? String,
              let uid = json["uid"] as? String else { return nil }
        self.init(
            id: id,
            platform: platform,
            uid: uid,
            nickname: json["nickname"] as? String ?? "",
            authStatus: json["authStatus"] as? String ?? "unknown",
            checkinEnabled: json["checkinEnabled"] as? Bool ?? false,
            updatedAt: json["updatedAt"] as? String
        )
    }

    var platformName: String {
        platform == "workbuddy" ? "WorkBuddy" : (platform == "qoder_cn" ? "Qoder CN" : platform)
    }

    var symbolName: String {
        platform == "workbuddy" ? "bubble.left.and.bubble.right.fill" : "chevron.left.forwardslash.chevron.right"
    }

    var authLabel: String {
        switch authStatus {
        case "valid": return "认证有效"
        case "expired": return "已过期"
        case "invalid": return "认证失效"
        case "denied": return "授权被拒"
        default: return "待验证"
        }
    }

    var authTone: AuthTone {
        switch authStatus {
        case "valid": return .ok
        case "expired", "invalid", "denied": return .error
        case "unknown": return .neutral
        default: return .warning
        }
    }

    /// 搜索用：昵称 + UID，统一小写。
    var searchText: String {
        "\(nickname) \(uid)".lowercased()
    }
}

enum ConsolePlatformFilter: String, CaseIterable, Identifiable {
    case all
    case workbuddy
    case qoder

    var id: String { rawValue }

    var title: String {
        switch self {
        case .all: return "全部"
        case .workbuddy: return "WorkBuddy"
        case .qoder: return "Qoder CN"
        }
    }

    func includes(_ account: ConsoleAccount) -> Bool {
        switch self {
        case .all: return true
        case .workbuddy: return account.platform == "workbuddy"
        case .qoder: return account.platform == "qoder_cn"
        }
    }
}

/// 总览页的计数。全部从真实账号数组推导，没有数据时是 0 而不是占位数字。
struct ConsoleOverviewStats: Equatable {
    let total: Int
    let workbuddy: Int
    let qoder: Int
    let healthy: Int
    let attention: Int
    let checkinEnabled: Int

    init(accounts: [ConsoleAccount]) {
        total = accounts.count
        workbuddy = accounts.filter { $0.platform == "workbuddy" }.count
        qoder = accounts.filter { $0.platform == "qoder_cn" }.count
        healthy = accounts.filter { $0.authStatus == "valid" }.count
        attention = accounts.filter {
            ["expired", "invalid", "denied"].contains($0.authStatus)
        }.count
        checkinEnabled = accounts.filter(\.checkinEnabled).count
    }

    var isEmpty: Bool { total == 0 }
}

/// 额度页顶部摘要。
struct ConsoleQuotaSummary: Equatable {
    let providerCount: Int
    let cardCount: Int
    let stale: Bool
    let updatedAt: String?

    init(snapshot: QuotaSnapshot?) {
        providerCount = snapshot?.providers.count ?? 0
        cardCount = snapshot?.accounts.count ?? 0
        stale = snapshot?.stale ?? false
        updatedAt = snapshot?.updatedAt
    }

    var isEmpty: Bool { cardCount == 0 }
}
