import Foundation

/// WorkBuddy 每日签到的视图模型（PR #69）。
///
/// 字段口径与 `account-worker/src/checkin/mod.rs` 的 `status_row` /
/// `scheduler::auto_status` 一一对应，改一边就要改另一边。这里只做解析与派生文案，
/// 不发请求，所以能进 `swift test`。
///
/// 两条显示原则：
/// 1. **没有奖励数字就显示破折号**。`credits == nil` 是"不知道"，`0` 是"确实是零"。
///    服务端本来就只在 AICC 自己签到成功时才给数字 —— "已经签过"没有数字。
/// 2. **区分三种"已签到"**：AICC 这次签的（有积分）、AICC 今天早些时候签的、
///    服务端已签但不知道是谁签的（多半是 Magpie）。后两种不能算成 AICC 的战果。

// MARK: - 状态

enum CheckinState: String {
    case disabled
    case unchecked
    case claimed
    case already
    case unknown
    case unauthorized
    case noCredential = "no_credential"
    case failed
    case unsupported
    case busy
    case stopped
    /// 自动调度里的中间态：还没到计划时间。
    case waiting

    init(raw: String?) {
        self = CheckinState(rawValue: raw ?? "") ?? .unknown
    }

    var title: String {
        switch self {
        case .claimed: return "已签到"
        case .already: return "已签到"
        case .unchecked: return "未签到"
        case .unknown: return "状态未知"
        case .unauthorized: return "授权失效"
        case .noCredential: return "缺少授权"
        case .failed: return "检查失败"
        case .unsupported: return "不支持"
        case .busy: return "执行中"
        case .stopped: return "已中止"
        case .disabled: return "未开启自动签到"
        case .waiting: return "等待计划时间"
        }
    }

    /// 服务端意义上的"今天已签"。
    var isChecked: Bool {
        self == .claimed || self == .already
    }

    /// 需要人看一眼的状态。
    var isProblem: Bool {
        switch self {
        case .failed, .unknown, .unauthorized, .noCredential: return true
        default: return false
        }
    }
}

// MARK: - 账号行

struct CheckinAccountRow: Identifiable, Equatable {
    let accountId: String
    var nickname: String
    var enabled: Bool
    var state: CheckinState
    var serverCheckedIn: Bool?
    var credits: Int?
    /// 本地记录的时间（RFC3339 解析失败就是 nil，不猜）。
    var checkedAt: Date?
    var claimedByAiccToday: Bool
    var streakDays: Int?
    var dailyCredit: Int?
    var errorKind: String?
    var errorDetail: String?
    var running: Bool
    /// 自动调度给这个账号算的下次执行时间。
    var nextRunAt: Date?

    var id: String { accountId }

    init?(json: [String: Any]) {
        guard let accountId = json["accountId"] as? String else { return nil }
        self.accountId = accountId
        self.nickname = json["nickname"] as? String ?? ""
        self.enabled = json["enabled"] as? Bool ?? false
        self.state = CheckinState(raw: json["state"] as? String)
        self.serverCheckedIn = json["serverCheckedIn"] as? Bool
        self.credits = CheckinJSON.int(json["credits"])
        self.checkedAt = CheckinJSON.date(json["localCheckedAt"])
        self.claimedByAiccToday = json["claimedByAiccToday"] as? Bool ?? false
        self.streakDays = CheckinJSON.int(json["streakDays"])
        self.dailyCredit = CheckinJSON.int(json["dailyCredit"])
        self.errorKind = json["errorKind"] as? String
        self.errorDetail = json["errorDetail"] as? String
        self.running = json["running"] as? Bool ?? false
        self.nextRunAt = CheckinJSON.date(json["nextRunAt"])
    }

    init(
        accountId: String,
        nickname: String,
        enabled: Bool,
        state: CheckinState,
        credits: Int? = nil,
        claimedByAiccToday: Bool = false,
        errorKind: String? = nil,
        running: Bool = false
    ) {
        self.accountId = accountId
        self.nickname = nickname
        self.enabled = enabled
        self.state = state
        self.serverCheckedIn = state.isChecked ? true : nil
        self.credits = credits
        self.checkedAt = nil
        self.claimedByAiccToday = claimedByAiccToday
        self.streakDays = nil
        self.dailyCredit = nil
        self.errorKind = errorKind
        self.errorDetail = nil
        self.running = running
    }

    /// 奖励那一列。**不知道就显示破折号**，不默认 100。
    var creditsText: String {
        guard let credits else { return "—" }
        return "+\(credits)"
    }

    /// 这一行的补充说明：谁签的、为什么失败。
    var detailText: String {
        if running { return "正在处理" }
        if let errorDetail, state.isProblem || state == .unknown {
            return errorDetail
        }
        switch state {
        case .claimed:
            return "AICC 已提交并回查确认"
        case .already:
            if claimedByAiccToday {
                return "今天早些时候已由 AICC 签到，本次未重复提交"
            }
            return "服务端已签到，本次未提交（可能由 Magpie 或官方客户端完成）"
        case .unchecked:
            return "服务端确认今天还没签"
        case .unknown:
            return "服务端没有给出明确状态，已停止写入"
        case .unauthorized:
            return "授权失效，请重新登录或从 Magpie 同步"
        case .noCredential:
            return "账号没有可用授权"
        case .disabled:
            return "打开右侧开关后才会自动签到"
        case .busy:
            return "同一账号已有一次执行在跑"
        case .stopped:
            return "提交前检测到开关已关闭"
        case .unsupported:
            return "该平台暂不支持签到"
        case .waiting:
            return "还没到计划时间"
        case .failed:
            return errorDetail ?? "服务端拒绝了这次请求"
        }
    }

    /// 是否要显示"未完成"的强调色。
    var needsAttention: Bool {
        state.isProblem || (state == .unchecked && enabled)
    }
}

// MARK: - 全部账号

struct CheckinCounts: Equatable {
    var accounts = 0
    var checked = 0
    var unchecked = 0
    var enabled = 0
    var credit = 0

    init() {}

    init(json: [String: Any]?) {
        guard let json else { return }
        accounts = CheckinJSON.int(json["accounts"]) ?? 0
        checked = CheckinJSON.int(json["checked"]) ?? 0
        unchecked = CheckinJSON.int(json["unchecked"]) ?? 0
        enabled = CheckinJSON.int(json["enabled"]) ?? 0
        credit = CheckinJSON.int(json["credit"]) ?? 0
    }
}

struct CheckinFleetStatus: Equatable {
    var today = ""
    var rows: [CheckinAccountRow] = []
    var counts = CheckinCounts()
    var checkedAt: Date?
    var cached = false

    static let empty = CheckinFleetStatus()

    init() {}

    init(json: [String: Any]?) {
        guard let json else { return }
        today = json["today"] as? String ?? ""
        cached = json["cached"] as? Bool ?? false
        checkedAt = CheckinJSON.date(json["checkedAt"])
        counts = CheckinCounts(json: json["counts"] as? [String: Any])
        let list = json["accounts"] as? [[String: Any]] ?? []
        rows = list.compactMap(CheckinAccountRow.init(json:))
    }

    var headline: String {
        "今日 \(counts.checked)/\(counts.accounts) 已签到"
    }

    var enabledNote: String {
        counts.enabled == 0 ? "未开启自动签到" : "自动开启 \(counts.enabled) 个账号"
    }

    /// 只有 AICC 自己确认到账的积分才展示成"本次新增"。
    var creditNote: String? {
        counts.credit > 0 ? "今日 AICC 签到 +\(counts.credit)" : nil
    }

    func row(_ accountId: String) -> CheckinAccountRow? {
        rows.first { $0.accountId == accountId }
    }

    /// 需要处理的排在前面：未签到且开着自动 → 有问题 → 已签到。
    ///
    /// 稳定排序（同名按 accountId），否则每次刷新行顺序都会跳。
    var sortedRows: [CheckinAccountRow] {
        rows.sorted { left, right in
            let leftRank = left.needsAttention ? 0 : (left.state.isChecked ? 2 : 1)
            let rightRank = right.needsAttention ? 0 : (right.state.isChecked ? 2 : 1)
            if leftRank != rightRank { return leftRank < rightRank }
            if left.nickname != right.nickname { return left.nickname < right.nickname }
            return left.accountId < right.accountId
        }
    }

    /// 服务端确认未签到、可以提交的账号。
    var pendingRows: [CheckinAccountRow] {
        rows.filter { $0.state == .unchecked }
    }
}

// MARK: - 调度状态

struct CheckinAutoStatus: Equatable {
    var enabled = false
    var enabledAccounts = 0
    var totalAccounts = 0
    var planTime = "09:00"
    /// 时区说明由 worker 给（"UTC+8（北京时间）"），界面直接显示，不在这里编造。
    var timezone = "UTC+8"
    var lastTickAt: Date?
    var nextRunAt: Date?
    var lastResult: String?
    var running = false
    var continuesAfterQuit = false

    static let unknown = CheckinAutoStatus()

    init() {}

    init(json: [String: Any]?) {
        guard let json else { return }
        enabled = json["enabled"] as? Bool ?? false
        enabledAccounts = CheckinJSON.int(json["enabledAccounts"]) ?? 0
        totalAccounts = CheckinJSON.int(json["totalAccounts"]) ?? 0
        planTime = json["planTime"] as? String ?? "09:00"
        timezone = json["timezone"] as? String ?? "UTC+8"
        lastTickAt = CheckinJSON.date(json["lastTickAt"])
        nextRunAt = CheckinJSON.date(json["nextRunAt"])
        lastResult = json["lastResult"] as? String
        running = json["running"] as? Bool ?? false
        continuesAfterQuit = json["continuesAfterQuit"] as? Bool ?? false
    }

    /// "明天 09:00" 这类说法。`now` 由调用方给，测试才能钉住。
    func nextRunText(now: Date = Date()) -> String {
        guard let nextRunAt else {
            return enabledAccounts == 0 ? "未开启" : "等待下一次检查"
        }
        return CheckinClock.describe(nextRunAt, now: now)
    }

    var lastTickText: String? {
        lastTickAt.map { CheckinClock.describe($0) }
    }

    /// 界面必须如实写清楚"App 退出就没人跑了"。
    var ownershipNote: String {
        "自动签到由 AICC 的账号助手进程执行，AICC 退出即停止。"
    }
}

// MARK: - 执行结果

struct CheckinTickResult: Equatable {
    var summary: String?
    var skipped: String?
    var ranAccounts = 0

    init(json: [String: Any]?) {
        guard let json else { return }
        summary = json["summary"] as? String
        skipped = json["skipped"] as? String
        ranAccounts = CheckinJSON.int(json["ranAccounts"]) ?? 0
    }

    /// 被跳过时给一句人话，而不是把 `no_accounts_enabled` 直接甩到界面上。
    var skippedText: String? {
        switch skipped {
        case "no_accounts_enabled": return "没有账号开启自动签到"
        case "already_running": return "上一轮还在跑，稍后再试"
        case "not_due": return "还没到计划时间"
        case nil: return nil
        default: return skipped
        }
    }
}

// MARK: - 历史记录

struct CheckinHistoryRow: Identifiable, Equatable {
    let localDate: String
    var status: String
    var credits: Int?
    var checkedAt: Date?
    var source: String?
    var errorKind: String?

    var id: String { "\(localDate)-\(status)" }

    init?(json: [String: Any]) {
        guard let localDate = json["localDate"] as? String else { return nil }
        self.localDate = localDate
        self.status = json["status"] as? String ?? ""
        self.credits = CheckinJSON.int(json["credits"])
        self.checkedAt = CheckinJSON.date(json["checkedAt"])
        self.source = json["executionSource"] as? String
        self.errorKind = json["errorKind"] as? String
    }

    var statusText: String {
        switch status {
        case "claimed": return "已签到"
        case "already": return "已签到（未提交）"
        case "failed": return "失败"
        case "skipped": return "跳过"
        default: return status
        }
    }

    var sourceText: String? {
        switch source {
        case "auto": return "自动"
        case "manual": return "手动"
        default: return nil
        }
    }
}

// MARK: - 解析与时间

enum CheckinJSON {
    /// 数字可能是 Int、Double 或字符串 —— 服务端三种都给过。
    static func int(_ value: Any?) -> Int? {
        switch value {
        case let number as Int: return number
        case let number as Double: return Int(number)
        case let text as String: return Int(text.trimmingCharacters(in: .whitespaces))
        case let number as NSNumber: return number.intValue
        default: return nil
        }
    }

    /// RFC3339（带毫秒 / 不带都认）。解析不出来就是 nil，不伪造时间。
    static func date(_ value: Any?) -> Date? {
        guard let text = value as? String, !text.isEmpty else { return nil }
        return CheckinClock.parse(text)
    }
}

enum CheckinClock {
    /// 按**北京时间**显示，与签到日期口径一致；不看机器本地时区。
    static let timezone = TimeZone(secondsFromGMT: 8 * 3600) ?? .current

    static func parse(_ text: String) -> Date? {
        let withFraction = ISO8601DateFormatter()
        withFraction.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = withFraction.date(from: text) { return date }
        let plain = ISO8601DateFormatter()
        plain.formatOptions = [.withInternetDateTime]
        return plain.date(from: text)
    }

    private static func formatter(_ format: String) -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_CN")
        formatter.timeZone = timezone
        formatter.dateFormat = format
        return formatter
    }

    static func time(_ date: Date) -> String {
        formatter("HH:mm").string(from: date)
    }

    static func dayAndTime(_ date: Date) -> String {
        formatter("MM-dd HH:mm").string(from: date)
    }

    /// "今天 09:12" / "明天 09:00" / "11-15 09:00"。
    static func describe(_ date: Date, now: Date = Date()) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timezone
        let target = calendar.startOfDay(for: date)
        let today = calendar.startOfDay(for: now)
        let days = calendar.dateComponents([.day], from: today, to: target).day ?? 0
        switch days {
        case 0: return "今天 \(time(date))"
        case 1: return "明天 \(time(date))"
        case -1: return "昨天 \(time(date))"
        default: return dayAndTime(date)
        }
    }

    /// 北京时间的今天（`YYYY-MM-DD`）。界面拿它和 `records.localDate` 对齐。
    static func beijingDay(_ date: Date = Date()) -> String {
        formatter("yyyy-MM-dd").string(from: date)
    }
}
