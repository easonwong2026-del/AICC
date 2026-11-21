import Foundation

// MARK: - 任务中心的两个模式

/// 签到任务页现在叫任务中心，内部有两个模式。
/// 这是页面内导航，不是第五个一级页面。
enum TaskCenterMode: String, CaseIterable, Identifiable {
    case today
    case automation

    var id: String { rawValue }

    var title: String {
        switch self {
        case .today: return "今日任务"
        case .automation: return "自动计划"
        }
    }
}

// MARK: - 五类自动任务

enum AutomationPlanKind: String, CaseIterable, Identifiable {
    case checkin
    case growth
    case travel
    case credits
    case rotation

    var id: String { rawValue }

    var title: String {
        switch self {
        case .checkin: return "每日签到"
        case .growth: return "成长中心"
        case .travel: return "Buddy 旅行"
        case .credits: return "Qoder Credits"
        case .rotation: return "WorkBuddy 客户端轮换"
        }
    }

    var symbol: String {
        switch self {
        case .checkin: return "calendar.badge.checkmark"
        case .growth: return "sparkles"
        case .travel: return "airplane"
        case .credits: return "circle.dotted.circle"
        case .rotation: return "arrow.triangle.2.circlepath"
        }
    }
}

/// 一个自动计划的展示状态。
///
/// `unknown` / `loading` / `failed` 与 `off` 必须分开：读不到状态时显示
/// "未开启"会让用户以为计划被关掉了。
enum AutomationState: Equatable {
    case unknown
    case loading
    case off
    case on
    case running
    /// 需要人处理，附带一句原因（例如"需要恢复"）。
    case attention(String)
    case failed(String)

    var statusText: String {
        switch self {
        case .unknown: return "未知"
        case .loading: return "读取中"
        case .off: return "未开启"
        case .on: return "已开启"
        case .running: return "正在执行"
        case .attention(let text): return text
        case .failed: return "读取失败"
        }
    }

    var tone: SettingsTone {
        switch self {
        case .unknown, .loading, .off: return .neutral
        case .on: return .ok
        case .running: return .accent
        case .attention: return .warning
        case .failed: return .error
        }
    }

    var isOnOrRunning: Bool {
        self == .on || self == .running
    }
}

/// 自动计划中心的一行。真实数据全部来自 store，不在这里编造。
struct AutomationPlanRow: Identifiable, Equatable {
    let kind: AutomationPlanKind
    let state: AutomationState
    /// 一行摘要：`5 个账号已开启` / `已开启 · 每天检查一次` / `暂无支持账号`。
    let summary: String
    /// 只有后端真的给了下一次执行时间才显示。
    var nextRun: String?

    var id: String { kind.rawValue }
    var title: String { kind.title }
    var statusText: String { state.statusText }
    var tone: SettingsTone { state.tone }
}

struct AutomationInputs {
    var checkinAuto: CheckinAutoStatus?
    var checkinLoading = false
    var checkinError: String?
    var growthAuto: GrowthAutoStatus?
    var growthLoading = false
    var growthError: String?
    var travelAuto: TravelAutoStatus?
    var travelLoading = false
    var travelError: String?
    var creditsAuto: [QoderCreditsAutoItem] = []
    var creditsLoading = false
    var creditsError: String?
    /// 空数组既可能是"没有支持账号"，也可能是"还没读"。
    var creditsLoaded = false
    var rotation: WorkbuddyRotationStatus?
    var rotationLoading = false
    var rotationError: String?
}

enum AutomationPlanBuilder {
    static func rows(_ inputs: AutomationInputs, now: Date = Date()) -> [AutomationPlanRow] {
        [
            checkin(inputs, now: now),
            growth(inputs, now: now),
            travel(inputs, now: now),
            credits(inputs, now: now),
            rotation(inputs, now: now),
        ]
    }

    // MARK: 签到

    static func checkin(_ inputs: AutomationInputs, now: Date) -> AutomationPlanRow {
        let state: AutomationState
        var summary = "未开启"
        var nextRun: String?

        if inputs.checkinLoading && inputs.checkinAuto == nil {
            state = .loading
            summary = "读取中"
        } else if let error = inputs.checkinError, inputs.checkinAuto == nil {
            state = .failed(error)
            summary = "读取失败"
        } else if let auto = inputs.checkinAuto {
            if auto.running {
                state = .running
            } else if auto.enabled && auto.enabledAccounts > 0 {
                state = .on
            } else {
                state = .off
            }
            if auto.enabled {
                summary = auto.enabledAccounts > 0
                    ? "\(auto.enabledAccounts)/\(auto.totalAccounts) 个账号已开启"
                    : "计划已开启，没有账号启用"
            } else {
                summary = "未开启"
            }
            if let next = auto.nextRunAt {
                nextRun = AutomationTime.short(next)
            }
        } else {
            state = .unknown
            summary = "未知"
        }

        return AutomationPlanRow(kind: .checkin, state: state, summary: summary, nextRun: nextRun)
    }

    // MARK: 成长

    static func growth(_ inputs: AutomationInputs, now: Date) -> AutomationPlanRow {
        let state: AutomationState
        var summary = "未开启"
        var nextRun: String?

        if inputs.growthLoading && inputs.growthAuto == nil {
            state = .loading
            summary = "读取中"
        } else if let error = inputs.growthError, inputs.growthAuto == nil {
            state = .failed(error)
            summary = "读取失败"
        } else if let auto = inputs.growthAuto {
            if auto.running {
                state = .running
                summary = "正在执行"
            } else if auto.enabled {
                state = .on
                summary = "已开启 · \(auto.intervalText)"
            } else {
                state = .off
            }
            if let next = auto.nextTickAt {
                nextRun = AutomationTime.short(next)
            }
        } else {
            state = .unknown
            summary = "未知"
        }

        return AutomationPlanRow(kind: .growth, state: state, summary: summary, nextRun: nextRun)
    }

    // MARK: 旅行

    static func travel(_ inputs: AutomationInputs, now: Date) -> AutomationPlanRow {
        let state: AutomationState
        var summary = "未开启"

        if inputs.travelLoading && inputs.travelAuto == nil {
            state = .loading
            summary = "读取中"
        } else if let error = inputs.travelError, inputs.travelAuto == nil {
            state = .failed(error)
            summary = "读取失败"
        } else if let auto = inputs.travelAuto {
            if auto.enabledAccounts > 0 {
                state = auto.busy ? .running : .on
                summary = "\(auto.enabledAccounts) 个账号已开启"
            } else {
                state = .off
            }
        } else {
            state = .unknown
            summary = "未知"
        }

        return AutomationPlanRow(kind: .travel, state: state, summary: summary)
    }

    // MARK: Credits

    static func credits(_ inputs: AutomationInputs, now: Date) -> AutomationPlanRow {
        let state: AutomationState
        var summary = "未开启"

        if inputs.creditsLoading && inputs.creditsAuto.isEmpty {
            state = .loading
            summary = "读取中"
        } else if let error = inputs.creditsError, inputs.creditsAuto.isEmpty {
            state = .failed(error)
            summary = "读取失败"
        } else if !inputs.creditsLoaded && inputs.creditsAuto.isEmpty {
            state = .unknown
            summary = "未知"
        } else {
            let supported = inputs.creditsAuto.filter(\.supported)
            let enabled = inputs.creditsAuto.filter { $0.supported && $0.autoEnabled }
            if supported.isEmpty {
                state = .off
                summary = "暂无支持账号"
            } else if enabled.isEmpty {
                state = .off
                summary = "未开启"
            } else {
                state = .on
                summary = "\(enabled.count)/\(supported.count) 个账号已开启"
            }
        }

        return AutomationPlanRow(kind: .credits, state: state, summary: summary)
    }

    // MARK: 轮换

    static func rotation(_ inputs: AutomationInputs, now: Date) -> AutomationPlanRow {
        let state: AutomationState
        var summary = "未配置"
        var nextRun: String?

        if inputs.rotationLoading && inputs.rotation == nil {
            state = .loading
            summary = "读取中"
        } else if let error = inputs.rotationError, inputs.rotation == nil {
            state = .failed(error)
            summary = "读取失败"
        } else if let status = inputs.rotation {
            let overview = RotationOverview.make(status: status, now: now)
            state = overview.state
            summary = overview.compactSummary
            nextRun = overview.nextRunText
        } else {
            state = .unknown
            summary = "未知"
        }

        return AutomationPlanRow(kind: .rotation, state: state, summary: summary, nextRun: nextRun)
    }
}

// MARK: - 轮换概览

/// 自动轮换的日常视图：打开就回答"开没开、跑几个账号、什么时候跑、上次怎样、有没有要处理"。
struct RotationOverview: Equatable {
    let state: AutomationState
    let accountCount: Int
    let scheduleText: String
    let nextRunText: String?
    /// 正在跑的这一轮：`停留中 · 等待恢复`。
    let currentText: String?
    let lastResultText: String?
    /// 恢复异常。非空时必须出现在最上面。
    let recoveryText: String?
    let liveText: String
    /// 是否已经在服务端配置过计划（有账号且星期非空）。
    let configured: Bool

    var needsRecovery: Bool { recoveryText != nil }

    /// 自动计划中心那一行用的短摘要。
    var compactSummary: String {
        if !configured { return "未配置" }
        switch state {
        case .on, .running:
            return "已开启 · \(accountCount) 个账号"
        case .attention:
            return "已开启 · 需要恢复"
        default:
            return "未开启 · \(accountCount) 个账号"
        }
    }

    static func make(
        status: WorkbuddyRotationStatus?,
        loading: Bool = false,
        error: String? = nil,
        now: Date = Date()
    ) -> RotationOverview {
        guard let status else {
            let state: AutomationState
            if loading { state = .loading }
            else if let error { state = .failed(error) }
            else { state = .unknown }
            return RotationOverview(
                state: state,
                accountCount: 0,
                scheduleText: "—",
                nextRunText: nil,
                currentText: nil,
                lastResultText: nil,
                recoveryText: nil,
                liveText: "—",
                configured: false
            )
        }

        let plan = status.plan
        let configured = !plan.accountIds.isEmpty && !plan.weekdays.isEmpty
        let recovery = status.attentionRun?.needsAttention == true
            ? (status.current?.restoreText.isEmpty == false ? status.current?.restoreText : status.lastRun?.restoreText)
            : nil
        let recoveryText = recovery ?? (status.attentionRun != nil
            ? "自动恢复没有完成，需要人工确认。"
            : nil)

        let state: AutomationState
        if recoveryText != nil {
            state = .attention("需要恢复")
        } else if let current = status.current, !current.isTerminal {
            state = .running
        } else if plan.enabled {
            state = .on
        } else {
            state = .off
        }

        // 只在有具体时间时给"下次"：窗口内已经在跑，状态行已经说过了。
        let next: String?
        if !status.schedule.inWindow, let date = status.schedule.nextRunAt, configured {
            next = AutomationTime.short(date)
        } else {
            next = nil
        }

        let currentText: String? = {
            guard let current = status.current, !current.isTerminal else { return nil }
            return "\(current.stateText) · \(current.restoreText)"
        }()
        let last = status.lastRun.map { run in
            "\(run.planDate) · \(run.stateText) · \(run.restoreText)"
        }

        return RotationOverview(
            state: state,
            accountCount: plan.accountIds.count,
            scheduleText: configured
                ? "\(AutomationScheduleText.weekdays(plan.weekdays)) \(plan.windowText) · 每账号 \(plan.dwellText)"
                : "还没有设置自动轮换",
            nextRunText: next,
            currentText: currentText,
            lastResultText: last,
            recoveryText: recoveryText,
            liveText: status.liveLoggedIn
                ? (status.liveUid.map(WorkbuddyLiveLogin.shortUID) ?? "已登录")
                : "未登录",
            configured: configured
        )
    }
}

// MARK: - 轮换草稿

/// 编辑中的计划。单独的纯值，服务端刷新只更新 `WorkbuddyRotationPlan`，
/// 不会覆盖这里 —— 除非用户没有未保存的修改。
struct RotationDraft: Equatable {
    var weekdays: Set<Int>
    var startMinutes: Int
    var endMinutes: Int
    var accountIds: [String]
    var dwellSeconds: Int
    var timezone: String

    static let dwellChoices = [300, 600, 900, 1800, 2700, 3600, 5400, 7200]

    init(plan: WorkbuddyRotationPlan) {
        weekdays = Set(plan.weekdays)
        startMinutes = plan.startMinutes
        endMinutes = plan.endMinutes
        accountIds = plan.accountIds
        dwellSeconds = plan.dwellSeconds
        timezone = plan.timezone
    }

    var crossesMidnight: Bool { endMinutes <= startMinutes }

    var windowText: String {
        let start = WorkbuddyRotationPlan.clockText(startMinutes)
        let end = WorkbuddyRotationPlan.clockText(endMinutes)
        return crossesMidnight ? "\(start) → 次日 \(end)" : "\(start) → \(end)"
    }

    var dwellText: String {
        dwellSeconds % 3600 == 0 ? "\(dwellSeconds / 3600) 小时" : "\(dwellSeconds / 60) 分钟"
    }

    /// 保存条件：至少一天、至少一个账号。
    var isValid: Bool { !weekdays.isEmpty && !accountIds.isEmpty }

    func isDirty(against plan: WorkbuddyRotationPlan) -> Bool {
        Set(plan.weekdays) != weekdays
            || plan.startMinutes != startMinutes
            || plan.endMinutes != endMinutes
            || plan.accountIds != accountIds
            || plan.dwellSeconds != dwellSeconds
            || plan.timezone != timezone
    }

    /// 交给 `saveWorkbuddyRotation` 的字段。键名必须与既有 RPC 一致。
    var params: [String: Any] {
        [
            "weekdays": weekdays.sorted(),
            "startMinutes": startMinutes,
            "endMinutes": endMinutes,
            "accountIds": accountIds,
            "dwellSeconds": dwellSeconds,
            "timezone": timezone,
        ]
    }

    mutating func toggleAccount(_ id: String, on: Bool) {
        if on {
            if !accountIds.contains(id) { accountIds.append(id) }
        } else {
            accountIds.removeAll { $0 == id }
        }
    }

    mutating func moveAccount(_ id: String, by offset: Int) {
        guard let index = accountIds.firstIndex(of: id) else { return }
        let target = index + offset
        guard accountIds.indices.contains(target) else { return }
        accountIds.swapAt(index, target)
    }
}

// MARK: - 配置步骤

enum RotationSetupStep: Int, CaseIterable, Identifiable {
    case accounts
    case schedule
    case preview

    var id: Int { rawValue }

    var title: String {
        switch self {
        case .accounts: return "参与账号"
        case .schedule: return "执行时间"
        case .preview: return "预演与保存"
        }
    }

    var next: RotationSetupStep? {
        RotationSetupStep(rawValue: rawValue + 1)
    }

    var previous: RotationSetupStep? {
        RotationSetupStep(rawValue: rawValue - 1)
    }
}

// MARK: - 保存并开启

enum RotationSaveStep: Equatable {
    case save
    case enable
    case reload
}

enum RotationSaveResult: Equatable {
    /// 只保存，没有启用。
    case savedOnly
    /// 保存 + 启用 + 回读都成功。
    case enabled
    case saveFailed
    /// 启用失败。调用方必须显示失败，并展示回读后的真实状态。
    case enableFailed
}

/// 「保存并开启」的执行顺序。顺序本身是被测对象：
/// 先保存 → 确认成功 → 再启用 → 回读服务端状态。
enum RotationSaveFlow {
    static func steps(dirty: Bool, enable: Bool) -> [RotationSaveStep] {
        if !enable {
            return dirty ? [.save] : []
        }
        return dirty ? [.save, .enable, .reload] : [.enable, .reload]
    }

    @discardableResult
    static func run(
        dirty: Bool,
        enable: Bool,
        save: () async -> Bool,
        setEnabled: (Bool) async -> Bool,
        reload: () async -> Void
    ) async -> RotationSaveResult {
        for step in steps(dirty: dirty, enable: enable) {
            switch step {
            case .save:
                // 保存失败就停：不能拿旧计划去开启。
                guard await save() else { return .saveFailed }
            case .enable:
                let ok = await setEnabled(true)
                guard ok else {
                    // 失败必须回读真实状态，界面不能停在"已开启"。
                    await reload()
                    return .enableFailed
                }
            case .reload:
                await reload()
            }
        }
        return enable ? .enabled : .savedOnly
    }
}

// MARK: - 文案

enum AutomationTime {
    static func short(_ date: Date?) -> String? {
        guard let date else { return nil }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "zh_Hans_CN")
        formatter.dateFormat = "MM-dd HH:mm"
        return formatter.string(from: date)
    }
}

enum AutomationScheduleText {
    static let names = ["周一", "周二", "周三", "周四", "周五", "周六", "周日"]

    /// `[1,2,3,4,5]` → `周一至周五`；不连续时逐个列出。
    static func weekdays(_ days: [Int]) -> String {
        let sorted = Array(Set(days)).sorted()
        guard !sorted.isEmpty else { return "未选星期" }
        if sorted.count == 7 { return "每天" }
        if sorted.count > 2,
           let first = sorted.first, let last = sorted.last,
           last - first == sorted.count - 1 {
            return "\(name(first))至\(name(last))"
        }
        return sorted.map { name($0) }.joined(separator: "、")
    }

    static func name(_ day: Int) -> String {
        guard (1...7).contains(day) else { return "周?" }
        return names[day - 1]
    }
}
