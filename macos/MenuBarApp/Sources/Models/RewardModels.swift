import Foundation

/// 奖励状态（PR #98）：`growth.rewards.status` 的 Swift 侧模型。
///
/// 后端那一层是**只读**的，这里的模型也只有一个方向：把 JSON 翻成界面能画的东西。
/// 三个约定必须守住，否则界面会说出后端刻意避免的错误结论：
///
/// 1. `awaitingVerification` 是 `Int?` —— `nil` 表示**本地账本读不到**，不是 0。
/// 2. `makeup.balance` 是 `Int?` —— `nil` 表示**余额没查到**，不是"没有卡"。
/// 3. `tier.state` 只认后端给的那五个值，其它一律按 `unknown` 显示，不能猜"可领"。

// MARK: - 档位

struct RewardTierState: Identifiable, Equatable {
    let tier: String
    let days: Int
    /// 服务端原话，可能为空。
    let status: String?
    /// `locked` / `claimable` / `claimed` / `ended` / `unknown`
    let state: String
    let claimable: Bool

    var id: String { tier }

    var title: String { "\(days) 天" }

    var stateLabel: String {
        switch state {
        case "claimable": return "可领取"
        case "claimed": return "已领取"
        case "locked": return "未达成"
        case "ended": return "已结束"
        default: return "状态未知"
        }
    }

    init(json: [String: Any]) {
        tier = json["tier"] as? String ?? ""
        days = (json["days"] as? NSNumber)?.intValue ?? 0
        status = (json["status"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        state = json["state"] as? String ?? "unknown"
        claimable = json["claimable"] as? Bool ?? false
    }

    init(tier: String, days: Int, status: String?, state: String, claimable: Bool) {
        self.tier = tier
        self.days = days
        self.status = status
        self.state = state
        self.claimable = claimable
    }
}

// MARK: - 补签

struct RewardMakeupState: Equatable {
    /// `nil` = 余额没查到。**不能当成 0**。
    let balance: Int?
    let max: Int?
    /// 服务端自己的"今天"。
    let today: String?
    /// 可补签日期，最近的在前。只包含服务端确认过的断签日。
    let candidates: [String]
    /// `no_broken_day` / `no_cards` / `balance_unknown` / 各种日历问题。
    let note: String?

    init(json: [String: Any]?) {
        let json = json ?? [:]
        balance = (json["balance"] as? NSNumber)?.intValue
        max = (json["max"] as? NSNumber)?.intValue
        today = (json["today"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        candidates = (json["candidates"] as? [String]) ?? []
        note = (json["note"] as? String).flatMap { $0.isEmpty ? nil : $0 }
    }

    init(balance: Int?, max: Int?, today: String?, candidates: [String], note: String?) {
        self.balance = balance
        self.max = max
        self.today = today
        self.candidates = candidates
        self.note = note
    }

    var balanceText: String {
        switch balance {
        case .some(let value):
            if let max { return "\(value) / \(max) 张" }
            return "\(value) 张"
        case .none:
            return "—"
        }
    }

    /// 一句人话。**余额未知和没有卡必须分开说**。
    var noteText: String? {
        switch note {
        case "no_broken_day": return "本月没有断签，不需要补签"
        case "no_cards": return "补签卡已用完"
        case "balance_unknown": return "补签卡余额没查到，稍后重试"
        case "calendar_missing": return "活跃日历没拿到，暂时无法判断断签"
        case "missing_today", "bad_today": return "服务端日期不可用，暂时无法判断断签"
        case "missing_launch", "bad_launch": return "缺少活动上线日期，无法判断补签范围"
        case "bad_makeup_dates": return "已补签记录格式异常，已停止计算"
        case "missing_cells", "bad_cell": return "活跃日历格式异常，已停止计算"
        case .some(let other): return other
        case .none: return nil
        }
    }

    var hasWorkToDo: Bool { !candidates.isEmpty }
}

// MARK: - 历史操作

struct RewardOperationRow: Identifiable, Equatable {
    let id: String
    let action: String
    let target: String
    let phase: String
    let attempt: Int
    let outcome: String?
    let detail: String?
    let createdAt: String
    let open: Bool

    var actionLabel: String {
        switch action {
        case "redeem": return "连登兑换"
        case "lottery_draw": return "抽奖"
        case "makeup_card": return "补签"
        default: return action
        }
    }

    var targetLabel: String {
        if target.isEmpty { return "" }
        if action == "redeem" { return target } // 7d / 14d / 28d
        return target
    }

    /// 阶段翻译成人话。**`unknown` 与 `frozen` 不能写成"失败"**：
    /// 前者是还没核验，后者是等人工，结论都还没定。
    var phaseLabel: String {
        switch phase {
        case "pending": return "待发送"
        case "ready": return "待重发"
        case "sent": return "已发出"
        case "unknown": return "待核验"
        case "frozen": return "已冻结"
        case "confirmed": return "已到账"
        case "rejected": return "未生效"
        case "failed": return "确认未执行"
        default: return phase
        }
    }

    // 只有 sent/unknown 才值得回查；pending/ready 还没发，frozen 等人工，不该被回查改写。
    var needsAttention: Bool { phase == "sent" || phase == "unknown" }

    init(json: [String: Any]) {
        id = json["id"] as? String ?? UUID().uuidString
        action = json["action"] as? String ?? ""
        target = json["target"] as? String ?? ""
        phase = json["phase"] as? String ?? "unknown"
        attempt = (json["attempt"] as? NSNumber)?.intValue ?? 0
        outcome = (json["outcome"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        detail = (json["detail"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        createdAt = json["createdAt"] as? String ?? ""
        open = json["open"] as? Bool ?? false
    }

    init(
        id: String,
        action: String,
        target: String,
        phase: String,
        attempt: Int,
        outcome: String?,
        detail: String?,
        createdAt: String,
        open: Bool
    ) {
        self.id = id
        self.action = action
        self.target = target
        self.phase = phase
        self.attempt = attempt
        self.outcome = outcome
        self.detail = detail
        self.createdAt = createdAt
        self.open = open
    }
}

// MARK: - 单账号

struct RewardAccountState: Identifiable, Equatable {
    let accountId: String
    let nickname: String
    /// `ok` / `partial` / `unauthorized` / `missing_credential` / `unsupported` / `failed`
    let state: String
    let streakDays: Int?
    let tiers: [RewardTierState]
    let lotteryChances: Int?
    let makeup: RewardMakeupState
    /// `nil` = 本地账本读不到。
    let awaitingVerification: Int?
    let recentOperations: [RewardOperationRow]
    let errors: [GrowthFieldError]
    let scannedAt: String
    let readOnly: Bool

    var id: String { accountId }

    init(json: [String: Any]) {
        accountId = json["accountId"] as? String ?? ""
        nickname = json["nickname"] as? String ?? ""
        state = json["state"] as? String ?? "failed"
        streakDays = (json["streakDays"] as? NSNumber)?.intValue
        tiers = (json["tiers"] as? [[String: Any]] ?? []).map { RewardTierState(json: $0) }
        lotteryChances = (json["lotteryChances"] as? NSNumber)?.intValue
        makeup = RewardMakeupState(json: json["makeup"] as? [String: Any])
        awaitingVerification = (json["awaitingVerification"] as? NSNumber)?.intValue
        recentOperations = (json["recentOperations"] as? [[String: Any]] ?? [])
            .map { RewardOperationRow(json: $0) }
        errors = (json["errors"] as? [[String: Any]] ?? []).map { GrowthFieldError(json: $0) }
        scannedAt = json["scannedAt"] as? String ?? ""
        readOnly = json["readOnly"] as? Bool ?? true
    }

    init(
        accountId: String,
        nickname: String,
        state: String,
        streakDays: Int?,
        tiers: [RewardTierState],
        lotteryChances: Int?,
        makeup: RewardMakeupState,
        awaitingVerification: Int?,
        recentOperations: [RewardOperationRow],
        errors: [GrowthFieldError],
        scannedAt: String,
        readOnly: Bool
    ) {
        self.accountId = accountId
        self.nickname = nickname
        self.state = state
        self.streakDays = streakDays
        self.tiers = tiers
        self.lotteryChances = lotteryChances
        self.makeup = makeup
        self.awaitingVerification = awaitingVerification
        self.recentOperations = recentOperations
        self.errors = errors
        self.scannedAt = scannedAt
        self.readOnly = readOnly
    }

    var claimableTiers: [RewardTierState] { tiers.filter(\.claimable) }

    /// 顶部摘要。**查不到的项不出现**，不用 0 占位。
    var summaryText: String {
        var parts: [String] = []
        if let streakDays, streakDays > 0 { parts.append("连登 \(streakDays) 天") }
        if let lotteryChances, lotteryChances > 0 { parts.append("抽奖 \(lotteryChances) 次") }
        // 只在确认有卡时才写：余额未知不该输出"补签卡 —"。
        if let balance = makeup.balance, balance > 0 { parts.append("补签卡 \(makeup.balanceText)") }
        if !claimableTiers.isEmpty { parts.append("\(claimableTiers.count) 档可领") }
        return parts.isEmpty ? "—" : parts.joined(separator: " · ")
    }

    var stateLabel: String {
        switch state {
        case "ok": return "已读取"
        case "partial": return "部分字段缺失"
        case "unauthorized": return "需要重新授权"
        case "missing_credential": return "没有可用凭据"
        case "unsupported": return "国际版不支持"
        default: return "读取失败"
        }
    }

    /// 账本里有几条还没核验。`nil`（读不到）与 0（没有）必须分开说。
    var awaitingText: String? {
        switch awaitingVerification {
        case .some(0), .none: return nil
        case .some(let count):
            return "有 \(count) 条操作还没核验完，AICC 会先回查再动"
        }
    }

    /// 连登为 0 时的解释。
    ///
    /// 实测（docs/evidence/）：服务端对三个账号返回的就是 `streak.days = 0`，
    /// 而它们的每日签到是成功的。两套判定：签到成功不等于成长中心活跃 +1。
    /// 这里必须把话说清楚，否则用户会以为"签到失败"或"界面坏了"。
    var streakNote: String? {
        guard streakDays == 0 else { return nil }
        // 只陈述服务端事实，不断言因果：签到是否计入成长活跃，目前没有证据。
        return "服务端返回连登 0 天；每日签到与成长中心活跃是否为同一计数，尚无确证"
    }

    /// 账本读不到时的提示。这与"没有未核验操作"是两件事。
    var ledgerUnreadableText: String? {
        awaitingVerification == nil ? "本地奖励账本读取失败，无法确认有没有未核验的操作" : nil
    }
}

// MARK: - 刷新失败（保留旧数据）

/// 「这次刷新失败了，但屏幕上还是上次的数据」这一条提示。
///
/// 存在的理由：`loadReward()` 失败时**不清空**缓存（清空会让人以为奖励没了），
/// 于是界面必须自己说清楚"现在看到的不是最新的"。把这段判定放进模型，
/// 是为了让它可单测 —— 放在 View 的 if 里就只能靠肉眼。
struct RewardRefreshNotice: Equatable {
    static let title = "刷新失败，以下为上次成功读取的数据"

    /// 失败原因（来自 store）。
    let reason: String
    /// 上次成功读取时间（服务端扫描时间）。空串表示不知道。
    let readAt: String

    /// 只有在"有缓存 + 本次失败"同时成立时才存在。
    static func make(state: RewardAccountState?, failure: String?) -> RewardRefreshNotice? {
        guard let state, let failure, !failure.isEmpty else { return nil }
        return RewardRefreshNotice(reason: failure, readAt: state.scannedAt)
    }

    /// 有没有一个可显示的时间。格式化交给界面层（模型层不依赖 Console）。
    var hasReadAt: Bool { !readAt.isEmpty }
}

/// 三个写操作。
enum RewardAction: String, Identifiable, CaseIterable {
    // rawValue 必须等于后端 `action` 字段（lottery_draw / redeem / makeup_card），
    // 否则 `RewardAction(rawValue: preview.action)` 得到 nil，确认按钮点了没反应。
    case draw = "lottery_draw"
    case redeem = "redeem"
    case makeup = "makeup_card"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .draw: return "抽奖"
        case .redeem: return "兑换连登奖励"
        case .makeup: return "补签"
        }
    }

    var cost: String {
        switch self {
        case .draw: return "消耗 1 次抽奖机会"
        case .redeem: return "消耗 1 个连登档位"
        case .makeup: return "消耗 1 张补签卡"
        }
    }
}

/// 实时预览：点按钮时现拉的，**不是缓存**。
struct RewardPreview: Equatable {
    let accountId: String
    let action: String
    let target: String?
    let balance: Int?
    let ready: Bool
    let reason: String?
    let cost: String
    let nickname: String

    init?(json: [String: Any]) {
        guard let preview = json["preview"] as? [String: Any] else { return nil }
        accountId = preview["accountId"] as? String ?? ""
        action = preview["action"] as? String ?? ""
        target = (preview["target"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        balance = (preview["balance"] as? NSNumber)?.intValue
        ready = preview["balancedReady"] as? Bool ?? false
        reason = preview["reason"] as? String
        cost = preview["cost"] as? String ?? ""
        nickname = preview["nickname"] as? String ?? ""
    }

    var balanceText: String { balance.map { "\($0)" } ?? "—" }
}

// MARK: - 写操作结果

/// 抽奖 / 兑换 / 补签 的一次结果。四种状态必须分开，**不能都叫"失败"**：
/// `pending` 是"可能已经执行、还没确认"，`frozen` 是"等人工"。
struct RewardActionResult: Equatable {
    /// 是否真的发了请求。
    let sent: Bool
    /// `succeeded` / `rejected` / `pending` / `frozen` / `failed` / `preview` / `blocked`
    let state: String
    let message: String
    let phase: String?

    init(json: [String: Any]) {
        sent = json["sent"] as? Bool ?? false
        state = json["state"] as? String
            ?? (json["decision"] as? String)
            ?? "failed"
        message = json["message"] as? String
            ?? (json["reason"] as? String)
            ?? (json["note"] as? String)
            ?? "—"
        phase = json["phase"] as? String
    }

    var needsAttention: Bool { state == "pending" || state == "frozen" }
}

// MARK: - 一次查询的结果

struct RewardStatusResult: Equatable {
    let accounts: [RewardAccountState]
    let queriedAt: String
    let readOnly: Bool

    init(json: [String: Any] = [:]) {
        accounts = (json["accounts"] as? [[String: Any]] ?? []).map { RewardAccountState(json: $0) }
        queriedAt = json["queriedAt"] as? String ?? ""
        readOnly = json["readOnly"] as? Bool ?? true
    }

    func account(_ id: String) -> RewardAccountState? {
        accounts.first { $0.accountId == id }
    }

    /// 给预览、视觉回归与单测用的固定数据。**不是真实响应**，值也不代表任何账号。
    ///
    /// `accountId` 可覆盖：视觉回归要把这份数据挂到样本账号上，单测则用默认值。
    static func mock(accountId: String = "mock-account") -> RewardStatusResult {
        RewardStatusResult(json: [
            "readOnly": true,
            "queriedAt": "2026-11-17T01:00:00Z",
            "accounts": [
                [
                    "accountId": accountId,
                    "nickname": "示例账号",
                    "state": "ok",
                    "streakDays": 16,
                    "lotteryChances": 1,
                    "awaitingVerification": 1,
                    "scannedAt": "2026-11-17T01:00:00Z",
                    "readOnly": true,
                    "tiers": [
                        ["tier": "7d", "days": 7, "status": "claimed", "state": "claimed", "claimable": false],
                        ["tier": "14d", "days": 14, "status": "available", "state": "claimable", "claimable": true],
                        ["tier": "28d", "days": 28, "status": "locked", "state": "locked", "claimable": false],
                    ],
                    "makeup": [
                        "balance": 2,
                        "max": 4,
                        "today": "2026-11-17",
                        "candidates": ["2026-11-16", "2026-11-15"],
                    ],
                    "recentOperations": [
                        [
                            "id": "op-1",
                            "action": "lottery_draw",
                            "target": "",
                            "phase": "unknown",
                            "attempt": 1,
                            "outcome": "indeterminate",
                            "createdAt": "2026-11-16T23:10:00Z",
                            "open": true,
                        ],
                        [
                            "id": "op-2",
                            "action": "redeem",
                            "target": "7d",
                            "phase": "confirmed",
                            "attempt": 1,
                            "outcome": "granted",
                            "createdAt": "2026-11-15T09:00:00Z",
                            "open": false,
                        ],
                    ],
                    "errors": [],
                ],
            ],
        ])
    }

    static let mock: RewardStatusResult = mock()
}
