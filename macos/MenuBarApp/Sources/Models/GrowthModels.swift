import Foundation

/// 成长中心的视图模型（PR #67：只读）。
///
/// 字段口径与 `account-worker/src/growth/scanner.rs` 的 `AccountScan` 一一对应，
/// 改一边就要改另一边。这里只做解析与派生文案，不发请求，所以能进 `swift test`。
///
/// 一条原则：**没有的数据显示破折号，不显示 0**。`growth.credit == nil` 是
/// "没查到"，`0` 是"确实是零"，这两个在界面上必须不一样。

/// 任务分类。Rust 侧 `TaskClass` 的 camelCase 镜像。
enum GrowthTaskClass: String {
    case automatic
    case realAction
    case manual
    case unknown

    init(raw: String?) {
        switch raw {
        case "automatic": self = .automatic
        case "realAction": self = .realAction
        case "manual": self = .manual
        default: self = .unknown
        }
    }

    var title: String {
        switch self {
        case .automatic: return "自动"
        case .realAction: return "真实操作"
        case .manual: return "人工"
        case .unknown: return "待识别"
        }
    }
}

/// 任务的接受 / 完成状态。与 Rust 侧 `TaskStatus` 同名字符串。
enum GrowthTaskStatus: String {
    case notAccepted = "not_accepted"
    case accepted
    case inProgress = "in_progress"
    case completed
    case claimed
    case locked
    case unknown

    init(raw: String?) {
        self = GrowthTaskStatus(rawValue: raw ?? "") ?? .unknown
    }

    var title: String {
        switch self {
        case .notAccepted: return "未接受"
        case .accepted: return "已接受"
        case .inProgress: return "进行中"
        case .completed: return "待领奖"
        case .claimed: return "已领取"
        case .locked: return "已锁定"
        case .unknown: return "待识别"
        }
    }

    /// 只有"服务端说完成"才显示待领奖强调色。
    var needsAttention: Bool { self == .completed }
}

struct GrowthTaskRow: Identifiable, Equatable {
    let code: String
    let title: String
    let condition: String
    let progressText: String
    let rewardText: String
    let status: GrowthTaskStatus
    let statusRaw: String?
    let taskClass: GrowthTaskClass
    let classReason: String
    let locked: Bool
    let jumpURL: String?

    var id: String { code }

    init?(json: [String: Any]) {
        guard let code = str(json["code"]) else { return nil }
        self.code = code
        title = str(json["title"]) ?? code
        condition = str(json["condition"])
            ?? str(json["description"])
            ?? "—"

        let current = (json["current"] as? NSNumber)?.intValue ?? 0
        let target = (json["target"] as? NSNumber)?.intValue ?? 0
        // target 为 0 表示服务端没给进度，而不是"进度为零"。
        progressText = target > 0 ? "\(min(current, target)) / \(target)" : "—"

        var rewards: [String] = []
        if let credit = (json["rewardCredit"] as? NSNumber)?.intValue, credit > 0 {
            rewards.append("+\(credit) 积分")
        }
        if let energy = (json["rewardEnergy"] as? NSNumber)?.intValue, energy > 0 {
            rewards.append("+\(energy) 能量")
        }
        rewardText = rewards.isEmpty ? "—" : rewards.joined(separator: " · ")

        // 线上 JSON 两个字段都有（status 是枚举串，rawStatus 是原始值）。
        // 只认 rawStatus 会让缺字段的老响应变成"待识别"，所以留一个回落。
        statusRaw = str(json["rawStatus"]) ?? str(json["status"])
        status = GrowthTaskStatus(raw: statusRaw)
        taskClass = GrowthTaskClass(raw: json["class"] as? String)
        classReason = str(json["classReason"]) ?? "—"
        locked = json["locked"] as? Bool ?? false
        jumpURL = str(json["jumpUrl"])
    }

    /// 未知类型的任务必须显式标出来，不能让人以为它"和别的任务一样"。
    var isUnrecognised: Bool { taskClass == .unknown }
}

struct GrowthCounts: Equatable {
    var total = 0
    var claimable = 0
    var acceptable = 0
    var inProgress = 0
    var locked = 0
    var automatic = 0
    var realAction = 0
    var manual = 0
    var unknown = 0

    init(json: [String: Any]?) {
        guard let json else { return }
        func int(_ key: String) -> Int { (json[key] as? NSNumber)?.intValue ?? 0 }
        total = int("total")
        claimable = int("claimable")
        acceptable = int("acceptable")
        inProgress = int("inProgress")
        locked = int("locked")
        automatic = int("automatic")
        realAction = int("realAction")
        manual = int("manual")
        unknown = int("unknown")
    }
}

struct GrowthRedeemTier: Identifiable, Equatable {
    let tier: String
    let status: String
    let claimable: Bool

    var id: String { tier }

    /// `7d` → `7 天`。界面不显示服务端的英文档位名。
    var title: String {
        tier.hasSuffix("d") ? "\(tier.dropLast()) 天" : tier
    }
}

struct GrowthProfile: Equatable {
    /// 成长等级。`/profile` 给的是 `level`，不是积分。
    var level: Int?
    var completedTasks: Int?
    var totalTasks: Int?
    /// 积分。成长中心没有积分接口，#67 阶段恒为 nil，界面显示「—」。
    var credit: Int?
    var energy: Int?
    var streakDays: Int?
    var makeupCards: Int?
    var lotteryChances: Int?
    var heatmapToday: String?
    var redeem: [GrowthRedeemTier] = []

    init(json: [String: Any]?) {
        guard let json else { return }
        level = (json["level"] as? NSNumber)?.intValue
        completedTasks = (json["completedTasks"] as? NSNumber)?.intValue
        totalTasks = (json["totalTasks"] as? NSNumber)?.intValue
        credit = (json["credit"] as? NSNumber)?.intValue
        energy = (json["energy"] as? NSNumber)?.intValue
        streakDays = (json["streakDays"] as? NSNumber)?.intValue
        makeupCards = (json["makeupCards"] as? NSNumber)?.intValue
        lotteryChances = (json["lotteryChances"] as? NSNumber)?.intValue
        heatmapToday = str(json["heatmapToday"])
        redeem = (json["redeem"] as? [[String: Any]] ?? []).compactMap { item in
            guard let tier = item["tier"] as? String else { return nil }
            return GrowthRedeemTier(
                tier: tier,
                status: item["status"] as? String ?? "",
                claimable: item["claimable"] as? Bool ?? false
            )
        }
    }

    /// 顶部一行摘要。查不到的项直接不出现，不用 0 占位。
    ///
    /// 这里没有"积分"：成长中心的档案接口只给等级和完成数，积分要等
    /// #68 领奖拿到服务端回执才能累计。写个 0 会让人以为余额清零了。
    var summaryText: String {
        var parts: [String] = []
        if let level { parts.append("Lv.\(level)") }
        if let completedTasks, let totalTasks, totalTasks > 0 {
            parts.append("完成 \(completedTasks)/\(totalTasks)")
        }
        if let energy { parts.append("\(energy) 能量") }
        if let streakDays { parts.append("连签 \(streakDays) 天") }
        if let makeupCards, makeupCards > 0 { parts.append("补签卡 \(makeupCards) 张") }
        if let lotteryChances, lotteryChances > 0 { parts.append("抽奖 \(lotteryChances) 次") }
        return parts.isEmpty ? "—" : parts.joined(separator: " · ")
    }
}

struct GrowthTravel: Equatable {
    let state: String?
    let location: String?
    let dailyLimitReached: Bool?

    init?(json: [String: Any]?) {
        guard let json else { return nil }
        state = str(json["state"])
        location = str(json["location"])
        dailyLimitReached = json["dailyLimitReached"] as? Bool
        if state == nil { return nil }
    }

    var stateText: String {
        switch state {
        case "idle": return "待派出"
        case "traveling": return "旅行中"
        case "arrived": return "已到达"
        default: return state ?? "—"
        }
    }
}

/// 一个接口的错误。`field` 是接口名，不是凭据名。
struct GrowthFieldError: Identifiable, Equatable {
    let field: String
    let kind: String
    let message: String

    var id: String { field + kind }

    init(json: [String: Any]) {
        field = json["field"] as? String ?? "?"
        kind = json["kind"] as? String ?? "?"
        message = str(json["message"]) ?? kind
    }

    /// 401 / 缺凭据要人去处理，其余只是"这次没取到"。
    var needsUserAction: Bool {
        kind == "unauthorized" || kind == "missing_credential"
    }
}

/// 一个账号的扫描结果。
struct GrowthAccountScan: Identifiable, Equatable {
    let accountId: String
    let nickname: String
    /// `ok` / `partial` / `failed` / `unauthorized` / `missing_credential`
    let state: String
    let tasks: [GrowthTaskRow]
    let profile: GrowthProfile
    let travel: GrowthTravel?
    let errors: [GrowthFieldError]
    let counts: GrowthCounts
    let authSource: String?
    let pathPrefix: String?
    let scannedAt: String?

    var id: String { accountId }

    init?(json: [String: Any]) {
        guard let accountId = str(json["accountId"]) else { return nil }
        self.accountId = accountId
        nickname = str(json["nickname"]) ?? accountId
        state = json["state"] as? String ?? "failed"
        tasks = (json["tasks"] as? [[String: Any]] ?? []).compactMap(GrowthTaskRow.init(json:))
        profile = GrowthProfile(json: json["profile"] as? [String: Any])
        travel = GrowthTravel(json: json["travel"] as? [String: Any])
        errors = (json["errors"] as? [[String: Any]] ?? []).map(GrowthFieldError.init(json:))
        counts = GrowthCounts(json: json["counts"] as? [String: Any])
        authSource = str(json["authSource"])
        pathPrefix = str(json["pathPrefix"])
        scannedAt = str(json["scannedAt"])
    }

    var needsAuth: Bool { state == "unauthorized" || state == "missing_credential" }

    /// 状态徽标：能用一句话说清就不放原文。
    var stateText: String {
        switch state {
        case "ok": return "正常"
        case "partial": return "部分接口失败"
        case "failed": return "扫描失败"
        case "unauthorized": return "授权失效"
        case "missing_credential": return "缺少授权"
        default: return state
        }
    }

    var authSourceText: String? {
        guard let authSource else { return nil }
        switch authSource {
        case "magpie": return "Magpie 授权"
        case "own": return "AICC 自有授权"
        default: return authSource
        }
    }

    /// 待领奖 + 待接受，用来决定这一行要不要高亮。
    var pendingCount: Int { counts.claimable + counts.acceptable }
}

struct GrowthOverview: Equatable {
    var accounts = 0
    var claimable = 0
    var acceptable = 0
    var manual = 0
    var unknown = 0
    var traveling = 0
    var energy = 0
    var tasksCompleted = 0
    var tasksTotal = 0
    var needsAuth = 0

    init(json: [String: Any]?) {
        guard let json else { return }
        func int(_ key: String) -> Int { (json[key] as? NSNumber)?.intValue ?? 0 }
        accounts = int("accounts")
        claimable = int("claimable")
        acceptable = int("acceptable")
        manual = int("manual")
        unknown = int("unknown")
        traveling = int("traveling")
        energy = int("energy")
        tasksCompleted = int("tasksCompleted")
        tasksTotal = int("tasksTotal")
        needsAuth = int("needsAuth")
    }

    /// 一次扫描的结论，放在区域顶部。
    var headline: String {
        var parts: [String] = ["\(accounts) 个账号"]
        if claimable > 0 { parts.append("\(claimable) 项待领奖") }
        if acceptable > 0 { parts.append("\(acceptable) 项可接受") }
        if manual > 0 { parts.append("\(manual) 项需人工") }
        if unknown > 0 { parts.append("\(unknown) 项待识别") }
        if traveling > 0 { parts.append("\(traveling) 个旅行中") }
        if needsAuth > 0 { parts.append("\(needsAuth) 个需重新授权") }
        return parts.joined(separator: " · ")
    }
}

/// `growth.scan` 的完整返回。
struct GrowthScanResult: Equatable {
    let scans: [GrowthAccountScan]
    let overview: GrowthOverview

    init(json: [String: Any]) {
        scans = (json["scans"] as? [[String: Any]] ?? []).compactMap(GrowthAccountScan.init(json:))
        overview = GrowthOverview(json: json["overview"] as? [String: Any])
    }

    static let empty = GrowthScanResult(json: [:])
}

// MARK: - 自动管理（PR #68）

/// 自动管理的运行状态。**`continuesAfterQuit` 恒为 false**，
/// 界面上要如实说明"App 退出就停"，不能让用户以为有常驻服务。
struct GrowthAutoStatus: Equatable {
    var enabled = false
    var intervalSeconds = 86_400
    var running = false
    var lastTickAt: Date?
    var nextTickAt: Date?
    var lastResult: String?
    var owner = "AICC 的账号助手进程"
    var continuesAfterQuit = false

    init(json: [String: Any]?) {
        guard let json else { return }
        enabled = json["enabled"] as? Bool ?? false
        intervalSeconds = (json["intervalSeconds"] as? NSNumber)?.intValue ?? 86_400
        running = json["running"] as? Bool ?? false
        lastTickAt = Self.date(fromRFC3339: json["lastTickAt"] as? String)
        nextTickAt = Self.date(fromRFC3339: json["nextTickAt"] as? String)
        lastResult = str(json["lastResult"])
        owner = str(json["owner"]) ?? owner
        continuesAfterQuit = json["continuesAfterQuit"] as? Bool ?? false
    }

    /// 间隔文案。默认 24 小时直接说"每天检查一次"，比"每 24 小时"更像人话。
    var intervalText: String {
        if intervalSeconds == 86_400 { return "每天检查一次" }
        if intervalSeconds % 3600 == 0 {
            return "每 \(intervalSeconds / 3600) 小时"
        }
        if intervalSeconds % 60 == 0 {
            let minutes = intervalSeconds / 60
            return minutes == 1 ? "每分钟" : "每 \(minutes) 分钟"
        }
        return "每 \(intervalSeconds) 秒"
    }

    var nextTickText: String {
        guard let nextTickAt else { return enabled ? "等待第一轮" : "未开启" }
        if nextTickAt <= Date() { return "即将检查" }
        return "下次 \(Self.timeFormatter.string(from: nextTickAt))"
    }

    var lastTickText: String {
        guard let lastTickAt else { return "还没检查过" }
        return "上次 \(Self.timeFormatter.string(from: lastTickAt))"
    }

    static func date(fromRFC3339 raw: String?) -> Date? {
        guard let raw = str(raw) else { return nil }
        return ISO8601DateFormatter().date(from: raw)
    }

    static let timeFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter
    }()
}

/// 一条执行记录（`growth_runs`）。
struct GrowthRunRecord: Identifiable, Equatable {
    enum Action: String {
        case accept
        case claim

        var title: String {
            self == .accept ? "接单" : "领奖"
        }
    }

    enum Outcome: String {
        case ok
        case already
        case rejected
        case unknown
        case failed

        var title: String {
            switch self {
            case .ok: return "成功"
            case .already: return "已领过"
            case .rejected: return "被拒绝"
            case .unknown: return "未确认"
            case .failed: return "失败"
            }
        }

        /// 需要在界面上标红的：拒了、失败、没确认。`already` 不算问题。
        var isProblem: Bool {
            self == .rejected || self == .failed || self == .unknown
        }
    }

    let id: Int
    let accountId: String
    let taskCode: String
    let action: Action
    let outcome: Outcome
    let credit: Int?
    let energy: Int?
    let errorKind: String?
    let errorDetail: String?
    let executedAt: Date?

    init?(json: [String: Any]) {
        guard let taskCode = str(json["taskCode"]) else { return nil }
        id = (json["id"] as? NSNumber)?.intValue ?? 0
        accountId = str(json["accountId"]) ?? ""
        self.taskCode = taskCode
        action = Action(rawValue: str(json["action"]) ?? "") ?? .claim
        outcome = Outcome(rawValue: str(json["outcome"]) ?? "") ?? .unknown
        credit = (json["credit"] as? NSNumber)?.intValue
        energy = (json["energy"] as? NSNumber)?.intValue
        errorKind = str(json["errorKind"])
        errorDetail = str(json["errorDetail"])
        executedAt = GrowthAutoStatus.date(fromRFC3339: json["executedAt"] as? String)
    }

    /// 任务行上的一句话："上次 领奖 · 成功 +100" / "上次 接单 · 被拒绝：prerequisite…"。
    var summary: String {
        var parts = ["上次 \(action.title) · \(outcome.title)"]
        if let credit, credit > 0 { parts.append("+\(credit) 积分") }
        if let energy, energy > 0 { parts.append("+\(energy) 能量") }
        if outcome.isProblem, let errorDetail {
            parts.append(errorDetail)
        }
        return parts.joined(separator: " ")
    }

    var timeText: String {
        guard let executedAt else { return "" }
        return GrowthAutoStatus.timeFormatter.string(from: executedAt)
    }
}

/// 一个账号的执行记录汇总。
struct GrowthRuns: Equatable {
    var latestByTask: [String: GrowthRunRecord] = [:]
    var recent: [GrowthRunRecord] = []
    var grantedCredit = 0
    var grantedEnergy = 0

    init(json: [String: Any]?) {
        guard let json else { return }
        let latest = (json["latestByTask"] as? [[String: Any]] ?? [])
            .compactMap(GrowthRunRecord.init(json:))
        for record in latest {
            latestByTask[record.taskCode] = record
        }
        recent = (json["recent"] as? [[String: Any]] ?? []).compactMap(GrowthRunRecord.init(json:))
        grantedCredit = (json["grantedCredit"] as? NSNumber)?.intValue ?? 0
        grantedEnergy = (json["grantedEnergy"] as? NSNumber)?.intValue ?? 0
    }
}

/// `growth.auto.tick` 的返回。
struct GrowthTickResult: Equatable {
    /// `disabled` / `not_due` / `already_running` / nil（真的跑了）
    let skipped: String?
    let summary: String?
    let readOnly: Bool

    init(json: [String: Any]?) {
        guard let json else {
            skipped = nil
            summary = nil
            readOnly = true
            return
        }
        skipped = str(json["skipped"])
        summary = str(json["summary"])
        readOnly = json["readOnly"] as? Bool ?? true
    }

    var skippedText: String? {
        switch skipped {
        case "disabled": return "自动管理未开启"
        case "not_due": return "还没到检查时间"
        case "already_running": return "上一轮还在跑"
        default: return nil
        }
    }
}

/// 读一个"可能有也可能没有"的字符串。服务端用 "" 表示没有，这里统一成 nil。
///
/// 不复用 `TravelModels` 里的 `nonEmpty`：那个是 `fileprivate`，跨文件用不了；
/// 而在这个文件里再声明一次同名成员会和它冲突。
private func str(_ value: Any?) -> String? {
    guard let text = value as? String else { return nil }
    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    return trimmed.isEmpty ? nil : trimmed
}
