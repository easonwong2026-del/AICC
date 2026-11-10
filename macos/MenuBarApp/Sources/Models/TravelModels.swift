import Foundation

/// Buddy 旅行的视图模型。
///
/// 只做解析与派生，不碰进程和网络，所以能进 `swift test`。
/// 数据来自助手的 `travel.summary`（本地快照）与 `travel.refresh` / `travel.depart` /
/// `travel.claim`（会真的打接口）。字段口径与 `account-worker/src/storage/travel.rs` 对齐。
struct TravelStatus: Equatable {
    /// 与 Rust 侧 `Label::as_str()` 一一对应，改一个就要改另一边。
    enum Stage: String {
        case dispatchable
        case traveling
        case arrived
        case doneToday = "done_today"
        case noBuddy = "no_buddy"
        case unsupported
        case failed
        case unknown
    }

    let stage: Stage
    let locationName: String
    /// 预计到达时间（秒级时间戳换算），只有 traveling / arrived 才有。
    let arriveAt: Date?
    let claimed: Bool
    let rewardCredits: Int?
    let claimedAt: String?
    let dailyLimitReached: Bool
    let lastError: String?
    /// 检测到「别的客户端也在动这个账号」时的说明。
    let conflict: String?
    /// 退避截止时间。到点之前助手不会自动再查。
    let retryAfter: Date?
    let lastCheckedAt: String?

    init?(json: [String: Any]?) {
        guard let json else { return nil }
        let raw = json["label"] as? String ?? ""
        stage = Stage(rawValue: raw) ?? .unknown
        locationName = json["locationName"] as? String ?? ""
        arriveAt = TravelStatus.date(fromSeconds: json["arriveAt"])
        claimed = json["claimed"] as? Bool ?? false
        rewardCredits = (json["rewardCredits"] as? NSNumber)?.intValue
        claimedAt = json["claimedAt"] as? String
        dailyLimitReached = json["dailyLimitReached"] as? Bool ?? false
        lastError = (json["lastError"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
            .nonEmpty
        conflict = (json["conflict"] as? String)?.nonEmpty
        retryAfter = TravelStatus.date(fromRFC3339: json["retryAfter"] as? String)
        lastCheckedAt = json["lastCheckedAt"] as? String
    }

    // MARK: - 展示

    var stageText: String {
        switch stage {
        case .dispatchable: return "可派出"
        case .traveling: return "旅行中"
        case .arrived: return "已到达"
        case .doneToday: return "今日已完成"
        case .noBuddy: return "无 Buddy"
        case .unsupported: return "不支持"
        case .failed: return "失败"
        case .unknown: return "待检查"
        }
    }

    /// 能不能现在派。失败 / 无 Buddy / 不支持的账号按钮一律置灰。
    var canDepart: Bool { stage == .dispatchable }

    /// 能不能现在领：只有「已到达且没领过」才允许。
    var canClaim: Bool { stage == .arrived && !claimed }

    /// 目的地 + 状态说明。没有地点就不硬凑一句"暂无"。
    var destinationText: String {
        switch stage {
        case .unsupported: return "该档位没有成长中心"
        case .noBuddy: return "账号还没有 Buddy"
        case .failed: return lastError ?? "查询失败"
        default: break
        }
        guard !locationName.isEmpty else {
            return stage == .dispatchable ? "等待派出" : "—"
        }
        switch stage {
        case .arrived:
            // 已到达就别再显示一个过去的时间当"预计到达"，那会误导。
            return "\(locationName) · 已到达"
        case .traveling:
            guard let arriveAt else { return "\(locationName) · 旅行中" }
            if arriveAt <= Date() {
                return "\(locationName) · 即将到达"
            }
            return "\(locationName) · \(TravelStatus.timeFormatter.string(from: arriveAt)) 到达"
        case .doneToday:
            return "\(locationName) · 今日已完成"
        default:
            return locationName
        }
    }

    /// 账号名下那一行小字：优先领取结果，其次"可以领多少"，最后上次检查时间。
    var secondaryText: String {
        if let claimText { return claimText }
        if stage == .arrived, let rewardCredits, rewardCredits > 0 {
            return "可领取 +\(rewardCredits) 积分"
        }
        if let lastCheckedAt, let date = TravelStatus.date(fromRFC3339: lastCheckedAt) {
            return "上次检查 \(TravelStatus.timeFormatter.string(from: date))"
        }
        return "还没检查"
    }

    /// 领取结果：「+5 积分 · 11-10 08:12」。
    var claimText: String? {
        guard claimed else { return nil }
        var parts: [String] = []
        if let rewardCredits, rewardCredits > 0 {
            parts.append("+\(rewardCredits) 积分")
        } else {
            parts.append("已领取")
        }
        if let claimedAt, let date = TravelStatus.date(fromRFC3339: claimedAt) {
            parts.append(TravelStatus.shortFormatter.string(from: date))
        }
        return parts.joined(separator: " · ")
    }

    /// 需要提示用户的异常（冲突优先于普通错误）。
    var warningText: String? { conflict ?? lastError }

    // MARK: - 时间工具

    static let timeFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        // 旅行按北京时间结算，界面不跟着机器时区飘。
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "HH:mm"
        return formatter
    }()

    static let shortFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.timeZone = TimeZone(identifier: "Asia/Shanghai")
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter
    }()

    /// 助手给的是秒（可能混毫秒），统一按数值猜。
    static func date(fromSeconds value: Any?) -> Date? {
        guard let number = value as? NSNumber else { return nil }
        let raw = number.doubleValue
        guard raw > 0 else { return nil }
        return Date(timeIntervalSince1970: raw > 1_000_000_000_000 ? raw / 1000 : raw)
    }

    static func date(fromRFC3339 value: String?) -> Date? {
        guard let value, !value.isEmpty else { return nil }
        let parser = ISO8601DateFormatter()
        parser.formatOptions = [.withInternetDateTime]
        return parser.date(from: value)
    }
}

/// 三个会真的打接口的旅行动作。放在模型层，调用方不用记 RPC 方法名。
enum TravelMethod: String {
    /// 只查状态，不派出、不领取。
    case refresh = "travel.refresh"
    /// 派出（服务端每日上限仍然生效）。
    case depart = "travel.depart"
    /// 领取奖励（只有 arrived 会真的提交）。
    case claim = "travel.claim"
}

/// 一个账号 + 它的旅行快照 + 开关。
struct TravelAccountRow: Identifiable, Equatable {
    let accountId: String
    let nickname: String
    let uid: String
    let travelEnabled: Bool
    let authSource: String
    let status: TravelStatus?

    var id: String { accountId }

    /// 从 Magpie 借来的 access：令牌过期要回 Magpie 重新同步。
    var isBorrowed: Bool { authSource == "magpie" }

    init?(json: [String: Any]) {
        guard let accountId = json["accountId"] as? String, !accountId.isEmpty else { return nil }
        self.accountId = accountId
        let name = (json["nickname"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        nickname = name.isEmpty ? "未命名账号" : name
        uid = json["uid"] as? String ?? ""
        travelEnabled = json["travelEnabled"] as? Bool ?? false
        authSource = json["authSource"] as? String ?? "own"
        status = TravelStatus(json: json["travel"] as? [String: Any])
    }
}

/// 自动旅行的运行信息。**`runsOnlyWhileAppRunning` 必须如实显示**，
/// 不能让用户以为关掉 App 还在后台跑。
struct TravelAutoStatus: Equatable {
    let owner: String
    let runsOnlyWhileAppRunning: Bool
    let departIntervalSecs: Int
    let claimIntervalSecs: Int
    let enabledAccounts: Int
    let busy: Bool
    let lastTickAt: String?

    var intervalText: String {
        let depart = max(1, departIntervalSecs / 60)
        let claim = max(1, claimIntervalSecs / 60)
        return "派出 \(depart) 分钟 · 领取 \(claim) 分钟"
    }

    /// 没有 tick 记录就返回空串：宁可不提，也不编一句"还没跑过"塞进句子里。
    var lastTickText: String {
        guard let lastTickAt, let date = TravelStatus.date(fromRFC3339: lastTickAt) else {
            return ""
        }
        return "上次检查 \(TravelStatus.timeFormatter.string(from: date))"
    }

    init?(json: [String: Any]?) {
        guard let json, let owner = json["owner"] as? String, !owner.isEmpty else { return nil }
        self.owner = owner
        runsOnlyWhileAppRunning = json["runsOnlyWhileAppRunning"] as? Bool ?? false
        departIntervalSecs = (json["departIntervalSecs"] as? NSNumber)?.intValue ?? 1800
        claimIntervalSecs = (json["claimIntervalSecs"] as? NSNumber)?.intValue ?? 900
        enabledAccounts = (json["enabledAccounts"] as? NSNumber)?.intValue ?? 0
        busy = json["busy"] as? Bool ?? false
        lastTickAt = (json["lastTick"] as? [String: Any])?["at"] as? String
    }
}

/// `travel.summary` / `travel.refresh` 的合并视图。
struct TravelSummary: Equatable {
    let rows: [TravelAccountRow]
    let auto: TravelAutoStatus?

    /// 有账号开着自动旅行 —— 决定 App 启动时要不要把助手拉起来。
    var automationRunning: Bool {
        (auto?.enabledAccounts ?? 0) > 0 || rows.contains(where: \.travelEnabled)
    }

    static func parse(_ json: [String: Any]) -> TravelSummary {
        let accounts = (json["accounts"] as? [[String: Any]] ?? []).compactMap(TravelAccountRow.init)
        return TravelSummary(rows: accounts, auto: TravelAutoStatus(json: json["auto"] as? [String: Any]))
    }
}

/// `travel.depart` / `travel.claim` / `travel.refresh` 的返回：新快照 + 这次做了什么。
struct TravelSyncResult: Equatable {
    let status: TravelStatus?
    let events: [TravelEvent]

    struct TravelEvent: Equatable {
        let kind: String
        let status: String
        let message: String?
        let credits: Int?
    }

    /// 用事件拼一句人话，给界面底部的即时反馈用。
    var summaryText: String? {
        events.compactMap(\.message).first(where: { !$0.isEmpty })
    }

    static func parse(_ json: [String: Any]) -> TravelSyncResult {
        let events = (json["events"] as? [[String: Any]] ?? []).map { item in
            TravelEvent(
                kind: item["kind"] as? String ?? "",
                status: item["status"] as? String ?? "",
                message: item["message"] as? String,
                credits: (item["credits"] as? NSNumber)?.intValue
            )
        }
        return TravelSyncResult(
            status: TravelStatus(json: json["travel"] as? [String: Any]),
            events: events
        )
    }
}

private extension String {
    var nonEmpty: String? {
        let trimmed = trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }
}
