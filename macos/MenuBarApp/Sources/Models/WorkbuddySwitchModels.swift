import Foundation

/// WorkBuddy **客户端**账号切换（`workbuddy.client.*`）的 Swift 侧模型。
///
/// 三条不能含糊的界线（对应 `docs/WORKBUDDY_CLIENT_SWITCH.md`）：
///
/// 1. "当前登录"只认 `live.uid` —— 它来自客户端自己的认证文件。
///    AICC 里选中了谁、查询优先用哪份授权，都不能代替这个状态。
/// 2. `switchable = false` 必须显示**具体原因**，不显示一个禁用的灰按钮了事。
/// 3. 失败分两种：**已回滚**（登录态没变）与**恢复失败**（要用户手动处理）。
///    混成一句"切换失败"会让人以为账号已经切过去了。
///
/// 这里只翻 JSON，不做任何判断；判定全部由 Rust 侧给出。

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
    }

    /// 界面上那一句"当前登录"。读不到就说"读不到"，不显示"未登录"糊过去。
    var summary: String {
        guard loggedIn, let uid else {
            if let error { return "读不到客户端登录态（\(error)）" }
            return "客户端当前未登录"
        }
        return "已登录 \(Self.shortUID(uid))"
    }

    static func shortUID(_ uid: String) -> String {
        uid.count > 12 ? String(uid.prefix(8)) + "…" : uid
    }
}

// MARK: - 可切换性

/// 不能切换的稳定分类（Rust 侧 `reason::*`）。
enum WorkbuddySwitchReason: String {
    case ok
    case live
    case unsupportedProvider = "unsupported_provider"
    case clientNotInstalled = "client_not_installed"
    case noCredential = "no_credential"
    case missingRefresh = "missing_refresh"
    case magpieNoRefresh = "magpie_no_refresh"

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
        case .magpieNoRefresh: return "只有 Magpie 关联授权（没有刷新令牌），暂不支持切换"
        }
    }
}

struct WorkbuddySwitchCandidate: Equatable, Identifiable {
    let accountId: String
    let uid: String
    let nickname: String
    let switchable: Bool
    let reason: WorkbuddySwitchReason

    var id: String { accountId }

    init(json: [String: Any]) {
        accountId = json["accountId"] as? String ?? ""
        uid = json["uid"] as? String ?? ""
        nickname = json["nickname"] as? String ?? ""
        switchable = json["switchable"] as? Bool ?? false
        reason = WorkbuddySwitchReason(raw: json["reason"] as? String)
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

    init(json: [String: Any]) {
        id = json["id"] as? String ?? ""
        targetAccountId = json["targetAccountId"] as? String ?? ""
        targetUid = json["targetUid"] as? String ?? ""
        previousUid = json["previousUid"] as? String
        phase = json["phase"] as? String ?? "unknown"
        detail = json["detail"] as? String
        createdAt = Self.parseTimestamp(json["createdAt"] as? String)
        activityBefore = (json["activityBefore"] as? [String: Any]).map(WorkbuddyActivityEvidence.init)
        activityAfter = (json["activityAfter"] as? [String: Any]).map(WorkbuddyActivityEvidence.init)
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

    /// 左栏要不要主动提示"恢复上一个"。
    ///
    /// 只有**需要人管**的情况才提示：写入没被确认（`unconfirmed`）或恢复失败过（`failed`）。
    /// 切换成功时不提示 —— 想回到上一个账号，去点那个账号的「切换到此账号」更干净
    /// （重新走一遍凭据 + 校验，而不是把一份旧快照塞回去）。
    var needsRestoreOffer: Bool {
        canRestore && (phase == "unconfirmed" || phase == "failed")
    }

    var phaseText: String {
        switch phase {
        case "prepared": return "准备中"
        case "written": return "已写入，未确认"
        case "launched": return "客户端已启动，校验中"
        case "verified": return "已确认切换"
        case "unconfirmed": return "已写入，但没确认到客户端"
        case "failed": return "失败"
        case "restored": return "已恢复"
        default: return phase
        }
    }

    private static func parseTimestamp(_ raw: String?) -> Date? {
        guard let raw, !raw.isEmpty else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: raw) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: raw)
    }
}

// MARK: - 整体状态

struct WorkbuddyClientStatus: Equatable {
    let client: WorkbuddyClientState
    let live: WorkbuddyLiveLogin
    let candidates: [WorkbuddySwitchCandidate]
    let pendingSwitch: WorkbuddySwitchJournal?
    let lastSwitch: WorkbuddySwitchJournal?
    let fetchedAt: Date

    init(json: [String: Any]) {
        client = WorkbuddyClientState(json: json["client"] as? [String: Any])
        live = WorkbuddyLiveLogin(json: json["live"] as? [String: Any])
        candidates = (json["candidates"] as? [[String: Any]] ?? [])
            .map(WorkbuddySwitchCandidate.init)
        pendingSwitch = (json["pendingSwitch"] as? [String: Any])
            .map(WorkbuddySwitchJournal.init)
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
    let activityBefore: WorkbuddyActivityEvidence?
    let activityAfter: WorkbuddyActivityEvidence?

    init(json: [String: Any]) {
        changed = json["changed"] as? Bool ?? false
        verified = json["verified"] as? Bool ?? false
        phase = json["phase"] as? String ?? "unknown"
        verifyDetail = json["verifyDetail"] as? String
        previousUid = json["previousUid"] as? String
        clientRunning = json["clientRunning"] as? Bool
        activityBefore = (json["activityBefore"] as? [String: Any])
            .map(WorkbuddyActivityEvidence.init)
        activityAfter = (json["activityAfter"] as? [String: Any])
            .map(WorkbuddyActivityEvidence.init)
    }

    /// 结果要能回答"到底切没切成"，而不是只说"操作完成"。
    var summary: String {
        if !changed { return "客户端本来就是这个账号，没有改动任何文件" }
        if verified {
            return "已切换，客户端已接受这个登录（重启后仍然是它）"
        }
        return "已写入并重启客户端，但没能确认客户端采用了它；请打开客户端确认，"
            + "需要的话恢复原账号"
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
