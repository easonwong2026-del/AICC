import Foundation
import SwiftUI

/// 主控制台的数据中枢：账号、额度、助手状态都从这里出。
///
/// 为什么集中在一处：
/// - 切换分页不能重新起助手进程，也不能重新打一遍 Magpie；
/// - 总览页和额度页必须看到同一份快照，否则两个页面数字会打架。
///
/// 这里只做"取数"，不做任何写入；扫码、签到等未接入能力不在这里假装。
@MainActor
final class ConsoleStore: ObservableObject {
    @Published private(set) var accounts: [ConsoleAccount] = []
    @Published private(set) var snapshot: QuotaSnapshot?
    @Published private(set) var accountsError: String?
    @Published private(set) var quotaError: String?
    @Published private(set) var loadingAccounts = false
    @Published private(set) var loadingQuota = false
    @Published private(set) var lastAccountsRefresh: Date?
    @Published private(set) var lastQuotaRefresh: Date?

    /// Buddy 旅行的本地快照 + 运行信息。
    @Published private(set) var travelRows: [TravelAccountRow] = []
    @Published private(set) var travelAuto: TravelAutoStatus?
    @Published private(set) var travelError: String?
    @Published private(set) var travelLoading = false
    /// 正在执行动作的账号 id，用来禁用该行按钮并显示进度。
    @Published private(set) var travelActing: String?
    /// 最近一次动作的一句话结果（成功/失败都给一句），显示在区域底部。
    @Published private(set) var travelNotice: String?

    /// 成长中心（PR #67：只读）。没有扫描过就是空，不预填假数据。
    @Published private(set) var growthResult: GrowthScanResult = .empty
    @Published private(set) var growthError: String?
    @Published private(set) var growthLoading = false
    @Published private(set) var lastGrowthScan: Date?
    /// 展开某个账号的任务列表（默认只展开第一个，避免一屏全是任务）。
    @Published var expandedGrowthAccount: String?

    /// 自动管理（PR #68）。开关默认关闭，状态从 worker 读。
    @Published private(set) var growthAuto: GrowthAutoStatus = GrowthAutoStatus(json: nil)
    @Published private(set) var growthRuns: [String: GrowthRuns] = [:]
    @Published private(set) var growthActing = false
    /// 最近一次手动检查 / 开关动作的一句话结果。
    @Published private(set) var growthNotice: String?

    let worker: AccountWorkerManager
    private let settings: AppSettings

    init(worker: AccountWorkerManager, settings: AppSettings) {
        self.worker = worker
        self.settings = settings
    }

    var stats: ConsoleOverviewStats { ConsoleOverviewStats(accounts: accounts) }
    var quotaSummary: ConsoleQuotaSummary { ConsoleQuotaSummary(snapshot: snapshot) }

    /// 首次进入窗口时拉一遍。重复调用是安全的。
    func loadIfNeeded() async {
        if snapshot == nil && !loadingQuota {
            await refreshQuota(force: false)
        }
        if lastAccountsRefresh == nil && !loadingAccounts {
            await refreshAccounts()
        }
    }

    func refreshAll() async {
        async let accountsTask: Void = refreshAccounts()
        async let quotaTask: Void = refreshQuota(force: true)
        _ = await (accountsTask, quotaTask)
    }

    // MARK: - Magpie 导入

    func magpiePreview() async throws -> MagpiePreview {
        try await worker.magpiePreview()
    }

    func importMagpieAccounts(keys: [String]) async throws -> MagpieImportResult {
        try await worker.importMagpieAccounts(keys: keys)
    }

    /// 用真实接口核一次登录态（只读）。
    func workbuddyStatus(accountId: String) async throws -> [String: Any] {
        try await worker.workbuddyStatus(accountId: accountId)
    }

    func refreshAccounts() async {
        guard !loadingAccounts else { return }
        loadingAccounts = true
        defer { loadingAccounts = false }

        // 助手没起就起一次；起不来时把原因原样带到界面上。
        if case .running = worker.state {} else {
            await worker.refresh()
        }
        // 另一个页面可能正在拉起助手，`refresh()` 会立刻返回 .starting。
        // 这种情况等它落定，别把"启动中"当成错误甩给用户。
        if case .starting = worker.state {
            for _ in 0..<20 {
                try? await Task.sleep(nanoseconds: 200_000_000)
                if case .starting = worker.state { continue }
                break
            }
        }
        guard case .running = worker.state else {
            accountsError = workerFailureText
            accounts = []
            lastAccountsRefresh = Date()
            return
        }

        do {
            let rows = try await worker.listAccounts()
            accounts = rows.compactMap(ConsoleAccount.init(json:))
            accountsError = nil
        } catch {
            accounts = []
            accountsError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
        lastAccountsRefresh = Date()
    }

    func refreshQuota(force: Bool = true) async {
        guard !loadingQuota else { return }
        loadingQuota = true
        defer { loadingQuota = false }

        let result = await WidgetStatusLoader.snapshot(force: force, port: settings.magpiePort)
        snapshot = result
        quotaError = result.accounts.isEmpty
            ? "没有读到额度。确认 Magpie 正在运行（端口 \(settings.magpiePort)）。"
            : nil
        lastQuotaRefresh = Date()
    }

    // MARK: - Buddy 旅行

    /// 拉一次本地快照。不发旅行接口，便宜到可以 5 秒轮询一次。
    func loadTravel() async {
        guard !travelLoading else { return }
        travelLoading = true
        defer { travelLoading = false }
        do {
            let summary = try await worker.travelSummary()
            travelRows = summary.rows
            travelAuto = summary.auto
            travelError = nil
            // 记一个「有没有开自动旅行」的标记：App 下次启动要靠它决定
            // 要不要提前把助手拉起来（否则用户设了自动、重启后没人跑）。
            TravelAutomationService.remember(summary.automationRunning)
            if !summary.automationRunning {
                TravelAutomationService.shared.stopIfIdle()
            }
        } catch {
            travelRows = []
            travelAuto = nil
            travelError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    func setTravelEnabled(_ row: TravelAccountRow, enabled: Bool) async {
        travelActing = row.accountId
        defer { travelActing = nil }
        do {
            try await worker.travelSetEnabled(accountId: row.accountId, enabled: enabled)
            travelNotice = enabled ? "已开启 \(row.nickname) 的自动旅行" : "已关闭 \(row.nickname) 的自动旅行"
            await loadTravel()
            TravelAutomationService.shared.startIfNeeded(worker: worker)
        } catch {
            travelNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 手动派出 / 领取 / 只查状态。
    func runTravel(_ method: TravelMethod, row: TravelAccountRow) async {
        travelActing = row.accountId
        defer { travelActing = nil }
        do {
            let result = try await worker.travelSync(method, accountId: row.accountId)
            travelNotice = result.summaryText ?? "\(row.nickname) 已更新"
            await loadTravel()
        } catch {
            travelNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            await loadTravel()
        }
    }

    // MARK: - 成长中心（PR #67：只读）

    /// 扫一遍成长中心。**由用户点按钮触发**，不自动轮询 ——
    /// 一次扫描要打 9 个接口 × N 个账号，静默定时跑会让用户以为"我没让它联网"。
    ///
    /// 也只做 GET：接受任务、领奖在 #68，各自会有独立开关。
    func loadGrowth(accountIds: [String] = []) async {
        guard !growthLoading else { return }
        growthLoading = true
        defer { growthLoading = false }
        do {
            let json = try await worker.growthScan(accountIds: accountIds)
            let result = GrowthScanResult(json: json)
            growthResult = result
            growthError = nil
            lastGrowthScan = Date()
            // 默认展开第一个"有事要做"的账号：全是空的时候展开哪一行都没意义。
            if expandedGrowthAccount == nil {
                expandedGrowthAccount = result.scans
                    .first(where: { $0.pendingCount > 0 || $0.needsAuth })?.accountId
                    ?? result.scans.first?.accountId
            }
            // 开关状态与执行记录一起刷新：任务行要显示"上次执行结果"。
            await loadGrowthAuto()
            await loadGrowthRuns()
        } catch {
            growthResult = .empty
            growthError = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    func toggleGrowthAccount(_ accountId: String) {
        expandedGrowthAccount = expandedGrowthAccount == accountId ? nil : accountId
    }

    // MARK: - 成长中心自动管理（PR #68）

    /// 读开关与下次检查时间。便宜，界面可以随扫描一起调。
    func loadGrowthAuto() async {
        do {
            growthAuto = GrowthAutoStatus(json: try await worker.growthAutoStatus())
        } catch {
            // 读不到就保持原样：这是次要信息，不该把整页打成错误态。
        }
    }

    /// 打开 / 关闭自动管理。**打开之后才可能有写操作**。
    func setGrowthAutoEnabled(_ enabled: Bool) async {
        growthActing = true
        defer { growthActing = false }
        do {
            growthAuto = GrowthAutoStatus(json: try await worker.setGrowthAutoEnabled(enabled))
            growthNotice = enabled
                ? "已开启自动管理：会替你接单并领取已完成任务的奖励"
                : "已关闭自动管理：只保留手动扫描"
        } catch {
            growthNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 手动跑一轮。`force` 跳过间隔判断，但仍然受总开关约束。
    func runGrowthTick(force: Bool = true) async {
        growthActing = true
        defer { growthActing = false }
        do {
            let json = try await worker.growthTick(force: force)
            let result = GrowthTickResult(json: json)
            if let skipped = result.skippedText {
                growthNotice = skipped
            } else {
                growthNotice = result.summary ?? "检查完成"
            }
            await loadGrowthAuto()
            // 跑完把最新状态与记录一起刷新，界面不用等下一轮。
            await loadGrowth()
        } catch {
            growthNotice = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    /// 读所有账号的执行记录。失败保持原样，不影响任务列表。
    func loadGrowthRuns() async {
        for scan in growthResult.scans {
            guard let runs = try? await worker.growthRuns(accountId: scan.accountId) else { continue }
            growthRuns[scan.accountId] = GrowthRuns(json: runs)
        }
    }

    private var workerFailureText: String {        switch worker.state {
        case .failed(let message):
            return message.components(separatedBy: "\n").first ?? "账号助手异常"
        case .starting:
            return "账号助手启动中，请稍后重试"
        case .stopped:
            return "账号助手未启动"
        case .running:
            return ""
        }
    }
}
