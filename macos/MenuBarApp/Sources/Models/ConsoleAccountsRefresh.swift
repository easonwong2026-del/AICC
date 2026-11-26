import Foundation

/// 账号列表刷新的触发来源。
///
/// 所有入口（冷启动、手动点刷新、切回前台、切换页面、前台停留 10 分钟定时）
/// 全部收敛到 `ConsoleAccountsRefreshCoordinator`，由同一份状态机负责：
/// - 区分**成功时间**、**尝试时间**与**刷新错误**（失败绝不推进成功时间）；
/// - 并发防重与手动刷新抢占旧静默请求；
/// - 失败指数退避（避免 worker 异常时高频打请求）；
/// - 代次保护（旧慢请求晚于新请求返回时直接丢弃，绝不覆盖新结果）。
enum ConsoleAccountsRefreshTrigger: Equatable {
    /// 冷启动或窗口首次挂载（`loadIfNeeded`）。
    case initialLoad
    /// 用户主动触发（页头刷新按钮、`⌘R`、重试按钮，或登录/导入/删号后的主动刷新）。
    case manual
    /// 主控制台窗口回到前台 / 激活。
    case windowActivated
    /// 切换到总览或 AI 账号页。
    case pageEntered
    /// 主窗口持续停留在前台且位于总览 / AI 账号页满 10 分钟后的自动只读刷新。
    case scheduled

    /// 是否为自动/静默类触发（非用户主动点刷新）。
    var isAutomatic: Bool {
        self != .manual
    }

    /// 窗口激活、切页、定时刷新不能抢跑冷启动；冷启动与手动刷新允许在首次加载前执行。
    var requiresInitialLoadAttempt: Bool {
        switch self {
        case .windowActivated, .pageEntered, .scheduled:
            return true
        case .initialLoad, .manual:
            return false
        }
    }
}

/// 一次刷新请求被策略跳过的原因。
enum ConsoleAccountsRefreshSkipReason: Equatable {
    /// 同级别或更新的请求正在执行。
    case inFlight
    /// 冷启动尚未开始，切窗/切页/定时不抢跑。
    case awaitingInitialLoad
    /// 距离上次成功读取未满 10 分钟。
    case fresh(remaining: TimeInterval)
    /// 上一次刷新失败，处于退避等待窗口内。
    case backoff(remaining: TimeInterval)
}

/// 一次账号刷新的最终落点。
enum ConsoleAccountsRefreshOutcome: Equatable {
    case skipped(ConsoleAccountsRefreshSkipReason)
    case succeeded(accounts: [ConsoleAccount], at: Date)
    /// 失败：保留旧快照与旧的 `lastSuccessAt`，同时明确记录本次尝试时间与错误原因。
    case failed(error: String, attemptedAt: Date, preservedLastSuccessAt: Date?)
    /// 已被更新代次的请求取代：旧结果（无论成功还是失败）直接丢弃。
    case superseded
}

/// 账号快照的只读元数据状态。
///
/// 三者严格分离：
/// - `lastSuccessAt`：最后一次**成功**读到账号快照的时间（失败绝不更新）；
/// - `lastAttemptAt`：最近一次发起/结束刷新尝试的时间（用于失败退避与防抖）；
/// - `lastError`：最近一次失败原因（静默失败也必须记录，界面据此提示数据已过期）。
struct ConsoleAccountsRefreshState: Equatable {
    var lastSuccessAt: Date?
    var lastAttemptAt: Date?
    var lastError: String?
    var consecutiveFailures: Int = 0
    var isLoading: Bool = false

    /// 快照是否已过期：已有旧快照但最近一次刷新失败，或距上次成功读取已超过 TTL。
    func isSnapshotStale(
        now: Date = .now,
        ttl: TimeInterval = ConsoleAccountsRefreshPolicy.defaultTTL
    ) -> Bool {
        guard let lastSuccessAt else { return false }
        if lastError != nil { return true }
        return now.timeIntervalSince(lastSuccessAt) >= ttl
    }
}

/// 主控制台账号刷新统一协调器（纯状态机，可在 `AICCCoreTests` 中确定性测试）。
@MainActor
final class ConsoleAccountsRefreshCoordinator {
    nonisolated static let defaultBaseFailureBackoff: TimeInterval = 30
    nonisolated static let defaultMaxFailureBackoff: TimeInterval = ConsoleAccountsRefreshPolicy.defaultTTL

    let ttl: TimeInterval
    let baseFailureBackoff: TimeInterval
    let maxFailureBackoff: TimeInterval

    private(set) var state = ConsoleAccountsRefreshState()
    private(set) var latestGeneration: UInt64 = 0
    private var inFlightGeneration: UInt64?
    private var inFlightTrigger: ConsoleAccountsRefreshTrigger?

    init(
        ttl: TimeInterval = ConsoleAccountsRefreshPolicy.defaultTTL,
        baseFailureBackoff: TimeInterval = ConsoleAccountsRefreshCoordinator.defaultBaseFailureBackoff,
        maxFailureBackoff: TimeInterval = ConsoleAccountsRefreshCoordinator.defaultMaxFailureBackoff
    ) {
        self.ttl = ttl
        self.baseFailureBackoff = baseFailureBackoff
        self.maxFailureBackoff = maxFailureBackoff
    }

    /// 连续失败后的退避间隔：30s → 60s → 120s → 240s → 480s → 上限 600s（10 分钟）。
    nonisolated static func failureBackoff(
        failures: Int,
        base: TimeInterval = ConsoleAccountsRefreshCoordinator.defaultBaseFailureBackoff,
        max maxDelay: TimeInterval = ConsoleAccountsRefreshCoordinator.defaultMaxFailureBackoff
    ) -> TimeInterval {
        guard failures > 0 else { return 0 }
        let exponent = min(failures - 1, 10)
        let candidate = base * pow(2.0, Double(exponent))
        return min(maxDelay, candidate)
    }

    /// 判断本次触发是否应当执行；返回 `nil` 表示允许发起请求。
    func evaluate(trigger: ConsoleAccountsRefreshTrigger, now: Date) -> ConsoleAccountsRefreshSkipReason? {
        if inFlightGeneration != nil {
            // 手动刷新允许抢占挂在后台的旧自动刷新（通过 generation 丢弃旧结果）；
            // 同为手动刷新，或任意自动刷新撞上在途请求，一律防重跳过。
            if !(trigger == .manual && inFlightTrigger?.isAutomatic == true) {
                return .inFlight
            }
        }

        if trigger == .manual {
            return nil
        }

        if trigger.requiresInitialLoadAttempt,
           state.lastSuccessAt == nil,
           state.lastAttemptAt == nil {
            return .awaitingInitialLoad
        }

        if state.consecutiveFailures > 0, let lastAttempt = state.lastAttemptAt {
            let backoff = Self.failureBackoff(
                failures: state.consecutiveFailures,
                base: baseFailureBackoff,
                max: maxFailureBackoff
            )
            let elapsed = now.timeIntervalSince(lastAttempt)
            if elapsed < backoff {
                return .backoff(remaining: backoff - elapsed)
            }
            return nil
        }

        if let lastSuccess = state.lastSuccessAt {
            let elapsed = now.timeIntervalSince(lastSuccess)
            if elapsed < ttl {
                return .fresh(remaining: ttl - elapsed)
            }
        }

        return nil
    }

    /// 计算下一次前台自动只读刷新的等待秒数；`nil` 表示冷启动尚未发生或已有刷新在途，暂不排期。
    func nextAutoRefreshDelay(now: Date) -> TimeInterval? {
        guard state.lastSuccessAt != nil || state.lastAttemptAt != nil else {
            return nil
        }
        // 已有刷新在途时暂停排期，等在途请求落定后再由外层重新 reconcile，避免到点空转。
        guard inFlightGeneration == nil else {
            return nil
        }
        if state.consecutiveFailures > 0, let lastAttempt = state.lastAttemptAt {
            let backoff = Self.failureBackoff(
                failures: state.consecutiveFailures,
                base: baseFailureBackoff,
                max: maxFailureBackoff
            )
            return max(0, backoff - now.timeIntervalSince(lastAttempt))
        }
        if let lastSuccess = state.lastSuccessAt {
            return max(0, ttl - now.timeIntervalSince(lastSuccess))
        }
        return 0
    }

    /// 统一执行一次账号只读刷新。
    ///
    /// `workerUnavailableMessage` 仅在 `ensureWorker()` 返回 `false` **之后**才求值，
    /// 确保拿到的是本次启动检查结束后的真实失败原因，而不是检查前的旧状态。
    func run(
        trigger: ConsoleAccountsRefreshTrigger,
        now: () -> Date = { .now },
        workerUnavailableMessage: () -> String,
        onStateChange: ((ConsoleAccountsRefreshState) -> Void)? = nil,
        ensureWorker: () async -> Bool,
        fetch: () async throws -> [ConsoleAccount]
    ) async -> ConsoleAccountsRefreshOutcome {
        let startTime = now()
        if let skip = evaluate(trigger: trigger, now: startTime) {
            return .skipped(skip)
        }

        latestGeneration += 1
        let generation = latestGeneration
        inFlightGeneration = generation
        inFlightTrigger = trigger
        state.isLoading = true
        state.lastAttemptAt = startTime
        onStateChange?(state)

        guard await ensureWorker() else {
            guard generation == latestGeneration else { return .superseded }
            let finishedAt = now()
            let rawMessage = workerUnavailableMessage().trimmingCharacters(in: .whitespacesAndNewlines)
            let message = rawMessage.isEmpty ? "账号助手未就绪" : rawMessage
            inFlightGeneration = nil
            inFlightTrigger = nil
            state.isLoading = false
            state.lastAttemptAt = finishedAt
            state.lastError = message
            state.consecutiveFailures += 1
            onStateChange?(state)
            return .failed(
                error: message,
                attemptedAt: finishedAt,
                preservedLastSuccessAt: state.lastSuccessAt
            )
        }

        do {
            let accounts = try await fetch()
            guard generation == latestGeneration else { return .superseded }
            let finishedAt = now()
            inFlightGeneration = nil
            inFlightTrigger = nil
            state.isLoading = false
            state.lastSuccessAt = finishedAt
            state.lastAttemptAt = finishedAt
            state.lastError = nil
            state.consecutiveFailures = 0
            onStateChange?(state)
            return .succeeded(accounts: accounts, at: finishedAt)
        } catch {
            guard generation == latestGeneration else { return .superseded }
            let finishedAt = now()
            let message = RotationRefreshOutcome.describe(error)
            inFlightGeneration = nil
            inFlightTrigger = nil
            state.isLoading = false
            state.lastAttemptAt = finishedAt
            state.lastError = message
            state.consecutiveFailures += 1
            onStateChange?(state)
            return .failed(
                error: message,
                attemptedAt: finishedAt,
                preservedLastSuccessAt: state.lastSuccessAt
            )
        }
    }

    /// 离屏视觉回归用：灌入固定样本状态，不触发真实 RPC。
    func applyFixture(lastSuccessAt: Date?, error: String?, isLoading: Bool) {
        latestGeneration += 1
        inFlightGeneration = nil
        inFlightTrigger = nil
        state = ConsoleAccountsRefreshState(
            lastSuccessAt: lastSuccessAt,
            lastAttemptAt: lastSuccessAt,
            lastError: error,
            consecutiveFailures: error == nil ? 0 : 1,
            isLoading: isLoading
        )
    }
}

/// 前台停留时的 10 分钟自动只读刷新调度器。
///
/// 规则：
/// - 仅在**主窗口处于前台**且**当前位于总览或 AI 账号页**时运行；
/// - 窗口进入后台、最小化、关闭，或切换到额度/任务页时立刻取消定时任务；
/// - 窗口持续停留不切页、不切应用时，也会在距上次成功满 10 分钟（或失败退避结束）后按时触发。
@MainActor
final class ConsoleAccountsAutoRefreshScheduler {
    typealias SleepFn = @Sendable (TimeInterval) async throws -> Void

    private(set) var isForeground = false
    private(set) var activePage: ConsolePage = .overview
    private var scheduledTask: Task<Void, Never>?
    private var activeTaskID = UUID()
    private let sleep: SleepFn
    private let now: () -> Date
    private let minimumRetryDelay: TimeInterval

    var isEligible: Bool {
        isForeground && activePage.supportsAccountsAutoRefresh
    }

    var isScheduled: Bool {
        scheduledTask != nil
    }

    init(
        sleep: SleepFn? = nil,
        now: @escaping () -> Date = { .now },
        minimumRetryDelay: TimeInterval = 1.0
    ) {
        self.sleep = sleep ?? { seconds in
            try await ConsoleAccountsAutoRefreshScheduler.defaultSleep(seconds)
        }
        self.now = now
        self.minimumRetryDelay = minimumRetryDelay > 0 ? minimumRetryDelay : 1.0
    }

    nonisolated static func defaultSleep(_ seconds: TimeInterval) async throws {
        let clamped = max(0, seconds)
        let nanos = UInt64((clamped * 1_000_000_000.0).rounded())
        try await Task.sleep(nanoseconds: nanos)
    }

    func update(
        isForeground: Bool,
        activePage: ConsolePage,
        nextDelay: @escaping (Date) -> TimeInterval?,
        onTick: @escaping () async -> Void
    ) {
        self.isForeground = isForeground
        self.activePage = activePage
        reconcile(nextDelay: nextDelay, onTick: onTick)
    }

    func reconcile(
        nextDelay: @escaping (Date) -> TimeInterval?,
        onTick: @escaping () async -> Void
    ) {
        stop()
        guard isEligible else { return }
        guard nextDelay(now()) != nil else { return }

        let taskID = UUID()
        activeTaskID = taskID
        scheduledTask = Task { @MainActor [weak self] in
            var hasTicked = false
            while !Task.isCancelled {
                guard let currentSelf = self,
                      currentSelf.isEligible,
                      currentSelf.activeTaskID == taskID,
                      let rawDelay = nextDelay(currentSelf.now()) else { break }
                let delay: TimeInterval
                if rawDelay > 0 {
                    delay = rawDelay
                } else if hasTicked {
                    // 已触发过 tick 后，若下一次延迟仍 <= 0（例如实时时钟持续前进、
                    // 在途刷新导致本次 tick 被跳过或状态尚未推进截止时间），
                    // 强制等待 minimumRetryDelay，绝不零延迟忙循环空转。
                    delay = currentSelf.minimumRetryDelay
                } else {
                    delay = 0
                }
                let sleepFn = currentSelf.sleep

                if delay > 0 {
                    do {
                        try await sleepFn(delay)
                    } catch {
                        break
                    }
                }
                guard let tickSelf = self,
                      !Task.isCancelled,
                      tickSelf.isEligible,
                      tickSelf.activeTaskID == taskID else { break }
                hasTicked = true
                await onTick()
            }
            if let currentSelf = self, currentSelf.activeTaskID == taskID {
                currentSelf.scheduledTask = nil
            }
        }
    }

    func stop() {
        activeTaskID = UUID()
        scheduledTask?.cancel()
        scheduledTask = nil
    }
}

/// 页头「账号快照更新时间」的纯派生视图模型。
///
/// 规则：
/// - 仅在总览 / AI 账号页显示；
/// - 文案明确限定为「账号快照」的最后**成功**读取时间，避免在总览页误导为整页所有数据都已更新；
/// - 静默或手动刷新失败时保留旧快照，不推进时间，并明确标出「已过期」。
struct ConsoleAccountsHeaderStatus: Equatable {
    enum Tone: Equatable {
        case neutral
        case warning
    }

    let text: String
    let help: String
    let tone: Tone
    let isStale: Bool

    static func make(
        page: ConsolePage,
        lastSuccessAt: Date?,
        error: String?,
        now: Date = .now,
        ttl: TimeInterval = ConsoleAccountsRefreshPolicy.defaultTTL,
        formatTime: (Date) -> String
    ) -> ConsoleAccountsHeaderStatus? {
        guard page.supportsAccountsAutoRefresh else { return nil }
        let trimmed = error?.trimmingCharacters(in: .whitespacesAndNewlines)
        let activeError = (trimmed?.isEmpty == false) ? trimmed : nil

        if let lastSuccessAt {
            let timeText = formatTime(lastSuccessAt)
            if let activeError {
                return ConsoleAccountsHeaderStatus(
                    text: "账号更新于 \(timeText) · 已过期",
                    help: "账号快照最后成功读取于 \(timeText)。最近一次刷新失败（\(activeError)），当前保留旧数据，请点刷新重试。",
                    tone: .warning,
                    isStale: true
                )
            }
            if now.timeIntervalSince(lastSuccessAt) >= ttl {
                return ConsoleAccountsHeaderStatus(
                    text: "账号更新于 \(timeText) · 已过期",
                    help: "账号快照最后成功读取于 \(timeText)，已超过 10 分钟，点刷新可立即重读。",
                    tone: .warning,
                    isStale: true
                )
            }
            return ConsoleAccountsHeaderStatus(
                text: "账号更新于 \(timeText)",
                help: "账号快照最后成功读取于 \(timeText)（仅在读取成功时更新）。",
                tone: .neutral,
                isStale: false
            )
        }

        if let activeError {
            return ConsoleAccountsHeaderStatus(
                text: "账号快照未读取",
                help: "账号快照尚未成功读取：\(activeError)",
                tone: .warning,
                isStale: true
            )
        }
        return nil
    }
}
