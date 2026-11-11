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
    /// 访问令牌到期时间（RFC3339）。没有登录态时为 nil。
    let tokenExpiresAt: String?
    /// `own` = AICC 自己扫码授权；`magpie` = 从 Magpie 借用的 access token。
    let authSource: String

    init(
        id: String,
        platform: String,
        uid: String,
        nickname: String,
        authStatus: String = "unknown",
        checkinEnabled: Bool = false,
        updatedAt: String? = nil,
        tokenExpiresAt: String? = nil,
        authSource: String = "own"
    ) {
        self.id = id
        self.platform = platform
        self.uid = uid
        let trimmed = nickname.trimmingCharacters(in: .whitespacesAndNewlines)
        self.nickname = trimmed.isEmpty ? "未命名账号" : trimmed
        self.authStatus = authStatus
        self.checkinEnabled = checkinEnabled
        self.updatedAt = updatedAt
        self.tokenExpiresAt = tokenExpiresAt
        self.authSource = authSource
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
            updatedAt: json["updatedAt"] as? String,
            tokenExpiresAt: json["tokenExpiresAt"] as? String,
            authSource: json["authSource"] as? String ?? "own"
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

    /// 来自 Magpie 的借用授权：令牌不在 AICC 手里刷新，过期要回 Magpie 同步。
    var isBorrowed: Bool { authSource == "magpie" }

    /// 卡片右上角的来源标签；独立授权的账号不显示。
    var sourceLabel: String? { isBorrowed ? "Magpie" : nil }

    /// 令牌已过期（或没有到期时间却标着 valid）时，界面必须显示"已过期"，
    /// 不能让用户以为还能用。
    func isTokenExpired(now: Date = .now) -> Bool {
        guard let tokenExpiresAt, !tokenExpiresAt.isEmpty else { return false }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        guard let expiry = parser.date(from: tokenExpiresAt) else { return false }
        return expiry <= now
    }

    /// 列表里显示的登录状态。
    var displayAuthLabel: String {
        isTokenExpired() ? "已过期" : authLabel
    }

    var displayAuthTone: AuthTone {
        isTokenExpired() ? .error : authTone
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

// MARK: - WorkBuddy 扫码登录

/// `auth.workbuddy.start` 的返回值。
struct WorkbuddyLoginStart: Equatable {
    let loginId: String
    let verificationUri: String
    let expiresIn: Int

    static func parse(_ json: [String: Any]) -> WorkbuddyLoginStart? {
        guard let loginId = json["loginId"] as? String, !loginId.isEmpty else { return nil }
        let uri = (json["verificationUri"] as? String) ?? (json["qrPayload"] as? String) ?? ""
        guard !uri.isEmpty else { return nil }
        let expires = (json["expiresIn"] as? NSNumber)?.intValue ?? 600
        return WorkbuddyLoginStart(loginId: loginId, verificationUri: uri, expiresIn: expires)
    }
}

/// 轮询结果。失败也要区分类型：取消 / 过期 / 网络 / 档位不符，界面提示不一样。
enum WorkbuddyLoginPhase: Equatable {
    case pending
    case success(reused: Bool)
    case cancelled
    case expired
    case finished
    case failed(kind: String, message: String)

    var isTerminal: Bool {
        if case .pending = self { return false }
        return true
    }

    static func parse(_ json: [String: Any]) -> WorkbuddyLoginPhase {
        let state = json["state"] as? String ?? "failed"
        switch state {
        case "pending":
            return .pending
        case "success":
            return .success(reused: json["reused"] as? Bool ?? false)
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
                message: error?["message"] as? String ?? "登录失败"
            )
        }
    }
}

// MARK: - Magpie 账号导入

/// `providers.magpie.accounts` 的单个候选账号。**不含任何令牌**。
struct MagpieAccountCandidate: Identifiable, Equatable {
    let key: String
    let uid: String
    let nickname: String
    let expiresInHours: Double?
    let expired: Bool
    let imported: Bool
    let importedSource: String?
    /// 已导入时对应的 AICC 账号 id，用于导入后立刻校验登录态。
    let accountId: String?

    var id: String { key }

    var isAvailable: Bool { !expired && !imported }

    var statusText: String {
        if imported {
            return importedSource == "own" ? "已独立授权" : "已导入"
        }
        if expired { return "令牌已过期" }
        guard let hours = expiresInHours else { return "有效期未知" }
        if hours >= 48 {
            return "约 \(Int(hours / 24)) 天后过期"
        }
        return "约 \(Int(hours.rounded())) 小时后过期"
    }

    static func parse(_ json: [String: Any]) -> MagpieAccountCandidate? {
        guard let key = json["key"] as? String, !key.isEmpty,
              let uid = json["uid"] as? String, !uid.isEmpty else { return nil }
        return MagpieAccountCandidate(
            key: key,
            uid: uid,
            nickname: json["nickname"] as? String ?? "WorkBuddy 账号",
            expiresInHours: (json["expiresInHours"] as? NSNumber)?.doubleValue,
            expired: json["expired"] as? Bool ?? false,
            imported: json["imported"] as? Bool ?? false,
            importedSource: json["importedSource"] as? String,
            accountId: json["accountId"] as? String
        )
    }
}

struct MagpiePreview: Equatable {
    let available: Bool
    let message: String?
    let candidates: [MagpieAccountCandidate]

    var importable: [MagpieAccountCandidate] { candidates.filter(\.isAvailable) }

    static func parse(_ json: [String: Any]) -> MagpiePreview {
        let accounts = (json["accounts"] as? [[String: Any]] ?? []).compactMap(MagpieAccountCandidate.parse)
        return MagpiePreview(
            available: json["available"] as? Bool ?? false,
            message: json["message"] as? String,
            candidates: accounts
        )
    }
}

struct MagpieImportResult: Equatable {
    let imported: Int
    let skipped: Int

    static func parse(_ json: [String: Any]) -> MagpieImportResult {
        MagpieImportResult(
            imported: (json["imported"] as? NSNumber)?.intValue ?? 0,
            skipped: (json["skipped"] as? [Any])?.count ?? 0
        )
    }
}
