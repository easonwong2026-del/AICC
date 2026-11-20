import Foundation

/// WorkBuddy **客户端**账号切换（`workbuddy.client.*`）的 Swift 侧模型。
///
/// 四条不能含糊的界线（对应 `docs/WORKBUDDY_CLIENT_SWITCH.md`）：
///
/// 1. "当前登录"只认 `live.uid` —— 它来自客户端自己的认证文件。
///    AICC 里选中了谁、查询优先用哪份授权，都不能代替这个状态。
/// 2. 两种授权必须能一眼分开：`own_refresh` = 长期，`magpie_access_temporary` = 会过期。
///    文案、按钮、确认弹层都要跟着变，不能混成一种"长期授权"。
/// 3. `switchable = false` 必须显示**具体原因**，不显示一个禁用的灰按钮了事。
/// 4. 失败分两种：**已回滚**（登录态没变）与**恢复失败**（要用户手动处理）。
///    混成一句"切换失败"会让人以为账号已经切过去了。
///
/// 这里只翻 JSON，不做任何"这份授权还能不能用"的推断；判定全部由 Rust 侧给出。

// MARK: - 时间

enum WorkbuddyTime {
    static func parse(_ raw: String?) -> Date? {
        guard let raw, !raw.isEmpty else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: raw) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: raw)
    }

    /// 把剩余秒数说成人话。`nil` = 不知道，**不是 0**。
    static func remaining(_ seconds: Int?) -> String {
        guard let seconds else { return "有效期未知" }
        if seconds <= 0 { return "已过期" }
        if seconds < 3600 { return "剩余 \(max(1, seconds / 60)) 分钟" }
        let hours = seconds / 3600
        let minutes = (seconds % 3600) / 60
        return minutes == 0 ? "剩余 \(hours) 小时" : "剩余 \(hours) 小时 \(minutes) 分"
    }

    /// `MM-dd HH:mm`，只用于展示到期时间。
    static func short(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }
}

// MARK: - 客户端

struct WorkbuddyClientState: Equatable {
    let installed: Bool
    let appPath: String
    let running: Bool?
    let authFile: String
    let authFileExists: Bool
    let logoutMarker: Bool

    init(json: [String: Any]?) {
        let json = json ?? [:]
        installed = json["installed"] as? Bool ?? false
        appPath = json["appPath"] as? String ?? ""
        running = json["running"] as? Bool
        authFile = json["authFile"] as? String ?? ""
        authFileExists = json["authFileExists"] as? Bool ?? false
        logoutMarker = json["logoutMarker"] as? Bool ?? false
    }

    /// 一句话状态。**没有客户端时说"未安装"，不猜"未登录"**。
    var summary: String {
        guard installed else { return "未安装 WorkBuddy 客户端" }
        guard let running else { return "客户端状态未知" }
        return running ? "客户端运行中" : "客户端未运行"
    }
}

// MARK: - 当前登录

struct WorkbuddyLiveLogin: Equatable {
    let uid: String?
    let loggedIn: Bool
    /// `plaintext` / `envelope` / `mixed`；只作诊断。
    let tokenForm: String?
    let accountsInFile: Int
    let modifiedAt: Date?
    let error: String?
    /// 当前这份登录是临时借来的（Magpie access-only）。
    let temporary: Bool
    let temporaryAccountId: String?
    let accessExpiresAt: Date?
    let remainingSeconds: Int?
    /// 建议最晚恢复时间（到期前留出安全余量）。只有临时登录才有。
    let safeRestoreUntil: Date?

    init(json: [String: Any]?) {
        let json = json ?? [:]
        uid = json["uid"] as? String
        loggedIn = json["loggedIn"] as? Bool ?? false
        tokenForm = json["tokenForm"] as? String
        accountsInFile = (json["accountsInFile"] as? NSNumber)?.intValue ?? 0
        if let ms = (json["modifiedAtMs"] as? NSNumber)?.doubleValue, ms > 0 {
            modifiedAt = Date(timeIntervalSince1970: ms / 1000)
        } else {
            modifiedAt = nil
        }
        error = json["error"] as? String
        temporary = json["temporary"] as? Bool ?? false
        temporaryAccountId = json["temporaryAccountId"] as? String
        accessExpiresAt = WorkbuddyTime.parse(json["accessExpiresAt"] as? String)
        remainingSeconds = (json["remainingSeconds"] as? NSNumber)?.intValue
        safeRestoreUntil = WorkbuddyTime.parse(json["safeRestoreUntil"] as? String)
    }

    /// 界面上那一句"当前登录"。读不到就说"读不到"，不显示"未登录"糊过去。
    var summary: String {
        guard loggedIn, let uid else {
            if let error { return "读不到客户端登录态（\(error)）" }
            return "客户端当前未登录"
        }
        return "已登录 \(Self.shortUID(uid))"
    }

    /// 临时登录时那行补充说明：到期时间 + 剩余。
    var temporarySummary: String? {
        guard temporary, loggedIn else { return nil }
        let expiry = accessExpiresAt.map { WorkbuddyTime.short($0) } ?? "未知时间"
        var text = "临时登录 · 有效至 \(expiry)（\(WorkbuddyTime.remaining(remainingSeconds))）"
        // 已过建议恢复时间：必须说"尽快回去"，不能只报剩余时间。
        if let safe = safeRestoreUntil, Date() >= safe {
            text += " · 建议立即恢复"
        }
        return text
    }

    static func shortUID(_ uid: String) -> String {
        uid.count > 12 ? String(uid.prefix(8)) + "…" : uid
    }
}

// MARK: - 授权模式

enum WorkbuddySwitchMode: String {
    /// AICC 自有授权：access + refresh，客户端自己续期。
    case ownRefresh = "own_refresh"
    /// Magpie 借来的 access：一次性、会过期。
    case magpieTemporary = "magpie_access_temporary"

    init(raw: String?) {
        self = WorkbuddySwitchMode(rawValue: raw ?? "") ?? .ownRefresh
    }

    var temporary: Bool { self == .magpieTemporary }

    var buttonTitle: String {
        temporary ? "临时切换到此账号" : "切换到此账号"
    }

    var badgeTitle: String { temporary ? "Magpie 临时登录" : "AICC 自有授权" }
}

/// 服务端是否接受这份 access。**客户端停在这个账号上 ≠ 服务端认这份令牌。**
enum WorkbuddyAuthAccepted: String {
    case accepted
    case rejected
    case unknown

    init(raw: String?) {
        self = WorkbuddyAuthAccepted(rawValue: raw ?? "") ?? .unknown
    }

    var text: String {
        switch self {
        case .accepted: return "服务端已接受这个登录"
        case .rejected: return "服务端拒绝了这份令牌"
        case .unknown: return "服务端是否接受尚未确认"
        }
    }

    var isBad: Bool { self == .rejected }
}

// MARK: - 可切换性

/// 不能切换的稳定分类（Rust 侧 `plan::reason::*`）。
enum WorkbuddySwitchReason: String {
    case ok
    case live
    case unsupportedProvider = "unsupported_provider"
    case clientNotInstalled = "client_not_installed"
    case noCredential = "no_credential"
    case missingRefresh = "missing_refresh"
    case magpieUnavailable = "magpie_unavailable"
    case magpieSourceMissing = "magpie_source_missing"
    case magpieUidMismatch = "magpie_uid_mismatch"
    case magpiePlatformMismatch = "magpie_platform_mismatch"
    case magpieAccessEmpty = "magpie_access_empty"
    case magpieAccessExpired = "magpie_access_expired"
    case magpieExpiryUnknown = "magpie_expiry_unknown"
    case magpieTooShort = "magpie_too_short"
    case liveChanged = "live_changed"

    init(raw: String?) {
        self = WorkbuddySwitchReason(rawValue: raw ?? "") ?? .noCredential
    }

    /// 为什么不能切。**每一条都要能指导下一步动作**，不能只说"不支持"。
    var explanation: String {
        switch self {
        case .ok: return ""
        case .live: return "客户端当前登录的就是这个账号"
        case .unsupportedProvider: return "国际版客户端切换还没有实测依据，暂不开放"
        case .clientNotInstalled: return "这台机器上没找到 WorkBuddy 客户端"
        case .noCredential: return "没有可用授权，先登录这个账号"
        case .missingRefresh: return "这份授权缺刷新令牌，切过去后无法续期；请重新用浏览器登录"
        case .magpieUnavailable: return "读不到 Magpie 的登录文件；先确认 Magpie 正在运行并已登录"
        case .magpieSourceMissing: return "Magpie 里已经没有这条登录态，重新导入或在 Magpie 里重新登录"
        case .magpieUidMismatch: return "Magpie 里这条登录态属于另一个账号，已拒绝"
        case .magpiePlatformMismatch: return "Magpie 里这条登录态不属于国内版 WorkBuddy"
        case .magpieAccessEmpty: return "Magpie 里的访问令牌是空的"
        case .magpieAccessExpired: return "Magpie 的访问令牌已过期，先在 Magpie 里重新登录"
        case .magpieExpiryUnknown: return "Magpie 没给出有效期，无法判断临时登录能用多久"
        case .magpieTooShort: return "Magpie 令牌剩余有效期太短，不够完成一次临时切换"
        case .liveChanged: return "客户端当前登录的已经不是你切换过去的账号"
        }
    }

    /// 需要用户去别处处理（重新登录 / 重新导入），而不是"这里暂时不行"。
    var isActionableElsewhere: Bool {
        switch self {
        case .noCredential, .missingRefresh,
             .magpieUnavailable, .magpieSourceMissing, .magpieAccessExpired:
            return true
        default:
            return false
        }
    }
}

struct WorkbuddySwitchCandidate: Equatable, Identifiable {
    let accountId: String
    let uid: String
    let nickname: String
    let switchable: Bool
    let reason: WorkbuddySwitchReason
    /// Rust 给的补充说明（可能是"首选来源不可用，已改用另一份"）。
    let reasonDetail: String?
    let mode: WorkbuddySwitchMode?
    let temporary: Bool
    let accessExpiresAt: Date?
    let remainingSeconds: Int?

    var id: String { accountId }

    init(json: [String: Any]) {
        accountId = json["accountId"] as? String ?? ""
        uid = json["uid"] as? String ?? ""
        nickname = json["nickname"] as? String ?? ""
        switchable = json["switchable"] as? Bool ?? false
        reason = WorkbuddySwitchReason(raw: json["reason"] as? String)
        let detail = (json["reasonDetail"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        reasonDetail = (detail?.isEmpty ?? true) ? nil : detail
        mode = (json["mode"] as? String).flatMap { $0.isEmpty ? nil : WorkbuddySwitchMode(raw: $0) }
        temporary = json["temporary"] as? Bool ?? false
        accessExpiresAt = WorkbuddyTime.parse(json["accessExpiresAt"] as? String)
        remainingSeconds = (json["remainingSeconds"] as? NSNumber)?.intValue
    }

    /// 临时模式的副标题：还能用多久。
    var temporaryNote: String? {
        guard switchable, temporary else { return nil }
        return "Magpie 令牌\(WorkbuddyTime.remaining(remainingSeconds))"
    }
}

// MARK: - 活跃证据

/// 切换前后各取一次的服务端活跃快照。**只是证据**，不参与任何判定。
struct WorkbuddyActivityEvidence: Equatable {
    let state: String
    let diagnosis: String
    let serverDate: String?
    let todayActive: Bool?
    let streakDays: Int?
    let error: String?

    init(json: [String: Any]?) {
        let json = json ?? [:]
        state = json["state"] as? String ?? "unknown"
        diagnosis = json["diagnosis"] as? String ?? "unknown"
        serverDate = json["serverDate"] as? String
        todayActive = json["todayActive"] as? Bool
        streakDays = (json["streakDays"] as? NSNumber)?.intValue
        error = json["error"] as? String
    }

    /// `true` / `false` / `nil`（没问到）。**nil 不能显示成"未活跃"**。
    var activeText: String {
        guard let todayActive else { return "活跃未知" }
        return todayActive ? "今日已活跃" : "今日未活跃"
    }

    var streakText: String {
        guard let streakDays else { return "连登未知" }
        return "连登 \(streakDays) 天"
    }

    var summary: String {
        if let error { return "活跃查不到（\(error)）" }
        return "\(activeText) · \(streakText)"
    }
}

// MARK: - 切换记录

struct WorkbuddySwitchJournal: Equatable, Identifiable {
    let id: String
    let targetAccountId: String
    let targetUid: String
    let previousUid: String?
    /// `prepared` / `written` / `launched` / `verified` / `unconfirmed` / `failed` / `restored`
    let phase: String
    let detail: String?
    let createdAt: Date?
    let activityBefore: WorkbuddyActivityEvidence?
    let activityAfter: WorkbuddyActivityEvidence?
    /// 这次切换用的是哪份授权：`own` / `magpie`。
    let authSource: String?
    let temporary: Bool
    let accessExpiresAt: Date?
    /// `pending` / `done` / `superseded`。与"切换是否在执行"是两件事。
    let restoreState: String
    let authAccepted: WorkbuddyAuthAccepted

    init(json: [String: Any]) {
        id = json["id"] as? String ?? ""
        targetAccountId = json["targetAccountId"] as? String ?? ""
        targetUid = json["targetUid"] as? String ?? ""
        previousUid = json["previousUid"] as? String
        phase = json["phase"] as? String ?? "unknown"
        detail = json["detail"] as? String
        createdAt = WorkbuddyTime.parse(json["createdAt"] as? String)
        activityBefore = (json["activityBefore"] as? [String: Any]).map(WorkbuddyActivityEvidence.init)
        activityAfter = (json["activityAfter"] as? [String: Any]).map(WorkbuddyActivityEvidence.init)
        authSource = json["authSource"] as? String
        temporary = json["temporary"] as? Bool ?? false
        accessExpiresAt = WorkbuddyTime.parse(json["accessExpiresAt"] as? String)
        restoreState = json["restoreState"] as? String ?? "pending"
        authAccepted = WorkbuddyAuthAccepted(raw: json["authAccepted"] as? String)
    }

    /// 还没走完的切换（`prepared` / `written` / `launched`）。
    var isPending: Bool {
        ["prepared", "written", "launched"].contains(phase)
    }

    /// 这份记录是否还带着一份可用的"切换前登录态"。
    var canRestore: Bool {
        guard let previousUid, !previousUid.isEmpty else { return false }
        return ["verified", "unconfirmed", "failed"].contains(phase)
    }

    /// 左栏要不要主动提示"恢复原账号"。
    ///
    /// - 需要人管的情况：写入没被确认（`unconfirmed`）或恢复失败过（`failed`）。
    /// - **临时登录成功（`verified`）也要提示**：它必然会过期，用户必须在到期前回去。
    ///   这是 Magpie 临时模式与自有授权最大的行为差别，不能漏。
    /// - 已经恢复过（`restore_state = done`）或被后一次切换取代，就不再提示。
    var needsRestoreOffer: Bool {
        guard canRestore, restoreState == "pending" else { return false }
        if phase == "verified" { return temporary }
        return true
    }

    var phaseText: String {
        switch phase {
        case "prepared": return "准备中"
        case "written": return "已写入，未确认"
        case "launched": return "客户端已启动，校验中"
        case "verified": return temporary ? "已临时切换" : "已确认切换"
        case "unconfirmed": return "已写入，但没确认到客户端"
        case "failed": return "失败"
        case "restored": return "已恢复"
        default: return phase
        }
    }

    /// 临时登录的到期说明；自有授权返回 nil。
    var temporaryNote: String? {
        guard temporary else { return nil }
        guard let accessExpiresAt else { return "临时登录（到期时间未知）" }
        return "临时登录 · 有效至 \(WorkbuddyTime.short(accessExpiresAt))"
    }
}

// MARK: - 整体状态

struct WorkbuddyClientStatus: Equatable {
    let client: WorkbuddyClientState
    let live: WorkbuddyLiveLogin
    let candidates: [WorkbuddySwitchCandidate]
    let pendingSwitch: WorkbuddySwitchJournal?
    /// 还没还原的临时登录（Magpie access-only）。成功后**一直存在**，直到用户恢复。
    let temporaryRestore: WorkbuddySwitchJournal?
    let restoreAvailable: Bool
    let lastSwitch: WorkbuddySwitchJournal?
    let fetchedAt: Date

    init(json: [String: Any]) {
        client = WorkbuddyClientState(json: json["client"] as? [String: Any])
        live = WorkbuddyLiveLogin(json: json["live"] as? [String: Any])
        candidates = (json["candidates"] as? [[String: Any]] ?? [])
            .map(WorkbuddySwitchCandidate.init)
        pendingSwitch = (json["pendingSwitch"] as? [String: Any])
            .map(WorkbuddySwitchJournal.init)
        temporaryRestore = (json["temporaryRestore"] as? [String: Any])
            .map(WorkbuddySwitchJournal.init)
        restoreAvailable = json["restoreAvailable"] as? Bool ?? false
        lastSwitch = (json["lastSwitch"] as? [String: Any])
            .map(WorkbuddySwitchJournal.init)
        fetchedAt = Date()
    }

    func candidate(forAccount id: String) -> WorkbuddySwitchCandidate? {
        candidates.first { $0.accountId == id }
    }

    /// 当前登录账号在 AICC 里的 id（对不上任何账号时 nil：客户端登录的是别的账号）。
    var liveAccountID: String? {
        guard let uid = live.uid else { return nil }
        return candidates.first { $0.uid == uid }?.accountId
    }

    /// 左栏要提示的那条"可恢复"记录：未完成的切换优先，其次是待还原的临时登录。
    var restoreOffer: WorkbuddySwitchJournal? {
        if let pendingSwitch, pendingSwitch.isPending { return pendingSwitch }
        if let temporaryRestore { return temporaryRestore }
        if let lastSwitch, lastSwitch.needsRestoreOffer { return lastSwitch }
        return nil
    }
}

// MARK: - 操作结果

struct WorkbuddySwitchOutcome: Equatable {
    /// `false` = 客户端本来就是目标账号，没有动任何文件。
    let changed: Bool
    let verified: Bool
    let phase: String
    let verifyDetail: String?
    let previousUid: String?
    let clientRunning: Bool?
    let mode: WorkbuddySwitchMode
    let temporary: Bool
    let accessExpiresAt: Date?
    let remainingSeconds: Int?
    let authAccepted: WorkbuddyAuthAccepted
    let authAcceptedDetail: String?
    let activityBefore: WorkbuddyActivityEvidence?
    let activityAfter: WorkbuddyActivityEvidence?

    init(json: [String: Any]) {
        changed = json["changed"] as? Bool ?? false
        verified = json["verified"] as? Bool ?? false
        phase = json["phase"] as? String ?? "unknown"
        verifyDetail = json["verifyDetail"] as? String
        previousUid = json["previousUid"] as? String
        clientRunning = json["clientRunning"] as? Bool
        mode = WorkbuddySwitchMode(raw: json["mode"] as? String)
        temporary = json["temporary"] as? Bool ?? false
        accessExpiresAt = WorkbuddyTime.parse(json["accessExpiresAt"] as? String)
        remainingSeconds = (json["remainingSeconds"] as? NSNumber)?.intValue
        authAccepted = WorkbuddyAuthAccepted(raw: json["authAccepted"] as? String)
        authAcceptedDetail = json["authAcceptedDetail"] as? String
        activityBefore = (json["activityBefore"] as? [String: Any])
            .map(WorkbuddyActivityEvidence.init)
        activityAfter = (json["activityAfter"] as? [String: Any])
            .map(WorkbuddyActivityEvidence.init)
    }

    /// 结果要能回答"到底切没切成、能用多久、该什么时候回去"。
    var summary: String {
        if !changed { return "客户端本来就是这个账号，没有改动任何文件" }
        var parts: [String] = []
        parts.append(verified
            ? "已切换，客户端已接受这个登录（重启后仍然是它）"
            : "已写入并重启客户端，但没能确认客户端采用了它；请打开客户端确认")
        if temporary {
            let expiry = accessExpiresAt.map { WorkbuddyTime.short($0) } ?? "未知时间"
            parts.append("临时登录有效至 \(expiry)（\(WorkbuddyTime.remaining(remainingSeconds))），"
                + "到期前请点「恢复原账号」")
        }
        if authAccepted == .rejected, let detail = authAcceptedDetail {
            parts.append(detail)
        } else if authAccepted == .unknown, temporary {
            parts.append("服务端是否接受这份令牌还没确认")
        }
        return parts.joined(separator: "；")
    }
}

struct WorkbuddyRestoreOutcome: Equatable {
    let restored: Bool
    let uid: String?
    let detail: String?
    let clientRunning: Bool?

    init(json: [String: Any]) {
        restored = json["restored"] as? Bool ?? false
        uid = json["uid"] as? String
        detail = json["detail"] as? String
        clientRunning = json["clientRunning"] as? Bool
    }

    var summary: String {
        if let detail, !detail.isEmpty { return detail }
        return restored ? "已恢复到切换前的登录态" : "恢复未完成"
    }
}
